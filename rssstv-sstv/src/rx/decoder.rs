use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use crate::color::{YCrCb8, y_cr_cb_to_rgb};
use crate::image::{ImageSize, Rgb8, RgbImage};
use crate::mode::{Mode, RasterOrganization, ScanChannel, Support};
use crate::signal::Frequency;
use crate::{RxProcessError, SstvError};

use super::acquisition::{acquire, window_samples};
use super::clock::{RasterClock, ceil_sample};
use super::config::{RxConfig, Staging};
use super::event::{RxEvent, RxOutcome, RxProcess, RxState, StopReason};
use super::input::{DemodulatedBlock, SampleBuffer};
use super::raster::{PixelSegment, RasterProfile};
use super::slant::{SlantEstimate, SlantEstimator};
use super::sync::{MIN_CONFIDENCE, SyncObservation, observe, push_bounded};

const BAD_SYNC_SCORE_LIMIT: u8 = 12;
const BAD_SYNC_PENALTY: u8 = 2;
const PHASE_AGREEMENT: usize = 3;
const PHASE_HOLDOFF_UNITS: usize = 6;
const MIN_PHASE_DISPLACEMENT: u64 = 2;
const PIXEL_GUARD: f64 = 0.25;

/// Result of rebuilding an image from staged immutable samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RefinementResult {
    /// New monotonically increasing image revision.
    pub revision: u64,
    /// Global timing estimate used for the rebuild.
    pub slant: SlantEstimate,
}

/// Concrete streaming decoder for the supported SSTV receive modes.
///
/// Input must extend at least one demodulated sample beyond the nominal raster
/// end so the final guarded pixel window can become available.
#[derive(Debug)]
pub struct RxDecoder {
    mode: Mode,
    profile: RasterProfile,
    sample_rate_hz: u32,
    config: RxConfig,
    decode: DecodeState,
    input: Option<SampleBuffer>,
    next_sample: Option<u64>,
    observations: VecDeque<SyncObservation>,
    staged_observations: Vec<SyncObservation>,
    phase_displacements: VecDeque<i64>,
    last_phase_adjustment: Option<usize>,
    bad_sync_score: u8,
    staged: Option<SampleBuffer>,
    image_revision: u64,
    rebuilding: bool,
}

#[derive(Debug)]
struct DecodeState {
    image: RgbImage,
    state: RxState,
    clock: Option<RasterClock>,
    raster_unit: usize,
    delivered_rows: usize,
    pending_row: Option<usize>,
    pending_events: VecDeque<RxEvent>,
    robot_chroma: [Vec<u8>; 2],
    robot_selector: Option<RobotSelector>,
}

impl DecodeState {
    fn new(size: ImageSize) -> Self {
        Self {
            image: RgbImage::new(size, Rgb8::default()),
            state: RxState::Acquiring,
            clock: None,
            raster_unit: 0,
            delivered_rows: 0,
            pending_row: None,
            pending_events: VecDeque::with_capacity(3),
            robot_chroma: [vec![128; size.width()], vec![128; size.width()]],
            robot_selector: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RobotSelector {
    Cr,
    Cb,
}

impl RobotSelector {
    const fn opposite(self) -> Self {
        match self {
            Self::Cr => Self::Cb,
            Self::Cb => Self::Cr,
        }
    }

    const fn plane(self) -> usize {
        match self {
            Self::Cr => 0,
            Self::Cb => 1,
        }
    }
}

impl RxDecoder {
    /// Validates mode and sample rate and allocates the transport-sized image.
    pub fn new(mode: Mode, sample_rate_hz: u32) -> Result<Self, SstvError> {
        Self::with_config(mode, sample_rate_hz, RxConfig::default())
    }

    /// Validates inputs and constructs a decoder with explicit receive options.
    pub fn with_config(
        mode: Mode,
        sample_rate_hz: u32,
        config: RxConfig,
    ) -> Result<Self, SstvError> {
        if sample_rate_hz == 0 {
            return Err(SstvError::InvalidSampleRate);
        }
        let Some(profile) = RasterProfile::for_mode(mode) else {
            return Err(SstvError::UnsupportedRxMode(mode));
        };
        if mode.spec().decode_support() != Support::Supported {
            return Err(SstvError::UnsupportedRxMode(mode));
        }
        let size = ImageSize::new(mode.spec().width() as usize, mode.spec().height() as usize)
            .expect("mode dimensions are valid");
        Ok(Self {
            mode,
            profile,
            sample_rate_hz,
            config,
            decode: DecodeState::new(size),
            input: None,
            next_sample: None,
            observations: VecDeque::with_capacity(16),
            staged_observations: Vec::new(),
            phase_displacements: VecDeque::with_capacity(PHASE_AGREEMENT),
            last_phase_adjustment: None,
            bad_sync_score: 0,
            staged: None,
            image_revision: 0,
            rebuilding: false,
        })
    }

    /// Returns the selected mode.
    pub const fn mode(&self) -> Mode {
        self.mode
    }

    /// Returns the current decoder state.
    pub const fn state(&self) -> RxState {
        self.decode.state
    }

    /// Returns the image, including decoded rows and untouched inactive rows.
    pub const fn image(&self) -> &RgbImage {
        &self.decode.image
    }

    /// Returns the acquired physical raster epoch, if available.
    pub fn source_epoch(&self) -> Option<u64> {
        self.decode.clock.map(|clock| clock.source_epoch())
    }

    /// Returns the acquired effective sample rate, if available.
    pub fn effective_sample_rate_hz(&self) -> Option<f64> {
        self.decode
            .clock
            .map(|clock| clock.effective_sample_rate_hz())
    }

    /// Returns the immutable physical sample rate supplied at construction.
    pub const fn physical_sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    /// Returns the bounded sync observation history from oldest to newest.
    pub fn sync_observations(&self) -> impl ExactSizeIterator<Item = &SyncObservation> {
        self.observations.iter()
    }

    /// Returns the number of paired samples retained for staged refinement.
    pub fn staged_samples_len(&self) -> usize {
        self.staged.as_ref().map_or(0, SampleBuffer::len)
    }

    /// Returns the current image revision, initially zero.
    pub const fn image_revision(&self) -> u64 {
        self.image_revision
    }

    /// Consumes a prefix and returns at most one event.
    ///
    /// For a non-empty valid block this either consumes at least one sample or
    /// returns an event. Once [`RxState::Complete`] is reached, further non-empty
    /// input is rejected. If processing fails after consuming a prefix, the
    /// returned [`RxProcessError`] reports that prefix so it is not resent.
    pub fn process(&mut self, block: DemodulatedBlock<'_>) -> Result<RxProcess, RxProcessError> {
        let expected = self.next_sample;
        self.process_inner(block).map_err(|error| {
            let consumed = if expected.is_none_or(|sample| sample == block.first_sample()) {
                self.next_sample
                    .and_then(|sample| sample.checked_sub(block.first_sample()))
                    .and_then(|count| usize::try_from(count).ok())
                    .unwrap_or(0)
                    .min(block.frequency_hz().len())
            } else {
                0
            };
            if consumed == 0 && expected.is_none() {
                self.input = None;
                self.staged = None;
                self.next_sample = None;
            }
            RxProcessError::new(consumed, error)
        })
    }

    fn process_inner(&mut self, block: DemodulatedBlock<'_>) -> Result<RxProcess, SstvError> {
        block.validate_header(self.next_sample)?;
        if let Some(event) = self.poll_event() {
            return Ok(RxProcess::new(0, Some(event)));
        }
        if matches!(
            self.decode.state,
            RxState::Complete | RxState::Stopped { .. }
        ) {
            return if block.frequency_hz().is_empty() {
                Ok(RxProcess::new(0, None))
            } else {
                Err(SstvError::RxAlreadyComplete)
            };
        }
        if self.input.is_none() {
            self.input = Some(SampleBuffer::new(block.first_sample()));
            if matches!(self.config.staging, Staging::Memory { .. }) {
                self.staged = Some(SampleBuffer::new(block.first_sample()));
            }
            self.next_sample = Some(block.first_sample());
        }

        let mut consumed = 0;
        loop {
            if self.decode.clock.is_none() {
                let input = self.input.as_ref().expect("input initialized");
                let target = window_samples(self.profile, self.sample_rate_hz);
                if input.len() as u64 >= target {
                    match acquire(input, self.profile, self.sample_rate_hz) {
                        Ok(clock) => {
                            self.decode.clock = Some(clock);
                            self.decode.state = RxState::Decoding { completed_rows: 0 };
                            return Ok(RxProcess::new(
                                consumed,
                                Some(RxEvent::RasterAcquired {
                                    source_epoch: clock.source_epoch(),
                                    effective_sample_rate_hz: clock.effective_sample_rate_hz(),
                                }),
                            ));
                        }
                        Err(SstvError::RasterNotAcquired) => {
                            let advance = (target / 3).max(1);
                            let first = input.first();
                            self.input
                                .as_mut()
                                .expect("input initialized")
                                .discard_before(first.saturating_add(advance));
                            if consumed == block.frequency_hz().len() {
                                return Ok(RxProcess::new(consumed, None));
                            }
                            continue;
                        }
                        Err(error) => return Err(error),
                    }
                }
                let needed = usize::try_from(target - input.len() as u64).unwrap_or(usize::MAX);
                let take = needed.min(block.frequency_hz().len() - consumed);
                self.append(block, consumed, take)?;
                consumed += take;
                if take == 0 {
                    return Ok(RxProcess::new(consumed, None));
                }
                continue;
            }

            if self.can_decode_next()? {
                if !self.synchronize()? {
                    let event = self.poll_event().expect("stop event queued");
                    return Ok(RxProcess::new(consumed, Some(event)));
                }
                if !self.can_decode_next()? {
                    if let Some(event) = self.poll_event() {
                        return Ok(RxProcess::new(consumed, Some(event)));
                    }
                    continue;
                }
                if let Some(row) = self.decode_next(false)? {
                    self.queue_event(RxEvent::RowDecoded { row });
                }
                if let Some(row) = self.decode.pending_row.take() {
                    self.queue_event(RxEvent::RowDecoded { row });
                }
                if let Some(event) = self.poll_event() {
                    return Ok(RxProcess::new(consumed, Some(event)));
                }
                continue;
            }
            let remaining = block.frequency_hz().len() - consumed;
            if remaining == 0 {
                return Ok(RxProcess::new(consumed, None));
            }
            let required_end = self.required_end()?;
            let available_end = self.input.as_ref().expect("input initialized").end();
            let needed =
                usize::try_from(required_end.saturating_sub(available_end)).unwrap_or(usize::MAX);
            let take = needed.max(1).min(remaining);
            self.append(block, consumed, take)?;
            consumed += take;
        }
    }

    /// Consumes the decoder and returns its complete or partial image.
    pub fn finish(self) -> RxOutcome {
        let DecodeState { image, state, .. } = self.decode;
        match state {
            RxState::Complete => RxOutcome::Complete(image),
            RxState::Stopped { reason, .. } => RxOutcome::Stopped { image, reason },
            state => RxOutcome::Incomplete { image, state },
        }
    }

    /// Retains contiguous samples after normal completion for staged refinement.
    ///
    /// Offline sources can provide their remaining tail after the active image
    /// has completed so a fitted raster clock still has enough guarded coverage.
    pub fn stage_for_refinement(&mut self, block: DemodulatedBlock<'_>) -> Result<(), SstvError> {
        if self.decode.state != RxState::Complete {
            return Err(SstvError::RxNotComplete);
        }
        block.validate_header(self.next_sample)?;
        block.validate_range(0, block.frequency_hz().len())?;
        let Some(staged) = self.staged.as_mut() else {
            return Err(SstvError::StagingDisabled);
        };
        let max_samples = match self.config.staging {
            Staging::Memory { max_samples } => max_samples,
            Staging::Disabled => return Err(SstvError::StagingDisabled),
        };
        if staged
            .len()
            .checked_add(block.frequency_hz().len())
            .is_none_or(|length| length > max_samples)
        {
            return Err(SstvError::StagingCapacityExceeded { max_samples });
        }
        staged.append(block, block.frequency_hz().len());
        self.next_sample = Some(
            block
                .first_sample()
                .checked_add(block.frequency_hz().len() as u64)
                .ok_or(SstvError::SamplePositionOverflow)?,
        );
        Ok(())
    }

    /// Re-estimates global rate and epoch and rebuilds from immutable staged samples.
    ///
    /// This remains available after normal completion. It does not apply local
    /// warping and never mutates the retained demodulated samples.
    /// A successful rebuild is terminal and sets the decoder state to
    /// [`RxState::Complete`], so callers must collect the complete transmission
    /// before invoking this method.
    pub fn refine_staged(&mut self) -> Result<RefinementResult, SstvError> {
        if self.staged.is_none() {
            return Err(SstvError::StagingDisabled);
        }
        let estimator = SlantEstimator::for_mode(self.sample_rate_hz, self.mode)
            .expect("decoder mode has a raster profile");
        let slant = estimator
            .estimate(&self.staged_observations)
            .ok_or(SstvError::InsufficientStagedSync)?;
        let clock = RasterClock::from_estimate(slant.source_epoch, slant.effective_sample_rate_hz)?;
        let staged = self.staged.take().expect("staging checked above");
        if let Err(error) = self.validate_staged_coverage(&staged, clock) {
            self.staged = Some(staged);
            return Err(error);
        }
        let mut rebuilding = DecodeState::new(self.decode.image.size());
        rebuilding.clock = Some(clock);
        let old_decode = core::mem::replace(&mut self.decode, rebuilding);
        let saved_input = self.input.take();
        self.input = Some(staged);
        let units = self.raster_units();
        self.rebuilding = true;
        let rebuild_result = (|| {
            while self.decode.raster_unit < units {
                self.decode_next(true)?;
                self.decode.pending_row = None;
            }
            Ok(())
        })();
        self.rebuilding = false;
        self.staged = self.input.take();
        self.input = saved_input;
        if let Err(error) = rebuild_result {
            self.decode = old_decode;
            return Err(error);
        }
        self.decode.delivered_rows = self.mode.spec().active_rows() as usize;
        self.decode.state = RxState::Complete;
        self.image_revision = self.image_revision.saturating_add(1);
        Ok(RefinementResult {
            revision: self.image_revision,
            slant,
        })
    }

    fn append(
        &mut self,
        block: DemodulatedBlock<'_>,
        offset: usize,
        count: usize,
    ) -> Result<(), SstvError> {
        block.validate_range(offset, count)?;
        let first = block
            .first_sample()
            .checked_add(offset as u64)
            .ok_or(SstvError::SamplePositionOverflow)?;
        let part = DemodulatedBlock::new(
            first,
            &block.frequency_hz()[offset..offset + count],
            &block.sync_strength()[offset..offset + count],
        );
        if let Staging::Memory { max_samples } = self.config.staging {
            let staged_len = self.staged.as_ref().map_or(0, SampleBuffer::len);
            if staged_len
                .checked_add(count)
                .is_none_or(|len| len > max_samples)
            {
                return Err(SstvError::StagingCapacityExceeded { max_samples });
            }
            self.staged
                .as_mut()
                .expect("staging initialized")
                .append(part, count);
        }
        self.input
            .as_mut()
            .expect("input initialized")
            .append(part, count);
        self.next_sample = Some(
            first
                .checked_add(count as u64)
                .ok_or(SstvError::SamplePositionOverflow)?,
        );
        Ok(())
    }

    fn raster_units(&self) -> usize {
        let spec = self.mode.spec();
        spec.active_rows() as usize / spec.rows_per_raster_unit() as usize
    }

    /// Returns the half-open sample range averaged for one transmitted pixel.
    ///
    /// A guard band at both edges keeps sub-sample timing error from pulling a
    /// neighbouring porch or component into the average. Pixels too short to
    /// hold a guarded sample fall back to the reading nearest their centre.
    fn pixel_window(
        &self,
        clock: RasterClock,
        unit: usize,
        segment: PixelSegment,
        x: usize,
    ) -> Result<(u64, u64), SstvError> {
        let width = self.decode.image.size().width() as u128;
        let base = u128::from(self.profile.period_ps) * unit as u128 + u128::from(segment.start_ps);
        let edge = |index: u128| -> Result<f64, SstvError> {
            let protocol = base + u128::from(segment.duration_ps) * index / width;
            let protocol = u64::try_from(protocol).map_err(|_| SstvError::TimeOverflow)?;
            clock.position_at(protocol)
        };
        let left = edge(x as u128)?;
        let right = edge(x as u128 + 1)?;
        let guard = (right - left) * PIXEL_GUARD;
        let first = ceil_sample(left + guard);
        let end = ceil_sample(right - guard);
        if end > first {
            Ok((first, end))
        } else {
            let center = ceil_sample((left + right) * 0.5 - 0.5);
            Ok((center, center + 1))
        }
    }

    fn selector_window(&self, clock: RasterClock, unit: usize) -> Result<(u64, u64), SstvError> {
        let (start_ps, end_ps) = self
            .profile
            .selector_window_ps()
            .ok_or(SstvError::UnsupportedRxMode(self.mode))?;
        let unit_start = self
            .profile
            .period_ps
            .checked_mul(unit as u64)
            .ok_or(SstvError::TimeOverflow)?;
        let edge = |offset_ps: u64| -> Result<u64, SstvError> {
            clock.sample_from(
                unit_start
                    .checked_add(offset_ps)
                    .ok_or(SstvError::TimeOverflow)?,
            )
        };
        let first = edge(start_ps)?;
        let last = edge(end_ps)?;
        Ok((first, last.max(first.saturating_add(1))))
    }

    fn required_end(&self) -> Result<u64, SstvError> {
        let clock = self.decode.clock.expect("clock acquired");
        let last = self
            .profile
            .pixels()
            .last()
            .ok_or(SstvError::UnsupportedRxMode(self.mode))?;
        let width = self.decode.image.size().width();
        let (_, end) = self.pixel_window(clock, self.decode.raster_unit, last, width - 1)?;
        Ok(end)
    }

    fn validate_staged_coverage(
        &self,
        staged: &SampleBuffer,
        clock: RasterClock,
    ) -> Result<(), SstvError> {
        let width = self.decode.image.size().width();
        let require = |sample: u64| -> Result<(), SstvError> {
            if staged.frequency(sample).is_some() {
                Ok(())
            } else {
                Err(SstvError::InsufficientStagedData {
                    required_sample: sample,
                })
            }
        };
        for unit in 0..self.raster_units() {
            // Sampling positions increase monotonically, so covering the first
            // and last sample of every segment covers everything between them.
            for segment in self.profile.pixels().iter() {
                let (first, _) = self.pixel_window(clock, unit, segment, 0)?;
                let (_, end) = self.pixel_window(clock, unit, segment, width - 1)?;
                require(first)?;
                require(end - 1)?;
            }
            if self.profile.selector_window_ps().is_some() {
                let (first, end) = self.selector_window(clock, unit)?;
                require(first)?;
                require(end - 1)?;
            }
        }
        Ok(())
    }

    fn can_decode_next(&self) -> Result<bool, SstvError> {
        if self.decode.raster_unit >= self.raster_units() {
            return Ok(false);
        }
        Ok(self.input.as_ref().expect("input initialized").end() >= self.required_end()?)
    }

    fn segment(&self, channel: ScanChannel, row_offset: u8) -> Result<PixelSegment, SstvError> {
        self.profile
            .pixels()
            .get(channel, row_offset)
            .ok_or(SstvError::UnsupportedRxMode(self.mode))
    }

    fn decode_next(&mut self, rebuilding: bool) -> Result<Option<usize>, SstvError> {
        let width = self.decode.image.size().width();
        let unit = self.decode.raster_unit;
        match self.profile.organization {
            RasterOrganization::DirectGbr => {
                let green = self.segment(ScanChannel::Green, 0)?;
                let blue = self.segment(ScanChannel::Blue, 0)?;
                let red = self.segment(ScanChannel::Red, 0)?;
                for x in 0..width {
                    let g = self.level_at(unit, green, x)?;
                    let b = self.level_at(unit, blue, x)?;
                    let r = self.level_at(unit, red, x)?;
                    self.set_pixel(x, unit, Rgb8::new(r, g, b));
                }
            }
            RasterOrganization::YCrCb => {
                let luminance = self.segment(ScanChannel::Luminance, 0)?;
                let red = self.segment(ScanChannel::RedDifference, 0)?;
                let blue = self.segment(ScanChannel::BlueDifference, 0)?;
                for x in 0..width {
                    let y = self.level_at(unit, luminance, x)?;
                    let cr = self.level_at(unit, red, x)?;
                    let cb = self.level_at(unit, blue, x)?;
                    self.set_pixel(x, unit, y_cr_cb_to_rgb(YCrCb8 { y, cr, cb }));
                }
            }
            RasterOrganization::AlternatingYCrCb => {
                // Only one chrominance plane arrives per raster unit, so every
                // row is drawn from the received plane and the retained one.
                let luminance = self.segment(ScanChannel::Luminance, 0)?;
                let chroma = self.segment(ScanChannel::RedDifference, 0)?;
                let selector = self.robot_selector_at(unit)?.unwrap_or_else(|| {
                    self.decode
                        .robot_selector
                        .map(RobotSelector::opposite)
                        .unwrap_or(if unit & 1 == 0 {
                            RobotSelector::Cr
                        } else {
                            RobotSelector::Cb
                        })
                });
                for x in 0..width {
                    let y = self.level_at(unit, luminance, x)?;
                    let received = self.level_at(unit, chroma, x)?;
                    self.decode.robot_chroma[selector.plane()][x] = received;
                    let cr = self.decode.robot_chroma[0][x];
                    let cb = self.decode.robot_chroma[1][x];
                    self.set_pixel(x, unit, y_cr_cb_to_rgb(YCrCb8 { y, cr, cb }));
                }
                self.decode.robot_selector = Some(selector);
            }
            RasterOrganization::PairedYCrCb => {
                let first = self.segment(ScanChannel::Luminance, 0)?;
                let red = self.segment(ScanChannel::RedDifference, 0)?;
                let blue = self.segment(ScanChannel::BlueDifference, 0)?;
                let second = self.segment(ScanChannel::Luminance, 1)?;
                let row = unit * 2;
                for x in 0..width {
                    let y0 = self.level_at(unit, first, x)?;
                    let cr = self.level_at(unit, red, x)?;
                    let cb = self.level_at(unit, blue, x)?;
                    let y1 = self.level_at(unit, second, x)?;
                    self.set_pixel(x, row, y_cr_cb_to_rgb(YCrCb8 { y: y0, cr, cb }));
                    self.set_pixel(x, row + 1, y_cr_cb_to_rgb(YCrCb8 { y: y1, cr, cb }));
                }
                if !rebuilding {
                    self.decode.pending_row = Some(row + 1);
                }
            }
            RasterOrganization::DirectRgb | RasterOrganization::PairedLuminance => {
                return Err(SstvError::UnsupportedRxMode(self.mode));
            }
        }
        self.decode.raster_unit += 1;
        let discard = self
            .decode
            .clock
            .expect("clock acquired")
            .sample_at(self.profile.period_ps * self.decode.raster_unit as u64)?;
        if !rebuilding {
            self.input
                .as_mut()
                .expect("input initialized")
                .discard_before(discard);
        }
        Ok(Some(
            unit * self.mode.spec().rows_per_raster_unit() as usize,
        ))
    }

    fn frequency_at(&self, sample: u64) -> Result<f32, SstvError> {
        self.input
            .as_ref()
            .expect("input initialized")
            .frequency(sample)
            .ok_or(if self.rebuilding {
                SstvError::InsufficientStagedData {
                    required_sample: sample,
                }
            } else {
                SstvError::SamplePositionOverflow
            })
    }

    fn mean_frequency(&self, range: (u64, u64)) -> Result<f64, SstvError> {
        let (first, end) = range;
        let mut sum = 0.0_f64;
        for sample in first..end {
            sum += f64::from(self.frequency_at(sample)?);
        }
        Ok(sum / (end - first) as f64)
    }

    fn level_at(&self, unit: usize, segment: PixelSegment, x: usize) -> Result<u8, SstvError> {
        let clock = self.decode.clock.expect("clock acquired");
        let window = self.pixel_window(clock, unit, segment, x)?;
        // Averaging the whole transmitted pixel keeps every demodulated sample
        // instead of discarding all but one centre reading.
        Ok(self.frequency_to_level(self.mean_frequency(window)?))
    }

    fn frequency_to_level(&self, frequency_hz: f64) -> u8 {
        let hz = if frequency_hz > 0.0 {
            (frequency_hz + 0.5) as u32
        } else {
            0
        };
        self.mode
            .spec()
            .signal_band()
            .frequency_to_level(Frequency::from_hz(hz))
    }

    fn robot_selector_at(&self, unit: usize) -> Result<Option<RobotSelector>, SstvError> {
        let clock = self.decode.clock.expect("clock acquired");
        let average = self.mean_frequency(self.selector_window(clock, unit)?)?;
        Ok(if average <= 1700.0 {
            Some(RobotSelector::Cr)
        } else if average >= 2100.0 {
            Some(RobotSelector::Cb)
        } else {
            None
        })
    }

    fn set_pixel(&mut self, x: usize, y: usize, pixel: Rgb8) {
        if let Some(value) = self.decode.image.row_mut(y).and_then(|row| row.get_mut(x)) {
            *value = pixel;
        }
    }

    fn deliver_row(&mut self, row: usize) -> RxEvent {
        self.decode.delivered_rows += 1;
        if self.decode.delivered_rows == self.mode.spec().active_rows() as usize {
            self.decode.state = RxState::Complete;
        } else {
            self.decode.state = RxState::Decoding {
                completed_rows: self.decode.delivered_rows,
            };
        }
        RxEvent::RowDecoded { row }
    }

    fn synchronize(&mut self) -> Result<bool, SstvError> {
        let unit = self.decode.raster_unit;
        let observation = observe(
            self.input.as_ref().expect("input initialized"),
            self.profile,
            self.decode.clock.expect("clock acquired"),
            unit,
        );
        let Some(observation) = observation else {
            self.phase_displacements.clear();
            return self.note_bad_sync();
        };
        push_bounded(&mut self.observations, observation);
        self.staged_observations.push(observation);
        if observation.confidence < MIN_CONFIDENCE {
            self.phase_displacements.clear();
            return self.note_bad_sync();
        }
        let expected_protocol = self
            .profile
            .period_ps
            .checked_mul(unit as u64)
            .and_then(|value| value.checked_add(self.profile.sync_center_ps))
            .ok_or(SstvError::TimeOverflow)?;
        let expected = self
            .decode
            .clock
            .expect("clock acquired")
            .sample_at(expected_protocol)?;
        let displacement = observation.center_sample as i128 - expected as i128;
        let max_consistent = self
            .decode
            .clock
            .expect("clock acquired")
            .samples_for(self.profile.period_ps)?
            / 4;
        if displacement.unsigned_abs() > u128::from(max_consistent.max(2)) {
            self.phase_displacements.clear();
            return self.note_bad_sync();
        }
        let displacement =
            i64::try_from(displacement).map_err(|_| SstvError::SamplePositionOverflow)?;
        self.note_good_sync();
        if self.config.live_sync {
            if self.phase_displacements.len() == PHASE_AGREEMENT {
                self.phase_displacements.pop_front();
            }
            self.phase_displacements.push_back(displacement);
            let holdoff_done = self
                .last_phase_adjustment
                .is_none_or(|last| unit >= last + PHASE_HOLDOFF_UNITS);
            if self.phase_displacements.len() == PHASE_AGREEMENT && holdoff_done {
                let minimum = *self.phase_displacements.iter().min().expect("non-empty");
                let maximum = *self.phase_displacements.iter().max().expect("non-empty");
                let correction =
                    self.phase_displacements.iter().sum::<i64>() / PHASE_AGREEMENT as i64;
                if maximum - minimum <= 2 && correction.unsigned_abs() >= MIN_PHASE_DISPLACEMENT {
                    self.decode
                        .clock
                        .as_mut()
                        .expect("clock acquired")
                        .adjust_epoch(correction)?;
                    self.last_phase_adjustment = Some(unit);
                    self.phase_displacements.clear();
                    self.queue_event(RxEvent::PhaseAdjusted {
                        unit,
                        displacement_samples: correction,
                        source_epoch: self.decode.clock.expect("clock acquired").source_epoch(),
                    });
                }
            }
        }
        Ok(true)
    }

    fn check_auto_stop(&mut self) -> Result<bool, SstvError> {
        if self.config.auto_stop && self.bad_sync_score >= BAD_SYNC_SCORE_LIMIT {
            let reason = StopReason::SynchronizationLost;
            self.decode.state = RxState::Stopped {
                completed_rows: self.decode.delivered_rows,
                reason,
            };
            self.queue_event(RxEvent::Stopped { reason });
            Ok(false)
        } else {
            Ok(true)
        }
    }

    fn note_bad_sync(&mut self) -> Result<bool, SstvError> {
        self.bad_sync_score = self.bad_sync_score.saturating_add(BAD_SYNC_PENALTY);
        self.check_auto_stop()
    }

    fn note_good_sync(&mut self) {
        self.bad_sync_score = self.bad_sync_score.saturating_sub(1);
    }

    fn queue_event(&mut self, event: RxEvent) {
        self.decode.pending_events.push_back(event);
    }

    /// Returns the next queued event without requiring an empty input block.
    pub fn poll_event(&mut self) -> Option<RxEvent> {
        self.decode
            .pending_events
            .pop_front()
            .map(|event| self.emit(event))
    }

    fn emit(&mut self, event: RxEvent) -> RxEvent {
        match event {
            RxEvent::RowDecoded { row } => self.deliver_row(row),
            event => event,
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;
    use crate::signal::TxComponent;
    use crate::tx::TxEncoder;

    const SAMPLE_RATE: u32 = 10_000;
    const VIS_END_PS: u64 = 910_000_000_000;

    fn source_image(mode: Mode) -> RgbImage {
        let spec = mode.spec();
        let size = ImageSize::new(spec.width() as usize, spec.height() as usize).unwrap();
        RgbImage::new(size, Rgb8::new(64, 128, 192))
    }

    fn band_image(mode: Mode) -> RgbImage {
        let spec = mode.spec();
        let size = ImageSize::new(spec.width() as usize, spec.height() as usize).unwrap();
        let mut image = RgbImage::new(size, Rgb8::default());
        let width = size.width();
        for (index, pixel) in image.pixels_mut().iter_mut().enumerate() {
            // Two-row bands keep every chrominance pair inside one colour, so
            // subsampled modes stay exactly reproducible.
            let band = (index / width / 2) as u32;
            *pixel = Rgb8::new(
                (band * 7 % 251) as u8,
                (band * 29 % 241) as u8,
                (band * 53 % 239) as u8,
            );
        }
        image
    }

    fn sampled_body(mode: Mode, padding: usize) -> (Vec<f32>, Vec<f32>) {
        sampled_image_body(mode, source_image(mode), padding)
    }

    /// Samples between the end of the raster and the end of the stream, which a
    /// live receiver always has because audio keeps arriving.
    const TAIL: usize = 1_024;

    fn sampled_image_body(mode: Mode, image: RgbImage, padding: usize) -> (Vec<f32>, Vec<f32>) {
        let mut frequency = vec![1900.0; padding];
        let mut sync = vec![0.0; padding];
        for tone in TxEncoder::new(mode, image).unwrap().skip(13) {
            let relative_end = tone.until().as_picos() - VIS_END_PS;
            let end_sample = (u128::from(relative_end) * u128::from(SAMPLE_RATE))
                .div_ceil(1_000_000_000_000) as usize;
            let end = padding + end_sample;
            frequency.resize(end, tone.frequency().as_hz() as f32);
            sync.resize(
                end,
                if tone.component() == TxComponent::Sync {
                    1.0
                } else {
                    0.0
                },
            );
        }
        frequency.resize(frequency.len() + TAIL, 1900.0);
        sync.resize(frequency.len(), 0.0);
        (frequency, sync)
    }

    fn mmsstv_martin2_golden(padding: usize, sample_rate_hz: u32) -> (Vec<f32>, Vec<f32>) {
        const PERIOD_PS: u64 = 226_798_000_000;
        const SEGMENTS: [(u64, u64, f32, bool); 4] = [
            (0, 4_862_000_000, 1200.0, true),
            (5_434_000_000, 78_650_000_000, 1900.0, false),
            (79_222_000_000, 152_438_000_000, 2100.0, false),
            (153_010_000_000, 226_226_000_000, 1700.0, false),
        ];
        let raster_ps = PERIOD_PS * 256;
        let samples = (u128::from(raster_ps) * u128::from(sample_rate_hz))
            .div_ceil(1_000_000_000_000) as usize;
        let mut frequency = vec![1500.0; padding + samples + TAIL];
        let mut sync = vec![0.0; frequency.len()];
        for unit in 0..256_u64 {
            for (start_ps, end_ps, value, is_sync) in SEGMENTS {
                let edge = |offset_ps| {
                    padding
                        + (u128::from(unit * PERIOD_PS + offset_ps) * u128::from(sample_rate_hz))
                            .div_ceil(1_000_000_000_000) as usize
                };
                let range = edge(start_ps)..edge(end_ps);
                frequency[range.clone()].fill(value);
                if is_sync {
                    sync[range].fill(1.0);
                }
            }
        }
        (frequency, sync)
    }

    fn decode(
        mode: Mode,
        frequency: &[f32],
        sync: &[f32],
        chunks: &[usize],
    ) -> (RgbImage, Vec<usize>, u64) {
        let absolute_start = 40_000;
        let mut decoder = RxDecoder::new(mode, SAMPLE_RATE).unwrap();
        let mut offset = 0;
        let mut chunk = 0;
        let mut rows = Vec::new();
        let mut epoch = None;
        while decoder.state() != RxState::Complete {
            let count = if offset == frequency.len() {
                0
            } else {
                chunks[chunk % chunks.len()].min(frequency.len() - offset)
            };
            chunk += 1;
            let result = decoder
                .process(DemodulatedBlock::new(
                    absolute_start + offset as u64,
                    &frequency[offset..offset + count],
                    &sync[offset..offset + count],
                ))
                .unwrap();
            offset += result.consumed();
            match result.event() {
                Some(RxEvent::RasterAcquired { source_epoch, .. }) => epoch = Some(source_epoch),
                Some(RxEvent::RowDecoded { row }) => rows.push(row),
                Some(_) => {}
                None if result.consumed() == 0 => panic!(
                    "decoder made no progress: mode={mode:?} state={:?} offset={offset} len={}",
                    decoder.state(),
                    frequency.len()
                ),
                None => {}
            }
        }
        assert!(offset <= frequency.len());
        let image = match decoder.finish() {
            RxOutcome::Complete(image) => image,
            RxOutcome::Incomplete { .. } => panic!("decoder did not complete"),
            RxOutcome::Stopped { .. } => panic!("decoder stopped"),
        };
        (image, rows, epoch.unwrap())
    }

    #[rstest]
    #[case(Mode::Martin2)]
    #[case(Mode::Scottie2)]
    #[case(Mode::Robot36)]
    #[case(Mode::Robot72)]
    #[case(Mode::Pd50)]
    fn synthetic_tx_body_decodes_each_family(#[case] mode: Mode) {
        let padding = 937;
        let (frequency, sync) = sampled_body(mode, padding);
        let (image, rows, epoch) = decode(mode, &frequency, &sync, &[17, 4093, 1, 701]);
        assert_eq!(
            rows,
            (0..mode.spec().active_rows() as usize).collect::<Vec<_>>()
        );
        assert_eq!(image.size().width(), mode.spec().width() as usize);
        assert_eq!(image.size().height(), mode.spec().height() as usize);
        let expected_epoch = 40_000 + padding as u64 + if mode == Mode::Scottie2 { 90 } else { 0 };
        assert!(epoch.abs_diff(expected_epoch) <= 1);
        assert_ne!(image.get(0, 0), Some(Rgb8::default()));
        if mode.spec().active_rows() < mode.spec().height() {
            assert_eq!(
                image.get(0, mode.spec().active_rows() as usize),
                Some(Rgb8::default())
            );
        }
    }

    #[rstest]
    #[case(Mode::Martin1, false)]
    #[case(Mode::Martin2, false)]
    #[case(Mode::Scottie2, false)]
    #[case(Mode::Robot72, true)]
    #[case(Mode::Pd50, true)]
    #[case(Mode::Robot36, true)]
    fn non_uniform_image_survives_a_transmit_receive_round_trip(
        #[case] mode: Mode,
        #[case] subsampled: bool,
    ) {
        let source = band_image(mode);
        let (frequency, sync) = sampled_image_body(mode, source.clone(), 512);
        let (image, _, _) = decode(mode, &frequency, &sync, &[usize::MAX]);
        let width = source.size().width();
        for row in 0..mode.spec().active_rows() as usize {
            // The first raster unit of an alternating mode has no retained
            // chrominance yet, exactly as in the reference decoder.
            if mode.spec().raster_organization() == RasterOrganization::AlternatingYCrCb
                && row % 2 == 0
            {
                continue;
            }
            // Component edges are left out because the acquired sample rate
            // still carries a few parts per million of fitting error.
            for x in [width / 4, width / 2, 3 * width / 4] {
                let expected = source.get(x, row).unwrap();
                let expected = if subsampled {
                    y_cr_cb_to_rgb(crate::color::rgb_to_y_cr_cb(expected))
                } else {
                    expected
                };
                assert_eq!(image.get(x, row), Some(expected), "{mode:?} ({x}, {row})");
            }
        }
    }

    #[rstest]
    #[case(Mode::Martin1, 2)]
    #[case(Mode::Martin2, 1)]
    fn pixel_window_averages_only_what_a_pixel_can_hold(#[case] mode: Mode, #[case] expected: u64) {
        let decoder = RxDecoder::new(mode, SAMPLE_RATE).unwrap();
        let clock = RasterClock::from_estimate(0.0, f64::from(SAMPLE_RATE)).unwrap();
        let segment = decoder.segment(ScanChannel::Green, 0).unwrap();
        let (first, end) = decoder.pixel_window(clock, 0, segment, 10).unwrap();
        assert_eq!(end - first, expected);
    }

    #[test]
    fn arbitrary_block_splits_do_not_change_output() {
        let (frequency, sync) = sampled_body(Mode::Martin2, 311);
        let contiguous = decode(Mode::Martin2, &frequency, &sync, &[usize::MAX]);
        let fragmented = decode(Mode::Martin2, &frequency, &sync, &[1, 2, 31, 997, 7]);
        assert_eq!(contiguous, fragmented);
    }

    #[test]
    fn mmsstv_source_golden_raster_decodes_without_the_tx_encoder() {
        let sample_rate_hz = 48_000;
        let (frequency, sync) = mmsstv_martin2_golden(503, sample_rate_hz);
        let mut decoder = RxDecoder::new(Mode::Martin2, sample_rate_hz).unwrap();
        let mut offset = 0;
        let mut rows = Vec::new();
        while decoder.state() != RxState::Complete {
            let result = decoder
                .process(DemodulatedBlock::new(
                    offset as u64,
                    &frequency[offset..],
                    &sync[offset..],
                ))
                .unwrap();
            offset += result.consumed();
            if let Some(RxEvent::RowDecoded { row }) = result.event() {
                rows.push(row);
            }
        }
        let image = decoder.image();
        assert_eq!(rows, (0..256).collect::<Vec<_>>());
        let expected = Rgb8::new(64, 128, 192);
        assert_eq!(
            image
                .pixels()
                .iter()
                .position(|pixel| *pixel != expected)
                .map(|index| (index, image.pixels()[index])),
            None
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn arbitrary_valid_blocks_never_panic_or_write_inactive_rows(
            frequency in prop::collection::vec(0.0_f32..=3_000.0, 0..8_000),
            sync in prop::collection::vec(0.0_f32..=1.0, 0..8_000),
            chunks in prop::collection::vec(1_usize..512, 1..32),
        ) {
            let len = frequency.len().min(sync.len());
            let frequency = &frequency[..len];
            let sync = &sync[..len];
            let mut decoder = RxDecoder::new(Mode::Robot36, 1_000).unwrap();
            let mut offset = 0;
            let mut chunk = 0;
            let mut steps = 0;
            while offset < len && steps < len.saturating_mul(2).saturating_add(64) {
                let end = (offset + chunks[chunk % chunks.len()]).min(len);
                let result = decoder.process(DemodulatedBlock::new(
                    offset as u64,
                    &frequency[offset..end],
                    &sync[offset..end],
                ));
                match result {
                    Ok(result) => {
                        offset += result.consumed();
                        if result.consumed() == 0 && result.event().is_none() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
                if matches!(decoder.state(), RxState::Complete | RxState::Stopped { .. }) {
                    break;
                }
                chunk += 1;
                steps += 1;
            }
            for row in Mode::Robot36.spec().active_rows() as usize..decoder.image().size().height() {
                prop_assert!(decoder.image().row(row).unwrap().iter().all(|pixel| *pixel == Rgb8::default()));
            }
        }
    }

    #[rstest]
    #[case(Mode::Martin1, 146_432_000_000)]
    #[case(Mode::Martin2, 73_216_000_000)]
    #[case(Mode::Scottie1, 138_240_000_000)]
    #[case(Mode::Scottie2, 88_064_000_000)]
    #[case(Mode::ScottieDx, 345_600_000_000)]
    #[case(Mode::Robot36, 44_000_000_000)]
    #[case(Mode::Robot72, 69_000_000_000)]
    #[case(Mode::Pd50, 91_520_000_000)]
    #[case(Mode::Pd90, 170_240_000_000)]
    #[case(Mode::Pd120, 121_600_000_000)]
    #[case(Mode::Pd160, 195_584_000_000)]
    #[case(Mode::Pd180, 183_040_000_000)]
    #[case(Mode::Pd240, 244_480_000_000)]
    #[case(Mode::Pd290, 228_800_000_000)]
    fn all_supported_profiles_construct_with_exact_component_time(
        #[case] mode: Mode,
        #[case] component_ps: u64,
    ) {
        let decoder = RxDecoder::new(mode, SAMPLE_RATE).unwrap();
        assert_eq!(
            decoder
                .profile
                .pixels()
                .last()
                .map(|segment| segment.duration_ps),
            Some(component_ps)
        );
        assert_eq!(decoder.image().size().width(), mode.spec().width() as usize);
        assert_eq!(
            decoder.image().size().height(),
            mode.spec().height() as usize
        );
    }

    #[test]
    fn support_and_sample_rate_errors_are_typed() {
        assert!(matches!(
            RxDecoder::new(Mode::Avt90, SAMPLE_RATE),
            Err(SstvError::UnsupportedRxMode(Mode::Avt90))
        ));
        assert!(matches!(
            RxDecoder::new(Mode::Martin1, 0),
            Err(SstvError::InvalidSampleRate)
        ));
    }

    #[test]
    fn malformed_blocks_and_gaps_are_rejected() {
        let mut decoder = RxDecoder::new(Mode::Martin1, SAMPLE_RATE).unwrap();
        assert_eq!(
            decoder
                .process(DemodulatedBlock::new(0, &[1500.0], &[]))
                .unwrap_err()
                .error(),
            SstvError::DemodulatedLengthMismatch
        );
        assert_eq!(
            decoder
                .process(DemodulatedBlock::new(0, &[f32::NAN], &[0.0]))
                .unwrap_err()
                .error(),
            SstvError::InvalidDemodulatedSample { offset: 0 }
        );
        assert_eq!(
            decoder
                .process(DemodulatedBlock::new(100, &[1500.0], &[0.0]))
                .unwrap()
                .consumed(),
            1
        );
        assert_eq!(
            decoder
                .process(DemodulatedBlock::new(102, &[1500.0], &[0.0]))
                .unwrap_err()
                .error(),
            SstvError::DemodulatedGap {
                expected: 101,
                actual: 102
            }
        );
    }

    #[test]
    fn unsuccessful_acquisition_window_is_consumed_without_error() {
        let decoder = RxDecoder::new(Mode::Martin2, SAMPLE_RATE).unwrap();
        let count = window_samples(decoder.profile, SAMPLE_RATE) as usize;
        let frequency = vec![1500.0; count];
        let sync = vec![0.0; count];
        let mut decoder = decoder;
        let result = decoder
            .process(DemodulatedBlock::new(0, &frequency, &sync))
            .unwrap();
        assert_eq!(result.consumed(), count);
        assert_eq!(result.event(), None);
        assert_eq!(decoder.state(), RxState::Acquiring);
        assert!(decoder.input.as_ref().unwrap().len() < count);
    }

    #[test]
    fn process_errors_report_the_consumed_prefix() {
        let mut decoder = RxDecoder::new(Mode::Martin2, SAMPLE_RATE).unwrap();
        let count = window_samples(decoder.profile, SAMPLE_RATE) as usize;
        let mut frequency = vec![1500.0; count + 1];
        let sync = vec![0.0; count + 1];
        frequency[count] = f32::NAN;
        let error = decoder
            .process(DemodulatedBlock::new(0, &frequency, &sync))
            .unwrap_err();
        assert_eq!(error.consumed(), count);
        assert_eq!(
            error.error(),
            SstvError::InvalidDemodulatedSample { offset: count }
        );
    }

    #[test]
    fn acquisition_recovers_after_more_than_two_periods_of_noise_and_chunked_retry() {
        let profile = RasterProfile::for_mode(Mode::Martin2).unwrap();
        let noise = (u128::from(profile.period_ps) * u128::from(SAMPLE_RATE) * 3
            / 1_000_000_000_000) as usize;
        let (frequency, sync) = sampled_body(Mode::Martin2, noise);
        let (_, rows, epoch) = decode(Mode::Martin2, &frequency, &sync, &[113, 997, 29]);
        assert_eq!(rows.len(), Mode::Martin2.spec().active_rows() as usize);
        assert!(
            epoch.abs_diff(40_000 + noise as u64) <= 6,
            "epoch={epoch} expected={}",
            40_000 + noise as u64
        );
    }

    fn drive_configured(
        mode: Mode,
        frequency: &[f32],
        sync: &[f32],
        config: RxConfig,
    ) -> (RxDecoder, Vec<RxEvent>) {
        let absolute_start = 70_000;
        let mut decoder = RxDecoder::with_config(mode, SAMPLE_RATE, config).unwrap();
        let mut offset = 0;
        let mut events = Vec::new();
        while !matches!(decoder.state(), RxState::Complete | RxState::Stopped { .. }) {
            let result = decoder
                .process(DemodulatedBlock::new(
                    absolute_start + offset as u64,
                    &frequency[offset..],
                    &sync[offset..],
                ))
                .unwrap();
            offset += result.consumed();
            if let Some(event) = result.event() {
                events.push(event);
            }
            assert!(result.consumed() != 0 || result.event().is_some());
        }
        (decoder, events)
    }

    fn shift_sync_runs(sync: &[f32], shift_for_run: impl Fn(usize) -> Option<usize>) -> Vec<f32> {
        let mut shifted = vec![0.0; sync.len()];
        let mut index = 0;
        let mut run = 0;
        while index < sync.len() {
            if sync[index] == 0.0 {
                index += 1;
                continue;
            }
            let start = index;
            while index < sync.len() && sync[index] != 0.0 {
                index += 1;
            }
            if let Some(displacement) = shift_for_run(run) {
                let destination = start + displacement;
                let count = (index - start).min(sync.len().saturating_sub(destination));
                shifted[destination..destination + count]
                    .copy_from_slice(&sync[start..start + count]);
            }
            run += 1;
        }
        shifted
    }

    #[test]
    fn staging_disabled_retains_no_full_stream_and_preserves_seam() {
        let (frequency, sync) = sampled_body(Mode::Martin2, 311);
        let (decoder, _) = drive_configured(Mode::Martin2, &frequency, &sync, RxConfig::default());
        assert_eq!(decoder.staged_samples_len(), 0);
        assert!(decoder.input.as_ref().unwrap().len() < frequency.len() / 10);
        assert_eq!(decoder.state(), RxState::Complete);
    }

    #[test]
    fn staging_overflow_is_typed_and_does_not_grow() {
        let mut decoder = RxDecoder::with_config(
            Mode::Martin2,
            SAMPLE_RATE,
            RxConfig {
                staging: Staging::Memory { max_samples: 2 },
                ..RxConfig::default()
            },
        )
        .unwrap();
        assert_eq!(
            decoder
                .process(DemodulatedBlock::new(
                    0,
                    &[1900.0, 1900.0, 1900.0],
                    &[0.0, 0.0, 0.0]
                ))
                .unwrap_err()
                .error(),
            SstvError::StagingCapacityExceeded { max_samples: 2 }
        );
        assert_eq!(decoder.staged_samples_len(), 0);
    }

    #[test]
    fn completed_decoder_accepts_a_contiguous_refinement_tail() {
        let (frequency, sync) = sampled_body(Mode::Martin2, 311);
        let (mut decoder, _) = drive_configured(
            Mode::Martin2,
            &frequency,
            &sync,
            RxConfig {
                staging: Staging::Memory {
                    max_samples: frequency.len() + 2,
                },
                ..RxConfig::default()
            },
        );
        let first = decoder.next_sample.unwrap();
        let staged = decoder.staged_samples_len();
        decoder
            .stage_for_refinement(DemodulatedBlock::new(first, &[1900.0, 1900.0], &[0.0, 0.0]))
            .unwrap();
        assert_eq!(decoder.staged_samples_len(), staged + 2);

        let mut acquiring = RxDecoder::new(Mode::Martin2, SAMPLE_RATE).unwrap();
        assert_eq!(
            acquiring.stage_for_refinement(DemodulatedBlock::new(0, &[], &[])),
            Err(SstvError::RxNotComplete)
        );
    }

    #[test]
    fn live_phase_correction_is_stable_and_held_off() {
        let (mut frequency, sync) = sampled_body(Mode::Martin2, 311);
        let mut shifted = shift_sync_runs(&sync, |run| Some(if run < 6 { 0 } else { 4 }));
        frequency.resize(frequency.len() + 64, 1900.0);
        shifted.resize(frequency.len(), 0.0);
        let (decoder, events) = drive_configured(
            Mode::Martin2,
            &frequency,
            &shifted,
            RxConfig {
                live_sync: true,
                ..RxConfig::default()
            },
        );
        let adjustments: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                RxEvent::PhaseAdjusted { unit, .. } => Some(*unit),
                _ => None,
            })
            .collect();
        assert!(!adjustments.is_empty());
        assert!(adjustments.windows(2).all(|units| units[1] - units[0] >= 6));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, RxEvent::RowDecoded { .. }))
                .count(),
            Mode::Martin2.spec().active_rows() as usize
        );
        assert_eq!(decoder.sync_observations().len(), 16);
    }

    #[test]
    fn auto_stop_is_a_terminal_event_and_outcome() {
        let (frequency, sync) = sampled_body(Mode::Martin2, 311);
        let missing = shift_sync_runs(&sync, |run| (run < 6).then_some(0));
        let (decoder, events) = drive_configured(
            Mode::Martin2,
            &frequency,
            &missing,
            RxConfig {
                auto_stop: true,
                ..RxConfig::default()
            },
        );
        assert!(matches!(decoder.state(), RxState::Stopped { .. }));
        assert!(matches!(events.last(), Some(RxEvent::Stopped { .. })));
        assert!(matches!(decoder.finish(), RxOutcome::Stopped { .. }));
    }

    #[test]
    fn auto_stop_score_leaks_instead_of_resetting_after_one_good_line() {
        let mut decoder = RxDecoder::new(Mode::Martin2, SAMPLE_RATE).unwrap();
        for _ in 0..5 {
            decoder.note_bad_sync().unwrap();
        }
        assert_eq!(decoder.bad_sync_score, 10);
        decoder.note_good_sync();
        assert_eq!(decoder.bad_sync_score, 9);
        decoder.note_bad_sync().unwrap();
        assert_eq!(decoder.bad_sync_score, 11);
    }

    #[rstest]
    #[case(Mode::Martin2)]
    #[case(Mode::Robot36)]
    #[case(Mode::Pd50)]
    fn staged_rebuild_is_deterministic_and_resets_family_state(#[case] mode: Mode) {
        let (frequency, sync) = sampled_body(mode, 311);
        let (mut decoder, _) = drive_configured(
            mode,
            &frequency,
            &sync,
            RxConfig {
                staging: Staging::Memory {
                    max_samples: frequency.len(),
                },
                ..RxConfig::default()
            },
        );
        let staged_len = decoder.staged_samples_len();
        let first = decoder.refine_staged().unwrap();
        let first_image = decoder.image().clone();
        assert_eq!(decoder.poll_event(), None);
        assert_eq!(decoder.staged_samples_len(), staged_len);
        let second = decoder.refine_staged().unwrap();
        assert_eq!(first.slant, second.slant);
        assert_eq!(first_image, *decoder.image());
        assert_eq!(second.revision, 2);
    }

    #[test]
    fn partial_staged_rebuild_is_typed_and_preserves_image_and_state() {
        let (frequency, sync) = sampled_body(Mode::Martin2, 311);
        let limit = frequency.len();
        let mut decoder = RxDecoder::with_config(
            Mode::Martin2,
            SAMPLE_RATE,
            RxConfig {
                staging: Staging::Memory { max_samples: limit },
                ..RxConfig::default()
            },
        )
        .unwrap();
        let partial = frequency.len() / 8;
        let mut offset = 0;
        while offset < partial {
            let result = decoder
                .process(DemodulatedBlock::new(
                    offset as u64,
                    &frequency[offset..partial],
                    &sync[offset..partial],
                ))
                .unwrap();
            offset += result.consumed();
            if result.consumed() == 0 && result.event().is_none() {
                break;
            }
        }
        assert!(decoder.staged_observations.len() >= 6);
        let image = decoder.decode.image.clone();
        let state = decoder.decode.state;
        let raster_unit = decoder.decode.raster_unit;
        let revision = decoder.image_revision;
        assert!(matches!(
            decoder.refine_staged(),
            Err(SstvError::InsufficientStagedData { .. })
        ));
        assert_eq!(decoder.decode.image, image);
        assert_eq!(decoder.decode.state, state);
        assert_eq!(decoder.decode.raster_unit, raster_unit);
        assert_eq!(decoder.image_revision, revision);
    }

    #[test]
    fn robot36_tcs_classification_overrides_parity_and_initial_selector() {
        let (mut frequency, sync) = sampled_body(Mode::Robot36, 0);
        fill_robot36_selector(&mut frequency, 0..1, 2300.0);
        fill_robot36_selector(&mut frequency, 1..2, 1500.0);
        let mut decoder = RxDecoder::new(Mode::Robot36, SAMPLE_RATE).unwrap();
        let block = DemodulatedBlock::new(0, &frequency, &sync);
        let mut input = SampleBuffer::new(0);
        input.append(block, frequency.len());
        decoder.input = Some(input);
        decoder.decode.clock = Some(RasterClock::from_estimate(0.0, SAMPLE_RATE as f64).unwrap());
        assert_eq!(
            decoder.robot_selector_at(0).unwrap(),
            Some(RobotSelector::Cb)
        );
        assert_eq!(
            decoder.robot_selector_at(1).unwrap(),
            Some(RobotSelector::Cr)
        );
        let (_, rows, _) = decode(Mode::Robot36, &frequency, &sync, &[37, 1009, 3]);
        assert_eq!(
            rows,
            (0..Mode::Robot36.spec().active_rows() as usize).collect::<Vec<_>>()
        );
    }

    fn fill_robot36_selector(frequency: &mut [f32], units: impl Iterator<Item = u64>, value: f32) {
        let profile = RasterProfile::for_mode(Mode::Robot36).unwrap();
        let (start_ps, end_ps) = profile.selector_window_ps().unwrap();
        for unit in units {
            let unit_start = u128::from(profile.period_ps) * u128::from(unit);
            let sample = |offset_ps: u64| {
                ((unit_start + u128::from(offset_ps)) * u128::from(SAMPLE_RATE) / 1_000_000_000_000)
                    as usize
            };
            let (start, end) = (sample(start_ps), sample(end_ps));
            if end <= frequency.len() {
                frequency[start..end].fill(value);
            }
        }
    }

    #[test]
    fn robot36_delivers_every_active_row_when_the_selector_never_alternates() {
        let (mut frequency, sync) = sampled_body(Mode::Robot36, 311);
        fill_robot36_selector(&mut frequency, 0..240, 1500.0);
        let (image, rows, _) = decode(Mode::Robot36, &frequency, &sync, &[71, 4093, 13]);
        assert_eq!(
            rows,
            (0..Mode::Robot36.spec().active_rows() as usize).collect::<Vec<_>>()
        );
        assert_eq!(
            image.get(0, Mode::Robot36.spec().active_rows() as usize),
            Some(Rgb8::default())
        );
    }

    #[test]
    fn frequency_inverse_matches_transmit_integer_mapping() {
        let decoder = RxDecoder::new(Mode::Martin1, SAMPLE_RATE).unwrap();
        let band = Mode::Martin1.spec().signal_band();
        for level in 0..=255_u8 {
            let frequency = f64::from(band.level_to_frequency(level).as_hz());
            assert_eq!(decoder.frequency_to_level(frequency), level);
        }
    }
}
