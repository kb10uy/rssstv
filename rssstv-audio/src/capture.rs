use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use cpal::{FromSample, Sample, Stream, traits::StreamTrait};

use crate::error::{FaultSlot, StreamFault};
use ringbuf::{
    HeapCons, HeapProd,
    traits::{Consumer, Observer, Producer, Split},
};

use crate::AudioError;

/// Frames converted per pass inside the capture callback.
///
/// Conversion works in fixed-size passes so the callback never allocates,
/// whatever buffer size the host delivers.
const CONVERSION_FRAMES: usize = 1_024;

/// One contiguous run of captured samples.
///
/// `first_sample` counts every sample the device produced, including samples
/// dropped on overrun, so positions remain a true timeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reading {
    /// Position of the first returned sample in the device's stream.
    pub first_sample: u64,
    /// Number of samples written to the caller's buffer.
    pub count: usize,
    /// Samples dropped since the previous read, immediately before this run.
    pub dropped_before: u64,
}

impl Reading {
    /// Returns whether samples were lost immediately before this run.
    pub const fn is_discontinuous(&self) -> bool {
        self.dropped_before > 0
    }
}

/// Converts overrun counts and read lengths into stream positions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Timeline {
    position: u64,
    observed_drops: u64,
}

impl Timeline {
    /// Records a read of `count` samples taken when the device had dropped
    /// `dropped_total` samples in total.
    fn advance(&mut self, dropped_total: u64, count: usize) -> Reading {
        let dropped_before = dropped_total.saturating_sub(self.observed_drops);
        self.observed_drops = dropped_total;
        self.position += dropped_before;
        let first_sample = self.position;
        self.position += count as u64;
        Reading {
            first_sample,
            count,
            dropped_before,
        }
    }
}

/// Open capture device.
///
/// Holding this value keeps the device open; dropping it closes the stream.
/// It is bound to the thread that opened it because the underlying host
/// stream is not `Send` on every platform. Samples are read through the
/// separate [`CaptureReader`], which may be moved to another thread.
pub struct Capture {
    stream: Stream,
    sample_rate_hz: u32,
    channels: u16,
    faults: FaultSlot,
}

impl Capture {
    pub(crate) const fn new(
        stream: Stream,
        sample_rate_hz: u32,
        channels: u16,
        faults: FaultSlot,
    ) -> Self {
        Self {
            stream,
            sample_rate_hz,
            channels,
            faults,
        }
    }

    /// Takes the report the stream left if it stopped on its own.
    ///
    /// The device is not usable again after one of these; the caller has to
    /// open it anew, or pick another.
    pub fn take_fault(&self) -> Option<StreamFault> {
        self.faults.take()
    }

    /// Returns the physical capture rate in hertz.
    pub const fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    /// Returns the device's channel count before mono extraction.
    pub const fn channels(&self) -> u16 {
        self.channels
    }

    /// Starts or resumes delivery.
    pub fn resume(&self) -> Result<(), AudioError> {
        self.stream
            .play()
            .map_err(|error| AudioError::Backend(error.to_string()))
    }

    /// Suspends delivery without closing the device.
    pub fn pause(&self) -> Result<(), AudioError> {
        self.stream
            .pause()
            .map_err(|error| AudioError::Backend(error.to_string()))
    }
}

impl core::fmt::Debug for Capture {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Capture")
            .field("sample_rate_hz", &self.sample_rate_hz)
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

/// Reading end of a capture queue.
///
/// This half is `Send`, so the consumer may run on a worker thread while the
/// [`Capture`] handle stays on the thread that opened the device.
pub struct CaptureReader {
    consumer: HeapCons<f32>,
    dropped: Arc<AtomicU64>,
    sample_rate_hz: u32,
    timeline: Timeline,
}

impl CaptureReader {
    pub(crate) fn new(
        consumer: HeapCons<f32>,
        dropped: Arc<AtomicU64>,
        sample_rate_hz: u32,
    ) -> Self {
        Self {
            consumer,
            dropped,
            sample_rate_hz,
            timeline: Timeline::default(),
        }
    }

    /// Returns the physical capture rate in hertz.
    pub const fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    /// Returns the total number of samples dropped on overrun.
    pub fn dropped_samples(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Moves available samples into `buffer` without blocking.
    ///
    /// The returned count may be zero. Callers own the buffer so the adapter
    /// never allocates while streaming.
    pub fn read(&mut self, buffer: &mut [f32]) -> Reading {
        let dropped_total = self.dropped.load(Ordering::Relaxed);
        let count = self.consumer.pop_slice(buffer);
        self.timeline.advance(dropped_total, count)
    }
}

/// Writing end of a capture queue fed by the caller instead of a device.
///
/// This exists so the receive pipeline can be driven from recorded audio
/// without hardware, keeping offline runs on exactly the same code path as a
/// live device.
pub struct CaptureWriter {
    producer: HeapProd<f32>,
    dropped: Arc<AtomicU64>,
}

impl CaptureWriter {
    /// Writes what fits and counts the rest as dropped, like a device overrun.
    pub fn write(&mut self, samples: &[f32]) -> usize {
        let written = self.producer.push_slice(samples);
        if written < samples.len() {
            self.dropped
                .fetch_add((samples.len() - written) as u64, Ordering::Relaxed);
        }
        written
    }

    /// Returns the number of samples that would fit right now.
    pub fn vacant(&self) -> usize {
        self.producer.vacant_len()
    }
}

impl core::fmt::Debug for CaptureWriter {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("CaptureWriter")
            .finish_non_exhaustive()
    }
}

/// Creates an in-memory capture queue with no device behind it.
pub fn synthetic_capture(
    sample_rate_hz: u32,
    capacity_samples: usize,
) -> Result<(CaptureWriter, CaptureReader), AudioError> {
    if capacity_samples == 0 {
        return Err(AudioError::EmptyCapacity);
    }
    if sample_rate_hz < crate::MINIMUM_SAMPLE_RATE_HZ {
        return Err(AudioError::UnsupportedConfiguration(format!(
            "{sample_rate_hz} Hz"
        )));
    }
    let (producer, consumer) = ringbuf::HeapRb::<f32>::new(capacity_samples).split();
    let dropped = Arc::new(AtomicU64::new(0));
    Ok((
        CaptureWriter {
            producer,
            dropped: Arc::clone(&dropped),
        },
        CaptureReader::new(consumer, dropped, sample_rate_hz),
    ))
}

impl core::fmt::Debug for CaptureReader {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("CaptureReader")
            .field("sample_rate_hz", &self.sample_rate_hz)
            .field("timeline", &self.timeline)
            .finish_non_exhaustive()
    }
}

/// Copies the first channel of an interleaved frame run into `out`.
///
/// Returns the number of frames written, which is bounded by both the input
/// frame count and the length of `out`.
fn extract_first_channel<T>(interleaved: &[T], channels: usize, out: &mut [f32]) -> usize
where
    T: Sample,
    f32: FromSample<T>,
{
    if channels == 0 {
        return 0;
    }
    let frames = interleaved.len() / channels;
    let count = frames.min(out.len());
    for (index, slot) in out.iter_mut().take(count).enumerate() {
        *slot = interleaved[index * channels].to_sample::<f32>();
    }
    count
}

/// Extracts mono samples from one host buffer and publishes them.
///
/// Overrun is reported through `dropped` instead of blocking, because this
/// runs on the host's audio callback thread.
fn publish<T>(
    data: &[T],
    channels: usize,
    scratch: &mut [f32],
    producer: &mut HeapProd<f32>,
    dropped: &AtomicU64,
) where
    T: Sample,
    f32: FromSample<T>,
{
    if channels == 0 {
        return;
    }
    let frames = data.len() / channels;
    let mut offset = 0;
    while offset < frames {
        let converted = extract_first_channel(&data[offset * channels..], channels, scratch);
        if converted == 0 {
            return;
        }
        let written = producer.push_slice(&scratch[..converted]);
        if written < converted {
            dropped.fetch_add((converted - written) as u64, Ordering::Relaxed);
        }
        offset += converted;
    }
}

/// Builds the callback that feeds captured mono samples into `producer`.
pub(crate) fn capture_callback<T>(
    channels: usize,
    mut producer: HeapProd<f32>,
    dropped: Arc<AtomicU64>,
) -> impl FnMut(&[T], &cpal::InputCallbackInfo) + Send + 'static
where
    T: Sample + Send + 'static,
    f32: FromSample<T>,
{
    let mut scratch = [0.0_f32; CONVERSION_FRAMES];
    move |data: &[T], _| publish(data, channels, &mut scratch, &mut producer, &dropped)
}

#[cfg(test)]
mod tests {
    use ringbuf::{HeapRb, traits::Split};
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(1, &[0.25_f32, 0.5, 0.75], &[0.25, 0.5, 0.75])]
    #[case(2, &[0.25_f32, -1.0, 0.5, -1.0], &[0.25, 0.5])]
    #[case(3, &[0.25_f32, -1.0, -1.0, 0.5, -1.0, -1.0], &[0.25, 0.5])]
    fn first_channel_is_extracted(
        #[case] channels: usize,
        #[case] input: &[f32],
        #[case] expected: &[f32],
    ) {
        let mut out = [0.0_f32; 8];
        let count = extract_first_channel(input, channels, &mut out);
        assert_eq!(&out[..count], expected);
    }

    #[test]
    fn integer_samples_are_normalized() {
        let mut out = [0.0_f32; 2];
        let count = extract_first_channel(&[i16::MAX, 0, 0, 0], 2, &mut out);
        assert_eq!(count, 2);
        assert!((out[0] - 1.0).abs() < 1.0e-4);
        assert_eq!(out[1], 0.0);
    }

    #[test]
    fn extraction_is_bounded_by_the_output_buffer() {
        let mut out = [0.0_f32; 2];
        assert_eq!(extract_first_channel(&[0.0_f32; 16], 1, &mut out), 2);
    }

    #[test]
    fn zero_channels_produce_no_frames() {
        let mut out = [0.0_f32; 4];
        assert_eq!(extract_first_channel(&[0.0_f32; 4], 0, &mut out), 0);
    }

    #[test]
    fn publishing_more_than_capacity_counts_the_overrun() {
        let (mut producer, mut consumer) = HeapRb::<f32>::new(4).split();
        let dropped = AtomicU64::new(0);
        let mut scratch = [0.0_f32; CONVERSION_FRAMES];

        publish(
            &[0.1_f32, 0.2, 0.3, 0.4, 0.5, 0.6],
            1,
            &mut scratch,
            &mut producer,
            &dropped,
        );

        assert_eq!(dropped.load(Ordering::Relaxed), 2);
        let mut buffer = [0.0_f32; 8];
        assert_eq!(consumer.pop_slice(&mut buffer), 4);
        assert_eq!(&buffer[..4], &[0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn publishing_extracts_only_the_first_channel() {
        let (mut producer, mut consumer) = HeapRb::<f32>::new(8).split();
        let dropped = AtomicU64::new(0);
        let mut scratch = [0.0_f32; CONVERSION_FRAMES];

        publish(
            &[0.1_f32, -1.0, 0.2, -1.0, 0.3, -1.0],
            2,
            &mut scratch,
            &mut producer,
            &dropped,
        );

        let mut buffer = [0.0_f32; 8];
        assert_eq!(consumer.pop_slice(&mut buffer), 3);
        assert_eq!(&buffer[..3], &[0.1, 0.2, 0.3]);
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn contiguous_reads_advance_positions() {
        let mut timeline = Timeline::default();
        assert_eq!(
            timeline.advance(0, 2),
            Reading {
                first_sample: 0,
                count: 2,
                dropped_before: 0,
            }
        );
        assert_eq!(
            timeline.advance(0, 3),
            Reading {
                first_sample: 2,
                count: 3,
                dropped_before: 0,
            }
        );
    }

    #[test]
    fn dropped_samples_occupy_their_place_in_the_timeline() {
        let mut timeline = Timeline::default();
        timeline.advance(0, 2);

        let reading = timeline.advance(5, 1);
        assert_eq!(reading.dropped_before, 5);
        assert_eq!(reading.first_sample, 7);
        assert!(reading.is_discontinuous());

        let next = timeline.advance(5, 1);
        assert_eq!(next.first_sample, 8);
        assert!(!next.is_discontinuous());
    }

    #[test]
    fn empty_reads_do_not_move_the_position() {
        let mut timeline = Timeline::default();
        timeline.advance(0, 4);
        assert_eq!(timeline.advance(0, 0).first_sample, 4);
        assert_eq!(timeline.advance(0, 1).first_sample, 4);
    }
}
