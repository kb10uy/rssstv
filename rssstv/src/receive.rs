use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rssstv_audio::CaptureReader;
use rssstv_demodulator::{DemodulatedChunk, Demodulator, sync_detector_delay};
use rssstv_sstv::image::RgbImage;
use rssstv_sstv::mode::Mode;
use rssstv_sstv::rx::{DemodulatedBlock, RxConfig, RxEvent, RxState, Staging};
use rssstv_sstv::{RxDecoder, SstvError};

/// Samples drained from the capture queue per pass.
const READ_SAMPLES: usize = 4_096;

/// Idle wait when the device has produced nothing yet.
const IDLE_POLL: Duration = Duration::from_millis(2);

/// Shortest interval between published image frames.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// Fraction of the previous level retained when the signal falls.
const RELEASE: f32 = 0.88;

const STAGING_SECONDS: usize = 300;

/// Trailing audio staged between staged-refinement attempts.
///
/// A refit raster usually reaches a little past the samples decoded so far, so
/// the first attempts fail until enough tail has arrived. Retrying on a sample
/// budget rather than every block keeps the cost of a failed attempt bounded.
const REFINEMENT_RETRY_SECONDS: usize = 1;

/// Trailing audio staged before staged refinement is abandoned.
const REFINEMENT_TAIL_SECONDS: usize = 15;

/// Longest a reception may stall before the worker searches for a new signal.
///
/// `auto_stop` is left off because its live scoring aborts real receptions
/// early, so a signal that simply disappears is caught by this timeout instead.
const STALL_TIMEOUT: Duration = Duration::from_secs(20);

/// Decoded image published to the interface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
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
    pub level: f32,
    pub sync_strength: f32,
    pub frame: Option<Frame>,
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
}

impl Worker {
    /// Starts decoding everything `reader` produces.
    pub fn spawn(reader: CaptureReader, slant: bool) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let slant = Arc::new(AtomicBool::new(slant));
        let mailbox = Arc::new(Mailbox::default());
        let join = {
            let stop = Arc::clone(&stop);
            let slant = Arc::clone(&slant);
            let mailbox = Arc::clone(&mailbox);
            thread::Builder::new()
                .name("rssstv-receive".to_owned())
                .spawn(move || run(reader, &mailbox, &stop, &slant))
                .ok()
        };
        Self {
            stop,
            join,
            mailbox,
            slant,
        }
    }

    pub fn set_slant(&self, enabled: bool) {
        self.slant.store(enabled, Ordering::Relaxed);
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
    refinement: Refinement,
    /// Staged length at which the next refinement attempt is worthwhile.
    refine_next_len: usize,
    /// Staged length beyond which refinement is abandoned. Zero until the
    /// first trailing block arrives.
    refine_limit_len: usize,
}

impl Session {
    fn new(sample_rate_hz: u32) -> Result<Self, String> {
        Ok(Self {
            demodulator: Demodulator::new(sample_rate_hz).map_err(|error| error.to_string())?,
            decoder: None,
            sample_rate_hz,
            published_revision: None,
            searching: true,
            refinement: Refinement::NotApplicable,
            refine_next_len: 0,
            refine_limit_len: 0,
        })
    }

    /// Discards all protocol state.
    ///
    /// Capture overrun breaks the contiguity the demodulator requires, so the
    /// pipeline restarts rather than decoding across the gap.
    fn reset(&mut self) -> Result<(), String> {
        *self = Self::new(self.sample_rate_hz)?;
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

    fn restart_search(&mut self) -> Result<(), String> {
        self.demodulator =
            Demodulator::new(self.sample_rate_hz).map_err(|error| error.to_string())?;
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
                    RxConfig {
                        live_sync: true,
                        fit_initial_rate: slant,
                        // Live scoring aborts real receptions early, so the
                        // worker's stall timeout ends dead signals instead.
                        auto_stop: false,
                        sync_detector_delay: sync_detector_delay(mode),
                        staging: if slant {
                            Staging::Memory {
                                max_samples: (self.sample_rate_hz as usize)
                                    .saturating_mul(STAGING_SECONDS),
                            }
                        } else {
                            Staging::Disabled
                        },
                    },
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
                    reason: rssstv_sstv::rx::StopReason::SynchronizationLost,
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
            self.refine_next_len = staged.saturating_add(rate * REFINEMENT_RETRY_SECONDS);
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
                self.refine_next_len = staged.saturating_add(rate * REFINEMENT_RETRY_SECONDS);
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

fn run(mut reader: CaptureReader, mailbox: &Mailbox, stop: &AtomicBool, slant: &AtomicBool) {
    let sample_rate_hz = reader.sample_rate_hz();
    let mut session = match Session::new(sample_rate_hz) {
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
    let mut sync_strength = 0.0_f32;
    let mut callsigns: Vec<String> = Vec::new();
    let mut last_frame = Instant::now() - FRAME_INTERVAL;
    let mut error = None;
    let mut last_progress = Progress::Idle;
    let mut progress_changed_at = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        let reading = reader.read(&mut pcm);
        if reading.count == 0 {
            thread::sleep(IDLE_POLL);
            continue;
        }
        let samples = &pcm[..reading.count];
        level = follow_peak(level, block_peak(samples), RELEASE);

        if reading.is_discontinuous() && session.reset().is_err() {
            error = Some("failed to restart after a capture overrun".to_owned());
        }

        match session.demodulator.process(samples) {
            Ok(chunk) => {
                for id in chunk.fsk_ids() {
                    let text = id.as_str().to_owned();
                    if !callsigns.contains(&text) {
                        callsigns.push(text);
                    }
                }
                sync_strength =
                    follow_peak(sync_strength, block_peak(chunk.sync_strength()), RELEASE);
                if let Err(reason) = session.decode(&chunk, slant.load(Ordering::Relaxed)) {
                    error = Some(reason);
                    let _ = session.reset();
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

        let mut progress = session.progress();
        if progress == last_progress {
            if progress.is_active() && progress_changed_at.elapsed() >= STALL_TIMEOUT {
                error = Some("reception stalled; searching for a new signal".to_owned());
                let _ = session.reset();
                progress = session.progress();
                progress_changed_at = Instant::now();
            }
        } else {
            last_progress = progress;
            progress_changed_at = Instant::now();
        }
        let reception_finished = session.reception_finished();
        let frame = (reception_finished || last_frame.elapsed() >= FRAME_INTERVAL)
            .then(|| session.frame())
            .flatten();
        if frame.is_some() {
            last_frame = Instant::now();
        }
        let snapshot = Snapshot {
            mode: session.decoder.as_ref().map(RxDecoder::mode),
            progress,
            level,
            sync_strength,
            frame,
            callsigns: callsigns.clone(),
            dropped_samples: reader.dropped_samples(),
            error: error.take(),
        };
        mailbox.publish(snapshot);
        if reception_finished && let Err(reason) = session.restart_search() {
            error = Some(reason);
        }
    }
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
