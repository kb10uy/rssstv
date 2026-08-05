use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use rssstv_audio::PlaybackWriter;
use rssstv_fskid::{FskId, FskNumber};
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

/// Output gain applied to generated PCM, shared with the interface.
///
/// Held as bits rather than behind a lock because the interface writes it on
/// every frame while the worker reads it between blocks, and neither should
/// wait for the other over one number.
#[derive(Debug)]
pub struct TxGain(AtomicU32);

impl TxGain {
    /// Builds a gain from how far along its travel the operator's fader is.
    pub fn from_travel(travel: f32) -> Self {
        Self(AtomicU32::new(Self::amplitude(travel).to_bits()))
    }

    /// Moves the fader, which a running transmission picks up per block.
    pub fn set_travel(&self, travel: f32) {
        self.0
            .store(Self::amplitude(travel).to_bits(), Ordering::Relaxed);
    }

    /// Returns the amplitude a fader position stands for.
    ///
    /// Loudness follows amplitude by a power law rather than in step with it,
    /// so a fader that scales amplitude directly spends its upper half on
    /// levels that all sound about the same and crowds everything audible into
    /// the bottom. Squaring the travel spreads that range over the whole bar:
    /// half way down is a quarter of full scale, about 12 dB, which is close to
    /// where a mixing desk's fader sits at its midpoint.
    pub fn amplitude(travel: f32) -> f32 {
        let travel = travel.clamp(0.0, 1.0);
        travel * travel
    }

    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// What a transmission ends with, when it identifies itself at all.
///
/// The number rides on the identifier rather than being announced on its own,
/// so the two travel together or not at all.
#[derive(Clone, Copy, Debug)]
pub struct Identification {
    pub station_id: FskId,
    /// The contest number appended to it, while contest mode is on.
    pub number: Option<FskNumber>,
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
        identification: Option<Identification>,
        gain: Arc<TxGain>,
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
                identification,
                gain,
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
    identification: Option<Identification>,
    gain: Arc<TxGain>,
    snapshot: Arc<Mutex<TxSnapshot>>,
    cancel: Arc<AtomicBool>,
) {
    let transmission = match identification {
        None => TransmissionEncoder::without_identifier(mode, (*frame).clone()),
        Some(Identification {
            station_id,
            number: None,
        }) => TransmissionEncoder::new(mode, (*frame).clone(), station_id),
        Some(Identification {
            station_id,
            number: Some(number),
        }) => TransmissionEncoder::with_contest_number(mode, (*frame).clone(), station_id, number),
    };
    let transmission = match transmission {
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
                    // Applied as the block is produced rather than as it is
                    // written, so a level changed mid-transmission takes
                    // effect on whole blocks and never scales one twice.
                    let level = gain.get();
                    for sample in &mut block[..next] {
                        *sample *= level;
                    }
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

    /// A fader that scaled amplitude directly would spend its top half on
    /// levels that all sound alike, so half travel has to be an audible step
    /// down rather than half as loud.
    #[rstest]
    #[case(-1.0, 0.0)]
    #[case(0.0, 0.0)]
    #[case(0.5, 0.25)]
    #[case(1.0, 1.0)]
    #[case(2.0, 1.0)]
    fn fader_travel_maps_onto_a_perceptual_amplitude(#[case] travel: f32, #[case] expected: f32) {
        assert_eq!(TxGain::amplitude(travel), expected);
    }

    #[test]
    fn a_shared_gain_follows_the_fader() {
        let gain = TxGain::from_travel(1.0);
        assert_eq!(gain.get(), 1.0);

        gain.set_travel(0.5);

        assert_eq!(gain.get(), 0.25);
    }

    /// The level scales what actually leaves for the sound card, rather than
    /// only the bar the operator drags.
    #[test]
    fn the_transmit_level_scales_the_generated_audio() {
        let mode = Mode::Robot36;
        let size = ImageSize::new(mode.spec().width().into(), mode.spec().height().into()).unwrap();
        let frame = Arc::new(RgbImage::new(size, Rgb8::new(80, 140, 200)));
        let (writer, mut reader) = synthetic_playback(8_000, 512).unwrap();
        let gain = Arc::new(TxGain::from_travel(0.0));
        let worker = TxWorker::spawn(writer, mode, frame, None, Arc::clone(&gain));

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut block = [0.0_f32; 512];
        let mut silent = 0;
        while silent < block.len() {
            let snapshot = worker.latest();
            assert!(snapshot.phase != TxPhase::Failed, "{:?}", snapshot.error);
            assert!(Instant::now() < deadline, "no audio was produced");
            let read = reader.read(&mut block);
            assert!(
                block[..read].iter().all(|sample| *sample == 0.0),
                "a silenced transmission still produced audio"
            );
            silent += read;
            thread::yield_now();
        }
        assert_eq!(gain.get(), 0.0);
    }

    #[test]
    fn transmit_worker_streams_a_complete_bounded_transmission() {
        let mode = Mode::Robot36;
        let size = ImageSize::new(mode.spec().width().into(), mode.spec().height().into()).unwrap();
        let frame = Arc::new(RgbImage::new(size, Rgb8::new(80, 140, 200)));
        let (writer, mut reader) = synthetic_playback(8_000, 512).unwrap();
        let worker = TxWorker::spawn(
            writer,
            mode,
            frame,
            Some(Identification {
                station_id: FskId::new("N0CALL").unwrap(),
                number: None,
            }),
            Arc::new(TxGain::from_travel(1.0)),
        );
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
