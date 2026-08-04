use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use rssstv_audio::PlaybackWriter;
use rssstv_fskid::FskId;
use rssstv_modulator::Modulator;
use rssstv_sstv::{TransmissionEncoder, image::RgbImage, mode::Mode};

const PCM_BLOCK_SIZE: usize = 1_024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TxPhase {
    #[default]
    Idle,
    Priming,
    Producing,
    Draining,
    Complete,
    Cancelled,
    Failed,
}

impl TxPhase {
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Priming | Self::Producing | Self::Draining)
    }
}

/// Where the image raster sits inside the transmission, in output samples.
///
/// The interface reports progress by row, so it needs the window the rows
/// occupy rather than the length of the audio around them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RasterTiming {
    pub start_samples: u64,
    pub samples: u64,
    pub rows: usize,
}

impl RasterTiming {
    /// Maps an audible sample position onto the row being transmitted.
    ///
    /// A position before the window is still in the leader and one past it is
    /// in the station identification; neither carries a row.
    pub fn progress_at(self, played_samples: u64) -> TxProgress {
        if self.rows == 0 || self.samples == 0 || played_samples < self.start_samples {
            return TxProgress::Leader;
        }
        let elapsed = played_samples - self.start_samples;
        if elapsed >= self.samples {
            return TxProgress::Identifying;
        }
        let rows = u128::from(elapsed) * self.rows as u128 / u128::from(self.samples);
        TxProgress::Scanning {
            rows: rows as usize,
            total: self.rows,
        }
    }
}

/// How far the audible transmission has advanced through the image raster.
///
/// The leader and the station identifier frame the raster and carry no rows,
/// so they are named rather than folded into a fraction that would sit pinned
/// at either end.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TxProgress {
    /// Nothing is being transmitted.
    #[default]
    Idle,
    /// The VOX sequence, VIS framing, or leading segments are being sent.
    Leader,
    /// Image rows are being sent.
    Scanning { rows: usize, total: usize },
    /// The raster is finished; the footer and station identifier are being sent.
    Identifying,
    /// The whole transmission was sent.
    Complete,
}

impl TxProgress {
    /// Returns the transmitted fraction of the raster in `0.0..=1.0`.
    pub fn fraction(self) -> f32 {
        match self {
            Self::Idle | Self::Leader => 0.0,
            Self::Scanning { rows, total } if total > 0 => {
                (rows as f32 / total as f32).clamp(0.0, 1.0)
            }
            Self::Scanning { .. } => 0.0,
            Self::Identifying | Self::Complete => 1.0,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TxSnapshot {
    pub phase: TxPhase,
    pub generated_samples: u64,
    pub total_samples: u64,
    pub raster: RasterTiming,
    pub error: Option<String>,
}

pub struct TxWorker {
    snapshot: Arc<Mutex<TxSnapshot>>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl TxWorker {
    pub fn spawn(
        writer: PlaybackWriter,
        mode: Mode,
        frame: Arc<RgbImage>,
        station_id: Option<FskId>,
    ) -> Self {
        let snapshot = Arc::new(Mutex::new(TxSnapshot {
            phase: TxPhase::Priming,
            ..TxSnapshot::default()
        }));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_snapshot = Arc::clone(&snapshot);
        let worker_cancel = Arc::clone(&cancel);
        let thread = thread::spawn(move || {
            transmit_loop(
                writer,
                mode,
                frame,
                station_id,
                worker_snapshot,
                worker_cancel,
            )
        });
        Self {
            snapshot,
            cancel,
            thread: Some(thread),
        }
    }

    pub fn latest(&self) -> TxSnapshot {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_else(|_| TxSnapshot {
                phase: TxPhase::Failed,
                error: Some("transmit worker state is unavailable".to_owned()),
                ..TxSnapshot::default()
            })
    }
}

impl Drop for TxWorker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl core::fmt::Debug for TxWorker {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TxWorker")
            .field("snapshot", &self.latest())
            .finish_non_exhaustive()
    }
}

fn transmit_loop(
    mut writer: PlaybackWriter,
    mode: Mode,
    frame: Arc<RgbImage>,
    station_id: Option<FskId>,
    snapshot: Arc<Mutex<TxSnapshot>>,
    cancel: Arc<AtomicBool>,
) {
    let transmission = match station_id.map_or_else(
        || TransmissionEncoder::without_identifier(mode, (*frame).clone()),
        |station_id| TransmissionEncoder::new(mode, (*frame).clone(), station_id),
    ) {
        Ok(transmission) => transmission,
        Err(error) => return fail(&snapshot, error.to_string()),
    };
    let sample_rate_hz = writer.sample_rate_hz();
    let total_samples = samples_for(transmission.duration().as_picos(), sample_rate_hz);
    let raster = RasterTiming {
        start_samples: samples_for(transmission.raster_start().as_picos(), sample_rate_hz),
        samples: samples_for(transmission.raster_duration().as_picos(), sample_rate_hz),
        rows: usize::from(mode.spec().active_rows()),
    };
    update(&snapshot, |state| {
        state.total_samples = total_samples;
        state.raster = raster;
    });
    let mut modulator = match Modulator::new(transmission, sample_rate_hz) {
        Ok(modulator) => modulator,
        Err(error) => return fail(&snapshot, error.to_string()),
    };
    let prefill = writer
        .vacant()
        .min((sample_rate_hz as usize / 4).max(1))
        .min(total_samples as usize);
    let mut block = [0.0_f32; PCM_BLOCK_SIZE];
    let mut count = 0;
    let mut offset = 0;
    let mut primed = false;
    loop {
        if cancel.load(Ordering::Acquire) || writer.is_cancelled() {
            update(&snapshot, |state| state.phase = TxPhase::Cancelled);
            return;
        }
        if offset == count {
            match modulator.process(&mut block) {
                Ok(0) => {
                    writer.finish();
                    update(&snapshot, |state| state.phase = TxPhase::Draining);
                    return;
                }
                Ok(next) => {
                    count = next;
                    offset = 0;
                }
                Err(error) => return fail(&snapshot, error.to_string()),
            }
        }
        let written = writer.write(&block[offset..count]);
        if written == 0 {
            thread::sleep(Duration::from_millis(2));
            continue;
        }
        offset += written;
        update(&snapshot, |state| {
            state.generated_samples += written as u64;
            if !primed && state.generated_samples >= prefill as u64 {
                state.phase = TxPhase::Producing;
                primed = true;
            }
        });
    }
}

fn samples_for(duration_picos: u64, sample_rate_hz: u32) -> u64 {
    let scaled = u128::from(duration_picos) * u128::from(sample_rate_hz);
    u64::try_from(scaled.div_ceil(1_000_000_000_000)).expect("SSTV transmission fits u64")
}

fn update(snapshot: &Mutex<TxSnapshot>, update: impl FnOnce(&mut TxSnapshot)) {
    if let Ok(mut state) = snapshot.lock() {
        update(&mut state);
    }
}

fn fail(snapshot: &Mutex<TxSnapshot>, error: String) {
    update(snapshot, |state| {
        state.phase = TxPhase::Failed;
        state.error = Some(error);
    });
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use rssstv_audio::synthetic_playback;
    use rssstv_sstv::image::{ImageSize, Rgb8};
    use rstest::rstest;

    use super::*;

    /// The window a mode's rows occupy is narrower than the transmission, so
    /// the same played position means different things at either edge.
    #[rstest]
    #[case(0, TxProgress::Leader)]
    #[case(99, TxProgress::Leader)]
    #[case(100, TxProgress::Scanning { rows: 0, total: 40 })]
    #[case(105, TxProgress::Scanning { rows: 10, total: 40 })]
    #[case(119, TxProgress::Scanning { rows: 38, total: 40 })]
    #[case(120, TxProgress::Identifying)]
    #[case(400, TxProgress::Identifying)]
    fn played_samples_map_onto_the_row_being_transmitted(
        #[case] played_samples: u64,
        #[case] expected: TxProgress,
    ) {
        let raster = RasterTiming {
            start_samples: 100,
            samples: 20,
            rows: 40,
        };
        assert_eq!(raster.progress_at(played_samples), expected);
    }

    #[test]
    fn an_unknown_raster_window_reports_the_leader_rather_than_a_row() {
        assert_eq!(
            RasterTiming::default().progress_at(1_000),
            TxProgress::Leader
        );
    }

    #[rstest]
    #[case(TxProgress::Leader, 0.0)]
    #[case(TxProgress::Scanning { rows: 62, total: 124 }, 0.5)]
    #[case(TxProgress::Scanning { rows: 0, total: 0 }, 0.0)]
    #[case(TxProgress::Identifying, 1.0)]
    #[case(TxProgress::Complete, 1.0)]
    fn progress_maps_to_a_transmitted_fraction(
        #[case] progress: TxProgress,
        #[case] expected: f32,
    ) {
        assert_eq!(progress.fraction(), expected);
    }

    #[test]
    fn transmit_worker_streams_a_complete_bounded_transmission() {
        let mode = Mode::Robot36;
        let size = ImageSize::new(mode.spec().width().into(), mode.spec().height().into()).unwrap();
        let frame = Arc::new(RgbImage::new(size, Rgb8::new(80, 140, 200)));
        let (writer, mut reader) = synthetic_playback(8_000, 512).unwrap();
        let worker = TxWorker::spawn(writer, mode, frame, Some(FskId::new("N0CALL").unwrap()));
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut received = 0_u64;
        let mut block = [0.0_f32; 512];
        loop {
            let snapshot = worker.latest();
            if snapshot.phase == TxPhase::Producing || snapshot.phase == TxPhase::Draining {
                received += reader.read(&mut block) as u64;
            }
            if snapshot.phase == TxPhase::Draining && reader.is_complete() {
                assert_eq!(received, snapshot.total_samples);
                assert_eq!(snapshot.generated_samples, snapshot.total_samples);
                let raster = snapshot.raster;
                assert_eq!(raster.rows, usize::from(mode.spec().active_rows()));
                // The leader precedes the window and the identifier follows it.
                assert!(raster.start_samples > 0);
                assert!(raster.start_samples + raster.samples < snapshot.total_samples);
                break;
            }
            assert!(snapshot.phase != TxPhase::Failed, "{:?}", snapshot.error);
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
    }
}
