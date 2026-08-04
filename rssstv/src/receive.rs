use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use jiff::Zoned;
use rssstv_audio::CaptureReader;
use rssstv_demodulator::{DemodulatedChunk, Demodulator, SyncStart, sync_detector_delay};
use rssstv_sstv::{
    RxDecoder, SstvError,
    image::RgbImage,
    mode::Mode,
    rx::{DemodulatedBlock, RxConfig, RxEvent, RxState, Staging, StopReason},
};

/// Samples drained from the capture queue per pass.
const READ_SAMPLES: usize = 4_096;

/// Idle wait when the device has produced nothing yet.
const IDLE_POLL: Duration = Duration::from_millis(2);

/// Shortest interval between published image frames.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// Fraction of the previous level retained when the signal falls.
const RELEASE: f32 = 0.88;

const STAGING_SECONDS: usize = 300;

/// Trailing audio staged between staged-refinement attempts, in milliseconds.
///
/// A refit raster reaches a little past the samples decoded so far, so early
/// attempts fail until enough tail has arrived. Retrying on a sample budget
/// rather than on every block bounds the cost of a failed attempt, while a
/// short budget keeps the corrected image from appearing late.
const REFINEMENT_RETRY_MS: usize = 250;

/// Trailing audio staged before staged refinement is abandoned.
const REFINEMENT_TAIL_SECONDS: usize = 15;

/// Longest a reception may stall before the worker stops it.
///
/// The window has to outlast startup acquisition, which fixes the raster phase
/// over five periods and reports no progress while it does. That is a little
/// over five seconds for the slowest mode, so this leaves ample room rather
/// than racing acquisition on a signal that is arriving perfectly well.
const STALL_TIMEOUT: Duration = Duration::from_secs(20);

const FSK_HISTORY_WAIT: Duration = Duration::from_secs(4);

fn live_rx_config(mode: Mode, sample_rate_hz: u32, slant: bool) -> RxConfig {
    RxConfig {
        live_sync: true,
        live_slant: slant,
        auto_stop: true,
        sync_detector_delay: sync_detector_delay(mode),
        staging: if slant {
            Staging::Memory {
                max_samples: (sample_rate_hz as usize).saturating_mul(STAGING_SECONDS),
            }
        } else {
            Staging::Disabled
        },
    }
}

/// Decoded image published to the interface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryCandidate {
    pub mode: Mode,
    pub frame: Frame,
    pub received_at: String,
    pub fsk_ids: Vec<String>,
}

impl Frame {
    fn from_image(image: &RgbImage) -> Self {
        let size = image.size();
        let mut rgba = Vec::with_capacity(size.pixel_count() * 4);
        for pixel in image.pixels() {
            rgba.extend_from_slice(&[pixel.r, pixel.g, pixel.b, u8::MAX]);
        }
        Self {
            width: size.width() as u32,
            height: size.height() as u32,
            rgba,
        }
    }
}

/// Progress of the current reception.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Progress {
    /// No SSTV signal has been identified.
    #[default]
    Idle,
    /// A mode was detected and raster timing is being acquired.
    Acquiring,
    /// Rows are being decoded.
    Decoding { rows: usize, total: usize },
    /// Every row was decoded.
    Complete,
    /// Synchronization was lost and decoding stopped.
    Stopped,
}

impl Progress {
    /// Returns the decoded fraction of the raster in `0.0..=1.0`.
    pub fn fraction(self) -> f32 {
        match self {
            Self::Idle | Self::Acquiring => 0.0,
            Self::Decoding { rows, total } if total > 0 => {
                (rows as f32 / total as f32).clamp(0.0, 1.0)
            }
            Self::Decoding { .. } => 0.0,
            Self::Complete | Self::Stopped => 1.0,
        }
    }

    /// Returns whether a reception is currently in progress.
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Acquiring | Self::Decoding { .. })
    }
}

/// One observation of the receive pipeline.
///
/// Each snapshot fully describes the current state, so the interface may
/// discard all but the newest.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub mode: Option<Mode>,
    pub progress: Progress,
    pub display_fraction: f32,
    pub level: f32,
    pub frame: Option<Frame>,
    pub history: Option<HistoryCandidate>,
    pub callsigns: Vec<String>,
    pub dropped_samples: u64,
    pub error: Option<String>,
}

/// Single-slot handoff holding the newest observation.
///
/// The interface only ever uses the newest snapshot, so the worker overwrites
/// the slot instead of queueing. This bounds the handoff by construction: an
/// interface that stops polling cannot make the worker accumulate frames.
#[derive(Debug, Default)]
struct Mailbox {
    slot: Mutex<Option<Snapshot>>,
}

impl Mailbox {
    /// Replaces the pending snapshot, keeping payloads not yet collected.
    fn publish(&self, mut snapshot: Snapshot) {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(previous) = slot.take() {
            if snapshot.frame.is_none() {
                snapshot.frame = previous.frame;
            }
            if snapshot.error.is_none() {
                snapshot.error = previous.error;
            }
            if snapshot.history.is_none() {
                snapshot.history = previous.history;
            }
        }
        *slot = Some(snapshot);
    }

    fn take(&self) -> Option<Snapshot> {
        self.slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

/// Handle to a running receive worker.
///
/// Dropping the handle stops the worker and waits for it to finish, so the
/// capture queue is never left with a live consumer.
pub struct Worker {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    mailbox: Arc<Mailbox>,
    slant: Arc<AtomicBool>,
    sync_start: Arc<Mutex<SyncStart>>,
}

impl Worker {
    /// Starts decoding everything `reader` produces.
    pub fn spawn(reader: CaptureReader, slant: bool, sync_start: SyncStart) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let slant = Arc::new(AtomicBool::new(slant));
        let sync_start = Arc::new(Mutex::new(sync_start));
        let mailbox = Arc::new(Mailbox::default());
        let join = {
            let stop = Arc::clone(&stop);
            let slant = Arc::clone(&slant);
            let sync_start = Arc::clone(&sync_start);
            let mailbox = Arc::clone(&mailbox);
            thread::Builder::new()
                .name("rssstv-receive".to_owned())
                .spawn(move || run(reader, &mailbox, &stop, &slant, &sync_start))
                .ok()
        };
        Self {
            stop,
            join,
            mailbox,
            slant,
            sync_start,
        }
    }

    pub fn set_slant(&self, enabled: bool) {
        self.slant.store(enabled, Ordering::Relaxed);
    }

    pub fn set_sync_start(&self, scope: SyncStart) {
        *self
            .sync_start
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = scope;
    }

    /// Returns the newest state, or `None` when nothing changed since the last
    /// call.
    pub fn latest(&self) -> Option<Snapshot> {
        self.mailbox.take()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl core::fmt::Debug for Worker {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("Worker").finish_non_exhaustive()
    }
}

/// State of the staged slant refinement that follows a completed raster.
///
/// Startup acquisition fits the raster rate from only a few periods, so the
/// live clock can be off by thousands of parts per million. Refitting the rate
/// over the whole staged reception is what removes the resulting slant, and it
/// is not optional for a usable image.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Refinement {
    /// Staging is off, or the reception cannot be refined.
    NotApplicable,
    /// Collecting trailing audio until the refit raster is covered.
    Waiting,
    Done,
    Failed(String),
}

/// Owns the protocol state for one worker thread.
struct Session {
    demodulator: Demodulator,
    decoder: Option<RxDecoder>,
    sample_rate_hz: u32,
    published_revision: Option<u64>,
    searching: bool,
    /// Which modes a reception may start in without a VIS header.
    ///
    /// Held here because every demodulator the session builds has to be given
    /// it again: a restarted search is still looking for the same signals.
    sync_start: SyncStart,
    refinement: Refinement,
    /// Staged length at which the next refinement attempt is worthwhile.
    refine_next_len: usize,
    /// Staged length beyond which refinement is abandoned. Zero until the
    /// first trailing block arrives.
    refine_limit_len: usize,
    received_at: Option<String>,
    fsk_ids: Vec<String>,
}

impl Session {
    fn new(sample_rate_hz: u32, sync_start: SyncStart) -> Result<Self, String> {
        let mut demodulator =
            Demodulator::new(sample_rate_hz).map_err(|error| error.to_string())?;
        demodulator.set_sync_start(sync_start);
        Ok(Self {
            demodulator,
            decoder: None,
            sample_rate_hz,
            published_revision: None,
            searching: true,
            sync_start,
            refinement: Refinement::NotApplicable,
            refine_next_len: 0,
            refine_limit_len: 0,
            received_at: None,
            fsk_ids: Vec::new(),
        })
    }

    /// Adopts a new sync-start scope, for this reception and later ones.
    fn set_sync_start(&mut self, scope: SyncStart) {
        if self.sync_start == scope {
            return;
        }
        self.sync_start = scope;
        self.demodulator.set_sync_start(scope);
    }

    /// Discards all protocol state.
    ///
    /// Capture overrun breaks the contiguity the demodulator requires, so the
    /// pipeline restarts rather than decoding across the gap.
    fn reset(&mut self) -> Result<(), String> {
        *self = Self::new(self.sample_rate_hz, self.sync_start)?;
        Ok(())
    }

    fn progress(&self) -> Progress {
        let Some(decoder) = self.decoder.as_ref() else {
            return Progress::Idle;
        };
        let total = decoder.mode().spec().active_rows() as usize;
        match decoder.state() {
            RxState::Acquiring => Progress::Acquiring,
            RxState::Decoding { completed_rows } => Progress::Decoding {
                rows: completed_rows,
                total,
            },
            RxState::Complete => Progress::Complete,
            RxState::Stopped { .. } => Progress::Stopped,
        }
    }

    fn display_fraction(&self) -> Option<f32> {
        let decoder = self.decoder.as_ref()?;
        let total = usize::from(decoder.mode().spec().active_rows());
        let rows = match decoder.state() {
            RxState::Acquiring => 0,
            RxState::Decoding { completed_rows } | RxState::Stopped { completed_rows, .. } => {
                completed_rows
            }
            RxState::Complete => total,
        };
        Some(if total == 0 {
            0.0
        } else {
            (rows as f32 / total as f32).clamp(0.0, 1.0)
        })
    }

    fn interrupt(&mut self) -> Result<Option<(Frame, Option<HistoryCandidate>)>, String> {
        let Some(decoder) = self.decoder.as_mut() else {
            return Ok(None);
        };
        decoder.stop(StopReason::SynchronizationLost);
        let frame = Frame::from_image(decoder.image());
        let history = Self::history_candidate(decoder, &self.received_at, &self.fsk_ids);
        self.reset()?;
        Ok(Some((frame, history)))
    }

    fn history_candidate(
        decoder: &RxDecoder,
        received_at: &Option<String>,
        fsk_ids: &[String],
    ) -> Option<HistoryCandidate> {
        let mode = decoder.mode();
        let completed_rows = match decoder.state() {
            RxState::Decoding { completed_rows } | RxState::Stopped { completed_rows, .. } => {
                completed_rows
            }
            RxState::Complete => usize::from(mode.spec().active_rows()),
            RxState::Acquiring => 0,
        };
        history_eligible(completed_rows, usize::from(mode.spec().active_rows())).then(|| {
            HistoryCandidate {
                mode,
                frame: Frame::from_image(decoder.image()),
                received_at: received_at.clone().unwrap_or_else(receive_time),
                fsk_ids: fsk_ids.to_vec(),
            }
        })
    }

    fn restart_search(&mut self) -> Result<(), String> {
        self.demodulator =
            Demodulator::new(self.sample_rate_hz).map_err(|error| error.to_string())?;
        self.demodulator.set_sync_start(self.sync_start);
        self.searching = true;
        Ok(())
    }

    /// Returns whether the reception is over and refinement has resolved.
    ///
    /// Refinement needs trailing audio, so the search for the next signal must
    /// not restart the demodulator while it is still waiting.
    fn reception_finished(&self) -> bool {
        !self.searching
            && self.refinement != Refinement::Waiting
            && self.decoder.as_ref().is_some_and(|decoder| {
                matches!(decoder.state(), RxState::Complete | RxState::Stopped { .. })
            })
    }

    fn take_refinement_error(&mut self) -> Option<String> {
        let Refinement::Failed(message) = &self.refinement else {
            return None;
        };
        let message = message.clone();
        self.refinement = Refinement::NotApplicable;
        Some(message)
    }

    /// Feeds one demodulated chunk into the raster decoder.
    fn decode(&mut self, chunk: &DemodulatedChunk, slant: bool) -> Result<(), String> {
        if let Some(mode) = chunk.detected_mode() {
            self.decoder = Some(
                RxDecoder::with_config(
                    mode,
                    self.sample_rate_hz,
                    live_rx_config(mode, self.sample_rate_hz, slant),
                )
                .map_err(|error| error.to_string())?,
            );
            self.published_revision = None;
            self.searching = false;
            self.refinement = if slant {
                Refinement::Waiting
            } else {
                Refinement::NotApplicable
            };
            self.refine_next_len = 0;
            self.refine_limit_len = 0;
            self.received_at = Some(receive_time());
            self.fsk_ids.clear();
        }
        if chunk.frequency_hz().is_empty() {
            return Ok(());
        }
        // The decoder is moved out so the refinement bookkeeping below can
        // borrow the session mutably alongside it.
        let Some(mut decoder) = self.decoder.take() else {
            return Ok(());
        };
        let result = self.drive(&mut decoder, chunk);
        self.decoder = Some(decoder);
        result
    }

    fn drive(&mut self, decoder: &mut RxDecoder, chunk: &DemodulatedChunk) -> Result<(), String> {
        let mut offset = 0;
        while offset < chunk.frequency_hz().len() {
            let block = DemodulatedBlock::new(
                chunk.first_sample() + offset as u64,
                &chunk.frequency_hz()[offset..],
                &chunk.sync_strength()[offset..],
            );
            match decoder.state() {
                RxState::Complete => {
                    self.stage_tail(decoder, block);
                    break;
                }
                RxState::Stopped { .. } => {
                    self.refinement = Refinement::NotApplicable;
                    break;
                }
                _ => {}
            }
            let processed = decoder.process(block).map_err(|error| {
                format!(
                    "receive decoding failed at sample {}: {}",
                    chunk.first_sample() + offset as u64 + error.consumed() as u64,
                    error.error()
                )
            })?;
            if processed.event()
                == Some(RxEvent::Stopped {
                    reason: StopReason::SynchronizationLost,
                })
            {
                break;
            }
            offset += processed.consumed();
            if processed.consumed() == 0 && processed.event().is_none() {
                break;
            }
        }
        Ok(())
    }

    /// Retains trailing audio after completion and refits the raster clock.
    ///
    /// A refit raster reaches slightly past the samples decoded live, so the
    /// tail has to be staged before refinement can succeed. This mirrors what
    /// the offline `decode-wav` integration does at end of file.
    fn stage_tail(&mut self, decoder: &mut RxDecoder, block: DemodulatedBlock<'_>) {
        if self.refinement != Refinement::Waiting {
            return;
        }
        if let Err(error) = decoder.stage_for_refinement(block) {
            self.refinement =
                Refinement::Failed(format!("staging the refinement tail failed: {error}"));
            return;
        }
        let staged = decoder.staged_samples_len();
        let rate = self.sample_rate_hz as usize;
        if self.refine_limit_len == 0 {
            self.refine_limit_len = staged.saturating_add(rate * REFINEMENT_TAIL_SECONDS);
            self.refine_next_len = staged.saturating_add(retry_step(rate));
            return;
        }
        if staged < self.refine_next_len {
            return;
        }
        match decoder.refine_staged() {
            Ok(_) => self.refinement = Refinement::Done,
            // The refit raster reaches past the tail collected so far. More
            // audio is still arriving, so wait rather than give up.
            Err(SstvError::InsufficientStagedData { .. }) if staged < self.refine_limit_len => {
                self.refine_next_len = staged.saturating_add(retry_step(rate));
            }
            Err(error) => {
                self.refinement = Refinement::Failed(format!("slant refinement failed: {error}"));
            }
        }
    }

    /// Returns a new frame when the decoder image has changed.
    fn frame(&mut self) -> Option<Frame> {
        let decoder = self.decoder.as_ref()?;
        let revision = decoder.image_revision();
        if self.published_revision == Some(revision) {
            return None;
        }
        self.published_revision = Some(revision);
        Some(Frame::from_image(decoder.image()))
    }
}

fn run(
    mut reader: CaptureReader,
    mailbox: &Mailbox,
    stop: &AtomicBool,
    slant: &AtomicBool,
    sync_start: &Mutex<SyncStart>,
) {
    let sample_rate_hz = reader.sample_rate_hz();
    let initial_sync_start = *sync_start.lock().unwrap_or_else(PoisonError::into_inner);
    let mut session = match Session::new(sample_rate_hz, initial_sync_start) {
        Ok(session) => session,
        Err(error) => {
            mailbox.publish(Snapshot {
                error: Some(error),
                ..Snapshot::default()
            });
            return;
        }
    };
    let mut pcm = vec![0.0_f32; READ_SAMPLES];
    let mut level = 0.0_f32;
    let mut callsigns: Vec<String> = Vec::new();
    let mut last_frame = Instant::now() - FRAME_INTERVAL;
    let mut error = None;
    let mut last_progress = Progress::Idle;
    let mut display_fraction = 0.0;
    let mut history_deadline = None;
    let mut progress_changed_at = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        let reading = reader.read(&mut pcm);
        if reading.count == 0 {
            thread::sleep(IDLE_POLL);
            continue;
        }
        let samples = &pcm[..reading.count];
        level = follow_peak(level, block_peak(samples), RELEASE);
        session.set_sync_start(*sync_start.lock().unwrap_or_else(PoisonError::into_inner));

        if reading.is_discontinuous() && session.reset().is_err() {
            error = Some("failed to restart after a capture overrun".to_owned());
        }

        match session.demodulator.process(samples) {
            Ok(chunk) => {
                if let Err(reason) = session.decode(&chunk, slant.load(Ordering::Relaxed)) {
                    error = Some(reason);
                    let _ = session.reset();
                }
                for id in chunk.fsk_ids() {
                    let text = id.as_str().to_owned();
                    if !callsigns.contains(&text) {
                        callsigns.push(text.clone());
                    }
                    if !session.fsk_ids.contains(&text) {
                        session.fsk_ids.push(text);
                    }
                }
                if error.is_none() {
                    error = session.take_refinement_error();
                }
            }
            Err(reason) => {
                error = Some(reason.to_string());
                let _ = session.reset();
            }
        }

        let mut interrupted = None;
        let mut progress = session.progress();
        if let Some(fraction) = session.display_fraction() {
            display_fraction = fraction;
        }
        if progress == Progress::Stopped {
            match session.interrupt() {
                Ok(result) => interrupted = result,
                Err(reason) => error = Some(reason),
            }
            history_deadline = None;
            progress = session.progress();
            last_progress = progress;
            progress_changed_at = Instant::now();
        } else if progress == last_progress {
            if progress.is_active() && progress_changed_at.elapsed() >= STALL_TIMEOUT {
                match session.interrupt() {
                    Ok(result) => interrupted = result,
                    Err(reason) => error = Some(reason),
                }
                history_deadline = None;
                progress = session.progress();
                last_progress = progress;
                progress_changed_at = Instant::now();
            }
        } else {
            last_progress = progress;
            progress_changed_at = Instant::now();
        }
        let reception_finished = session.reception_finished();
        let (interrupted_frame, mut history) = interrupted
            .map(|(frame, history)| (Some(frame), history))
            .unwrap_or_default();
        let history_ready = if reception_finished {
            let deadline =
                *history_deadline.get_or_insert_with(|| Instant::now() + FSK_HISTORY_WAIT);
            !session.fsk_ids.is_empty() || Instant::now() >= deadline
        } else {
            false
        };
        if history.is_none() && history_ready {
            history = session.decoder.as_ref().and_then(|decoder| {
                Session::history_candidate(decoder, &session.received_at, &session.fsk_ids)
            });
        }
        let frame = interrupted_frame.or_else(|| {
            (reception_finished || last_frame.elapsed() >= FRAME_INTERVAL)
                .then(|| session.frame())
                .flatten()
        });
        if frame.is_some() {
            last_frame = Instant::now();
        }
        let snapshot = Snapshot {
            mode: session.decoder.as_ref().map(RxDecoder::mode),
            progress,
            display_fraction,
            level,
            frame,
            history,
            callsigns: callsigns.clone(),
            dropped_samples: reader.dropped_samples(),
            error: error.take(),
        };
        mailbox.publish(snapshot);
        if history_ready {
            history_deadline = None;
            if let Err(reason) = session.restart_search() {
                error = Some(reason);
            }
        }
    }
}

fn history_eligible(completed_rows: usize, total_rows: usize) -> bool {
    total_rows > 0 && completed_rows.saturating_mul(100) >= total_rows.saturating_mul(65)
}

fn receive_time() -> String {
    Zoned::now().strftime("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

/// Samples of trailing audio between staged-refinement attempts.
fn retry_step(sample_rate_hz: usize) -> usize {
    (sample_rate_hz * REFINEMENT_RETRY_MS / 1_000).max(1)
}

fn block_peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
}

/// Tracks `peak` instantly upward and decays toward it by `release`.
fn follow_peak(current: f32, peak: f32, release: f32) -> f32 {
    if peak >= current {
        peak.clamp(0.0, 1.0)
    } else {
        (current * release).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn peak_uses_absolute_amplitude() {
        assert_eq!(block_peak(&[0.1, -0.8, 0.3]), 0.8);
        assert_eq!(block_peak(&[]), 0.0);
    }

    #[test]
    fn rising_signals_are_followed_immediately() {
        assert_eq!(follow_peak(0.2, 0.9, RELEASE), 0.9);
    }

    #[test]
    fn falling_signals_decay_gradually() {
        assert_eq!(follow_peak(0.8, 0.0, 0.5), 0.4);
    }

    #[rstest]
    #[case(2.0, 0.0)]
    #[case(0.5, 3.0)]
    fn meter_values_stay_normalized(#[case] current: f32, #[case] peak: f32) {
        let level = follow_peak(current, peak, RELEASE);
        assert!((0.0..=1.0).contains(&level), "{level} is out of range");
    }

    #[rstest]
    #[case(Progress::Idle, 0.0)]
    #[case(Progress::Acquiring, 0.0)]
    #[case(Progress::Decoding { rows: 62, total: 124 }, 0.5)]
    #[case(Progress::Decoding { rows: 0, total: 0 }, 0.0)]
    #[case(Progress::Complete, 1.0)]
    #[case(Progress::Stopped, 1.0)]
    fn progress_maps_to_a_decoded_fraction(#[case] progress: Progress, #[case] expected: f32) {
        assert_eq!(progress.fraction(), expected);
    }

    #[test]
    fn only_live_receptions_are_active() {
        assert!(Progress::Acquiring.is_active());
        assert!(Progress::Decoding { rows: 1, total: 2 }.is_active());
        assert!(!Progress::Idle.is_active());
        assert!(!Progress::Complete.is_active());
        assert!(!Progress::Stopped.is_active());
    }

    #[rstest]
    #[case(false)]
    #[case(true)]
    fn live_receptions_enable_automatic_stop(#[case] slant: bool) {
        assert!(live_rx_config(Mode::Robot36, 8_000, slant).auto_stop);
    }

    #[test]
    fn frames_carry_opaque_rgba_pixels() {
        use rssstv_sstv::image::{ImageSize, Rgb8};

        let size = ImageSize::new(2, 1).unwrap();
        let image = RgbImage::new(size, Rgb8::new(10, 20, 30));
        let frame = Frame::from_image(&image);
        assert_eq!(frame.width, 2);
        assert_eq!(frame.height, 1);
        assert_eq!(frame.rgba, vec![10, 20, 30, 255, 10, 20, 30, 255]);
    }

    #[rstest]
    #[case(155, 240, false)]
    #[case(156, 240, true)]
    #[case(240, 240, true)]
    #[case(0, 0, false)]
    fn history_starts_at_sixty_five_percent(
        #[case] completed_rows: usize,
        #[case] total_rows: usize,
        #[case] expected: bool,
    ) {
        assert_eq!(history_eligible(completed_rows, total_rows), expected);
    }

    #[test]
    fn overwriting_the_mailbox_preserves_uncollected_payloads() {
        let mailbox = Mailbox::default();
        let frame = Frame {
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 255],
        };
        mailbox.publish(Snapshot {
            frame: Some(frame.clone()),
            error: Some("capture gap".to_owned()),
            ..Snapshot::default()
        });
        mailbox.publish(Snapshot {
            progress: Progress::Decoding { rows: 3, total: 10 },
            ..Snapshot::default()
        });

        let snapshot = mailbox.take().unwrap();
        assert_eq!(snapshot.progress, Progress::Decoding { rows: 3, total: 10 });
        assert_eq!(snapshot.frame, Some(frame));
        assert_eq!(snapshot.error.as_deref(), Some("capture gap"));
    }

    #[test]
    fn overwriting_the_mailbox_preserves_uncollected_history() {
        let mailbox = Mailbox::default();
        let history = HistoryCandidate {
            mode: Mode::Robot36,
            frame: Frame {
                width: 1,
                height: 1,
                rgba: vec![1, 2, 3, 255],
            },
            received_at: "2026-08-04T12:34:56+09:00".to_owned(),
            fsk_ids: vec!["JA1ABC".to_owned()],
        };
        mailbox.publish(Snapshot {
            history: Some(history.clone()),
            ..Snapshot::default()
        });
        mailbox.publish(Snapshot::default());
        assert_eq!(mailbox.take().unwrap().history, Some(history));
    }

    #[test]
    fn a_collected_mailbox_reports_nothing_until_it_is_written_again() {
        let mailbox = Mailbox::default();
        mailbox.publish(Snapshot::default());
        assert!(mailbox.take().is_some());
        assert!(mailbox.take().is_none());
    }

    #[test]
    fn newer_payloads_replace_uncollected_ones() {
        let mailbox = Mailbox::default();
        mailbox.publish(Snapshot {
            error: Some("first".to_owned()),
            ..Snapshot::default()
        });
        mailbox.publish(Snapshot {
            error: Some("second".to_owned()),
            ..Snapshot::default()
        });
        assert_eq!(mailbox.take().unwrap().error.as_deref(), Some("second"));
    }
}

#[cfg(test)]
mod pipeline_tests {
    use std::f64::consts::TAU;

    use rssstv_audio::synthetic_capture;
    use rssstv_fskid::FskId;
    use rssstv_sstv::{
        TransmissionEncoder, TxEncoder,
        image::{ImageSize, Rgb8, RgbImage},
    };

    use super::*;

    const RATE: u32 = 8_000;

    /// Smooth ramps keep chroma subsampling out of the measurement while still
    /// making a mistimed raster obvious: any slant shears the gradient.
    fn source_image(mode: Mode) -> RgbImage {
        let width = mode.spec().width() as usize;
        let height = mode.spec().height() as usize;
        let size = ImageSize::new(width, height).unwrap();
        let pixels = (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| {
                    Rgb8::new(
                        (x * 255 / width) as u8,
                        (y * 255 / height) as u8,
                        ((x * 255 / width) + (y * 255 / height)) as u8 / 2,
                    )
                })
            })
            .collect();
        RgbImage::from_pixels(size, pixels).unwrap()
    }

    /// Renders one transmission whose clock runs `offset_ppm` away from the
    /// receiver's, which is what puts slant into a real reception.
    fn transmission(mode: Mode, image: RgbImage, offset_ppm: f64) -> Vec<f32> {
        let transmit_rate = f64::from(RATE) * (1.0 + offset_ppm / 1.0e6);
        let mut pcm = Vec::new();
        let mut phase = 0.0_f64;
        for tone in TxEncoder::new(mode, image).unwrap() {
            let deadline =
                (tone.until().as_picos() as f64 * transmit_rate / 1.0e12).round() as usize;
            while pcm.len() < deadline {
                pcm.push((phase.sin() * 0.8) as f32);
                phase = (phase + TAU * f64::from(tone.frequency().as_hz()) / transmit_rate)
                    .rem_euclid(TAU);
            }
        }
        pcm
    }

    fn complete_transmission(mode: Mode, image: RgbImage, callsign: &str) -> Vec<f32> {
        let mut pcm = Vec::new();
        let mut phase = 0.0_f64;
        for tone in TransmissionEncoder::new(mode, image, FskId::new(callsign).unwrap()).unwrap() {
            let deadline =
                (tone.until().as_picos() as f64 * f64::from(RATE) / 1.0e12).round() as usize;
            while pcm.len() < deadline {
                pcm.push((phase.sin() * 0.8) as f32);
                phase = (phase + TAU * f64::from(tone.frequency().as_hz()) / f64::from(RATE))
                    .rem_euclid(TAU);
            }
        }
        pcm
    }

    fn mean_abs_error(decoded: &Frame, expected: &RgbImage) -> f64 {
        let mut total = 0_u64;
        for (pixel, chunk) in expected.pixels().iter().zip(decoded.rgba.chunks_exact(4)) {
            total += u64::from(pixel.r.abs_diff(chunk[0]));
            total += u64::from(pixel.g.abs_diff(chunk[1]));
            total += u64::from(pixel.b.abs_diff(chunk[2]));
        }
        total as f64 / (expected.size().pixel_count() * 3) as f64
    }

    /// Drives the real worker through the capture queue and returns its last
    /// image, so the test covers the same code path a live device does.
    fn receive(pcm: &[f32], trailing_silence: usize) -> (Snapshot, Option<Frame>) {
        receive_with(pcm, trailing_silence, SyncStart::Disabled)
    }

    fn receive_with(
        pcm: &[f32],
        trailing_silence: usize,
        sync_start: SyncStart,
    ) -> (Snapshot, Option<Frame>) {
        let (mut feed, reader) = synthetic_capture(RATE, 1 << 16).unwrap();
        let worker = Worker::spawn(reader, true, sync_start);
        let mut snapshot = Snapshot::default();
        let mut frame = None;
        let silence = vec![0.0_f32; trailing_silence];

        for source in [pcm, silence.as_slice()] {
            let mut offset = 0;
            while offset < source.len() {
                let room = feed.vacant().min(source.len() - offset);
                if room == 0 {
                    thread::sleep(Duration::from_millis(1));
                } else {
                    offset += feed.push(&source[offset..offset + room]);
                }
                collect(&worker, &mut snapshot, &mut frame);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        while feed.vacant() < (1 << 16) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
            collect(&worker, &mut snapshot, &mut frame);
        }
        thread::sleep(Duration::from_millis(100));
        collect(&worker, &mut snapshot, &mut frame);
        (snapshot, frame)
    }

    fn collect(worker: &Worker, snapshot: &mut Snapshot, frame: &mut Option<Frame>) {
        if let Some(mut latest) = worker.latest() {
            if let Some(new) = latest.frame.take() {
                *frame = Some(new);
            }
            if latest.history.is_none() {
                latest.history = snapshot.history.take();
            }
            *snapshot = latest;
        }
    }

    #[test]
    fn completed_history_carries_the_reception_fskid() {
        let mode = Mode::Robot36;
        let pcm = complete_transmission(mode, source_image(mode), "JA1ABC");

        let (snapshot, _) = receive(&pcm, RATE as usize);

        let history = snapshot.history.expect("a history candidate");
        assert_eq!(history.mode, mode);
        assert_eq!(history.fsk_ids, ["JA1ABC"]);
        assert!(history.received_at.contains('T'));
    }

    #[test]
    fn automatic_stop_keeps_the_partial_frame_visible_while_waiting() {
        let mode = Mode::Robot36;
        let pcm = transmission(mode, source_image(mode), 0.0);
        let truncated = &pcm[..pcm.len() * 3 / 4];

        let (snapshot, frame) = receive(truncated, RATE as usize * 3);

        assert_eq!(snapshot.progress, Progress::Idle, "{snapshot:?}");
        assert!(frame.is_some());
        assert!(
            (0.65..1.0).contains(&snapshot.display_fraction),
            "{snapshot:?}"
        );
    }

    fn decode_at(mode: Mode, expected: &RgbImage, offset_ppm: f64) -> (Snapshot, f64) {
        let pcm = transmission(mode, expected.clone(), offset_ppm);
        let (snapshot, frame) = receive(&pcm, RATE as usize * 3);
        let frame = frame.expect("a decoded frame");
        assert_eq!(frame.width, u32::from(mode.spec().width()));
        let error = mean_abs_error(&frame, expected);
        (snapshot, error)
    }

    /// A transmitter whose clock is off produces a slanted raster unless the
    /// worker refits the clock from the staged reception. Refinement needs
    /// audio that arrives after the raster completes, so this only passes when
    /// the worker keeps staging its tail.
    ///
    /// The mistimed reception is judged against a matched reception rather
    /// than an absolute threshold, so the assertion measures slant correction
    /// and not codec fidelity.
    #[test]
    fn a_mistimed_transmission_decodes_like_a_matched_one() {
        let mode = Mode::Robot36;
        let expected = source_image(mode);
        let (matched, baseline) = decode_at(mode, &expected, 0.0);
        assert_eq!(matched.progress, Progress::Complete, "{matched:?}");
        assert!(
            baseline < 40.0,
            "matched reception is already poor: {baseline}"
        );

        let (mistimed, error) = decode_at(mode, &expected, 300.0);
        assert_eq!(mistimed.progress, Progress::Complete, "{mistimed:?}");
        assert_eq!(mistimed.mode, Some(mode));
        assert_eq!(mistimed.error, None);
        assert!(
            error < baseline + 4.0,
            "mistimed reception scored {error} against a {baseline} baseline"
        );
    }

    /// Tuning in part way through a picture leaves no VIS header to identify
    /// the mode, so nothing is decoded unless the raster's own sync spacing is
    /// enough to start on. The rows that arrived before the mode was known
    /// stay blank, which is what the operator sees in MMSSTV too.
    #[test]
    fn a_transmission_joined_after_its_header_still_decodes() {
        let mode = Mode::Martin1;
        let expected = source_image(mode);
        let pcm = transmission(mode, expected.clone(), 0.0);
        // Drop the VOX, VIS, and the first quarter of the raster.
        let joined = &pcm[pcm.len() / 4..];

        let (without, _) = receive(joined, RATE as usize * 3);
        assert_eq!(
            without.mode, None,
            "a headerless signal must not start while sync start is off"
        );

        let (snapshot, frame) = receive_with(joined, RATE as usize * 3, SyncStart::Any);

        assert_eq!(snapshot.mode, Some(mode), "{snapshot:?}");
        let frame = frame.expect("a decoded frame");
        assert_eq!(frame.width, u32::from(mode.spec().width()));

        let (offset, error) = best_vertical_alignment(&frame, &expected);
        assert!(
            error < 40.0,
            "the joined reception scored {error} at its best alignment of {offset} rows"
        );
        assert!(
            offset > 0,
            "the reception was cut into, so it cannot have started on row zero"
        );
    }

    /// Returns the vertical alignment that best matches `expected`, and its
    /// mean error.
    ///
    /// A reception joined part way through has to be judged this way: nothing
    /// in the signal says which row it started on, so the picture is decoded
    /// correctly but rolled. MMSSTV leaves the operator to shift it too.
    ///
    /// Only the lower half of the frame is compared, which is clear of the
    /// blank rows left where the picture arrived before the mode was known.
    fn best_vertical_alignment(frame: &Frame, expected: &RgbImage) -> (usize, f64) {
        let width = expected.size().width();
        let height = expected.size().height();
        let first = height / 2;
        let mut best = (0, f64::INFINITY);
        for offset in 0..first {
            let mut total = 0_u64;
            for y in first..height - offset {
                for x in 0..width {
                    let pixel = expected.get(x, y + offset).unwrap();
                    let chunk = &frame.rgba[(y * width + x) * 4..][..3];
                    total += u64::from(pixel.r.abs_diff(chunk[0]));
                    total += u64::from(pixel.g.abs_diff(chunk[1]));
                    total += u64::from(pixel.b.abs_diff(chunk[2]));
                }
            }
            let rows = height - offset - first;
            let error = total as f64 / (rows * width * 3) as f64;
            if error < best.1 {
                best = (offset, error);
            }
        }
        best
    }
}
