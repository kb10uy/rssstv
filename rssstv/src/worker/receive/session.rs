use std::{
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use jiff::Zoned;
use rssstv_audio::CaptureReader;
use rssstv_demodulator::{DemodulatedChunk, Demodulator, SyncStart, sync_detector_delay};
use rssstv_sstv::{
    RxDecoder, SstvError,
    mode::Mode,
    rx::{DemodulatedBlock, RxConfig, RxEvent, RxState, Staging, StopReason},
};

use crate::worker::receive::{Frame, HistoryCandidate, Mailbox, Progress, Snapshot};

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
    /// The reception a new header started over, waiting to be published.
    superseded: Option<HistoryCandidate>,
}

impl Session {
    fn new(sample_rate_hz: u32, sync_start: SyncStart) -> Result<Self, String> {
        let mut demodulator =
            Demodulator::new(sample_rate_hz).map_err(|error| error.to_string())?;
        demodulator.set_sync_start(sync_start);
        // A live receiver has to hear a station starting over, which it cannot
        // do if the header that starts the new picture is spent on the one
        // being abandoned.
        demodulator.set_header_restart(true);
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
            superseded: None,
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
            // A header during a reception is the station sending again, so the
            // picture it interrupts is closed out before the decoder is handed
            // over. It is kept on the same terms as any other reception: only
            // one far enough along to be worth looking at.
            if let Some(previous) = self.decoder.as_ref() {
                self.superseded =
                    Self::history_candidate(previous, &self.received_at, &self.fsk_ids);
            }
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

pub(super) fn run(
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

        // A reception the new header started over is published straight away:
        // the identifier it was waiting for cannot arrive now that the station
        // is sending again.
        let superseded = session.superseded.take();
        if superseded.is_some() {
            history_deadline = None;
            last_progress = Progress::Idle;
            progress_changed_at = Instant::now();
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
        let (interrupted_frame, interrupted_history) = interrupted
            .map(|(frame, history)| (Some(frame), history))
            .unwrap_or_default();
        let mut history = superseded.or(interrupted_history);
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
    #[case(false)]
    #[case(true)]
    fn live_receptions_enable_automatic_stop(#[case] slant: bool) {
        assert!(live_rx_config(Mode::Robot36, 8_000, slant).auto_stop);
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
}
