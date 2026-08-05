use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use rssstv_audio::CaptureReader;
use rssstv_demodulator::SyncStart;
use rssstv_sstv::{
    image::{ImageSize, Rgb8, RgbImage},
    mode::Mode,
};

use crate::worker::receive::session::run;

mod session;

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
    pub(super) fn from_image(image: &RgbImage) -> Self {
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

    /// Rebuilds the image the frame was published from.
    ///
    /// The alpha channel carries nothing: every frame originates from an
    /// [`RgbImage`] and is written fully opaque, so dropping it loses no
    /// decoded pixel. A frame with no area has no image.
    pub fn to_image(&self) -> Option<RgbImage> {
        let size = ImageSize::new(self.width as usize, self.height as usize).ok()?;
        let pixels = self
            .rgba
            .chunks_exact(4)
            .map(|pixel| Rgb8::new(pixel[0], pixel[1], pixel[2]))
            .collect();
        RgbImage::from_pixels(size, pixels).ok()
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
    /// The contest numbers decoded alongside those identifiers.
    pub numbers: Vec<String>,
    pub dropped_samples: u64,
    pub error: Option<String>,
}

/// Single-slot handoff holding the newest observation.
///
/// The interface only ever uses the newest snapshot, so the worker overwrites
/// the slot instead of queueing. This bounds the handoff by construction: an
/// interface that stops polling cannot make the worker accumulate frames.
#[derive(Debug, Default)]
pub(super) struct Mailbox {
    slot: Mutex<Option<Snapshot>>,
}

impl Mailbox {
    /// Replaces the pending snapshot, keeping payloads not yet collected.
    pub(super) fn publish(&self, mut snapshot: Snapshot) {
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
    vis_restart: Arc<AtomicBool>,
    sync_start: Arc<Mutex<SyncStart>>,
}

impl Worker {
    /// Starts decoding everything `reader` produces.
    pub fn spawn(
        reader: CaptureReader,
        slant: bool,
        vis_restart: bool,
        sync_start: SyncStart,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let slant = Arc::new(AtomicBool::new(slant));
        let vis_restart = Arc::new(AtomicBool::new(vis_restart));
        let sync_start = Arc::new(Mutex::new(sync_start));
        let mailbox = Arc::new(Mailbox::default());
        let join = {
            let stop = Arc::clone(&stop);
            let slant = Arc::clone(&slant);
            let vis_restart = Arc::clone(&vis_restart);
            let sync_start = Arc::clone(&sync_start);
            let mailbox = Arc::clone(&mailbox);
            thread::Builder::new()
                .name("rssstv-receive".to_owned())
                .spawn(move || run(reader, &mailbox, &stop, &slant, &vis_restart, &sync_start))
                .ok()
        };
        Self {
            stop,
            join,
            mailbox,
            slant,
            vis_restart,
            sync_start,
        }
    }

    pub fn set_slant(&self, enabled: bool) {
        self.slant.store(enabled, Ordering::Relaxed);
    }

    pub fn set_vis_restart(&self, enabled: bool) {
        self.vis_restart.store(enabled, Ordering::Relaxed);
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

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

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
    use std::{
        f64::consts::TAU,
        time::{Duration, Instant},
    };

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
        let worker = Worker::spawn(reader, true, true, sync_start);
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

    /// A station that notices a mistake stops and sends again a few seconds
    /// later. Its new header arrives while the abandoned picture is still being
    /// decoded, so the reception only starts over if the header is still being
    /// listened for.
    #[test]
    fn a_station_sending_again_is_received_from_its_new_header() {
        let mode = Mode::Scottie1;
        let expected = source_image(mode);
        let abandoned = transmission(mode, expected.clone(), 0.0);
        let mut pcm = abandoned[..abandoned.len() * 3 / 4].to_vec();
        pcm.extend(core::iter::repeat_n(0.0, RATE as usize * 3));
        pcm.extend(transmission(mode, expected.clone(), 0.0));

        let (snapshot, frame) = receive(&pcm, RATE as usize * 3);

        assert_eq!(snapshot.progress, Progress::Complete, "{snapshot:?}");
        let frame = frame.expect("a decoded frame");
        assert!(
            mean_abs_error(&frame, &expected) < 40.0,
            "the second transmission decoded poorly: {}",
            mean_abs_error(&frame, &expected)
        );
        let history = snapshot.history.expect("the abandoned reception is kept");
        assert_eq!(history.mode, mode);
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
