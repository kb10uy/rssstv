use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rssstv_audio::CaptureReader;
use rssstv_demodulator::{DemodulatedChunk, Demodulator, sync_detector_delay};
use rssstv_sstv::RxDecoder;
use rssstv_sstv::image::RgbImage;
use rssstv_sstv::mode::Mode;
use rssstv_sstv::rx::{DemodulatedBlock, RxConfig, RxEvent, RxState, Staging};

/// Samples drained from the capture queue per pass.
const READ_SAMPLES: usize = 4_096;

/// Idle wait when the device has produced nothing yet.
const IDLE_POLL: Duration = Duration::from_millis(2);

/// Shortest interval between published image frames.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// Fraction of the previous level retained when the signal falls.
const RELEASE: f32 = 0.88;

const STAGING_SECONDS: usize = 300;

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

/// Handle to a running receive worker.
///
/// Dropping the handle stops the worker and waits for it to finish, so the
/// capture queue is never left with a live consumer.
pub struct Worker {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    snapshots: Receiver<Snapshot>,
    slant: Arc<AtomicBool>,
}

impl Worker {
    /// Starts decoding everything `reader` produces.
    pub fn spawn(reader: CaptureReader, slant: bool) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let slant = Arc::new(AtomicBool::new(slant));
        let (sender, snapshots) = channel();
        let join = {
            let stop = Arc::clone(&stop);
            let slant = Arc::clone(&slant);
            thread::Builder::new()
                .name("rssstv-receive".to_owned())
                .spawn(move || run(reader, &sender, &stop, &slant))
                .ok()
        };
        Self {
            stop,
            join,
            snapshots,
            slant,
        }
    }

    pub fn set_slant(&self, enabled: bool) {
        self.slant.store(enabled, Ordering::Relaxed);
    }

    /// Returns the newest state while preserving transient queued payloads.
    pub fn latest(&self) -> Option<Snapshot> {
        let mut newest = None;
        let mut frame = None;
        let mut error = None;
        loop {
            match self.snapshots.try_recv() {
                Ok(mut snapshot) => {
                    if snapshot.frame.is_some() {
                        frame = snapshot.frame.take();
                    }
                    if snapshot.error.is_some() {
                        error = snapshot.error.take();
                    }
                    newest = Some(snapshot);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => {
                    return newest.map(|mut snapshot| {
                        snapshot.frame = frame;
                        snapshot.error = error;
                        snapshot
                    });
                }
            }
        }
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

/// Owns the protocol state for one worker thread.
struct Session {
    demodulator: Demodulator,
    decoder: Option<RxDecoder>,
    sample_rate_hz: u32,
    published_revision: Option<u64>,
    searching: bool,
    decoder_slant: bool,
    refinement_attempted: bool,
    refinement_error: Option<String>,
}

impl Session {
    fn new(sample_rate_hz: u32) -> Result<Self, String> {
        Ok(Self {
            demodulator: Demodulator::new(sample_rate_hz).map_err(|error| error.to_string())?,
            decoder: None,
            sample_rate_hz,
            published_revision: None,
            searching: true,
            decoder_slant: false,
            refinement_attempted: false,
            refinement_error: None,
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

    fn reception_finished(&self) -> bool {
        !self.searching
            && self.decoder.as_ref().is_some_and(|decoder| {
                matches!(decoder.state(), RxState::Complete | RxState::Stopped { .. })
            })
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
                        auto_stop: true,
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
            self.decoder_slant = slant;
            self.refinement_attempted = false;
            self.refinement_error = None;
        }
        if chunk.frequency_hz().is_empty() {
            return Ok(());
        }
        let Some(decoder) = self.decoder.as_mut() else {
            return Ok(());
        };
        let mut offset = 0;
        while offset < chunk.frequency_hz().len() {
            let block = DemodulatedBlock::new(
                chunk.first_sample() + offset as u64,
                &chunk.frequency_hz()[offset..],
                &chunk.sync_strength()[offset..],
            );
            match decoder.state() {
                RxState::Complete | RxState::Stopped { .. } => {
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
        if decoder.state() == RxState::Complete
            && self.decoder_slant
            && slant
            && !self.refinement_attempted
        {
            self.refinement_attempted = true;
            if let Err(error) = decoder.refine_staged() {
                self.refinement_error = Some(format!("slant refinement failed: {error}"));
            }
        }
        Ok(())
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
    sender: &Sender<Snapshot>,
    stop: &AtomicBool,
    slant: &AtomicBool,
) {
    let sample_rate_hz = reader.sample_rate_hz();
    let mut session = match Session::new(sample_rate_hz) {
        Ok(session) => session,
        Err(error) => {
            let _ = sender.send(Snapshot {
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
                    error = session.refinement_error.take();
                }
            }
            Err(reason) => {
                error = Some(reason.to_string());
                let _ = session.reset();
            }
        }

        let progress = session.progress();
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
        if sender.send(snapshot).is_err() {
            return;
        }
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
    use std::sync::mpsc::channel;

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
    fn snapshot_coalescing_preserves_the_latest_frame_and_error() {
        let (sender, snapshots) = channel();
        let worker = Worker {
            stop: Arc::new(AtomicBool::new(false)),
            join: None,
            snapshots,
            slant: Arc::new(AtomicBool::new(true)),
        };
        let frame = Frame {
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 255],
        };
        sender
            .send(Snapshot {
                frame: Some(frame.clone()),
                error: Some("capture gap".to_owned()),
                ..Snapshot::default()
            })
            .unwrap();
        sender
            .send(Snapshot {
                progress: Progress::Decoding { rows: 3, total: 10 },
                ..Snapshot::default()
            })
            .unwrap();

        let snapshot = worker.latest().unwrap();
        assert_eq!(snapshot.progress, Progress::Decoding { rows: 3, total: 10 });
        assert_eq!(snapshot.frame, Some(frame));
        assert_eq!(snapshot.error.as_deref(), Some("capture gap"));
    }
}
