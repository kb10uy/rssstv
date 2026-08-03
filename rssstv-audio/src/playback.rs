use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use cpal::{FromSample, Sample, Stream, traits::StreamTrait};
use ringbuf::{
    HeapCons, HeapProd,
    traits::{Consumer, Observer, Producer, Split},
};

use crate::AudioError;

#[derive(Debug, Default)]
pub(crate) struct PlaybackState {
    written: AtomicU64,
    played: AtomicU64,
    underrun: AtomicU64,
    finished: AtomicBool,
    cancelled: AtomicBool,
}

/// Open playback device.
///
/// Holding this value keeps the device open. The writing half may move to a
/// worker thread while this handle remains on the thread that opened it.
pub struct Playback {
    stream: Stream,
    sample_rate_hz: u32,
    channels: u16,
    state: Arc<PlaybackState>,
}

impl Playback {
    pub(crate) const fn new(
        stream: Stream,
        sample_rate_hz: u32,
        channels: u16,
        state: Arc<PlaybackState>,
    ) -> Self {
        Self {
            stream,
            sample_rate_hz,
            channels,
            state,
        }
    }

    /// Returns the physical playback rate in hertz.
    pub const fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    /// Returns the physical output channel count.
    pub const fn channels(&self) -> u16 {
        self.channels
    }

    /// Starts or resumes playback.
    pub fn play(&self) -> Result<(), AudioError> {
        self.stream
            .play()
            .map_err(|error| AudioError::Backend(error.to_string()))
    }

    /// Suspends playback without closing the device.
    pub fn pause(&self) -> Result<(), AudioError> {
        self.stream
            .pause()
            .map_err(|error| AudioError::Backend(error.to_string()))
    }

    /// Returns the number of mono samples waiting for the device callback.
    pub fn queued_samples(&self) -> usize {
        let written = self.state.written.load(Ordering::Acquire);
        let played = self.state.played.load(Ordering::Acquire);
        usize::try_from(written.saturating_sub(played)).unwrap_or(usize::MAX)
    }

    /// Returns the number of queued mono samples delivered to the device.
    pub fn played_samples(&self) -> u64 {
        self.state.played.load(Ordering::Acquire)
    }

    /// Returns the number of unexpected silent samples caused by starvation.
    pub fn underrun_samples(&self) -> u64 {
        self.state.underrun.load(Ordering::Acquire)
    }

    /// Returns whether the producer finished and the device consumed its queue.
    pub fn is_complete(&self) -> bool {
        self.state.finished.load(Ordering::Acquire) && self.queued_samples() == 0
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.state.cancelled.store(true, Ordering::Release);
    }
}

impl core::fmt::Debug for Playback {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Playback")
            .field("sample_rate_hz", &self.sample_rate_hz)
            .field("channels", &self.channels)
            .field("queued_samples", &self.queued_samples())
            .finish_non_exhaustive()
    }
}

/// Writing end of a bounded mono playback queue.
pub struct PlaybackWriter {
    producer: HeapProd<f32>,
    state: Arc<PlaybackState>,
    sample_rate_hz: u32,
}

impl PlaybackWriter {
    pub(crate) const fn new(
        producer: HeapProd<f32>,
        state: Arc<PlaybackState>,
        sample_rate_hz: u32,
    ) -> Self {
        Self {
            producer,
            state,
            sample_rate_hz,
        }
    }

    /// Returns the physical playback rate in hertz.
    pub const fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    /// Pushes as many mono samples as the bounded queue can accept.
    pub fn write(&mut self, samples: &[f32]) -> usize {
        if self.is_cancelled() || self.state.finished.load(Ordering::Acquire) {
            return 0;
        }
        let written = self.producer.push_slice(samples);
        self.state
            .written
            .fetch_add(written as u64, Ordering::Release);
        written
    }

    /// Returns the number of mono samples the queue can accept immediately.
    pub fn vacant(&self) -> usize {
        self.producer.vacant_len()
    }

    /// Announces that no more samples will be written.
    pub fn finish(&self) {
        self.state.finished.store(true, Ordering::Release);
    }

    /// Returns whether the playback handle was dropped.
    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Acquire)
    }
}

impl Drop for PlaybackWriter {
    fn drop(&mut self) {
        self.finish();
    }
}

impl core::fmt::Debug for PlaybackWriter {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("PlaybackWriter")
            .field("sample_rate_hz", &self.sample_rate_hz)
            .field("vacant", &self.vacant())
            .finish_non_exhaustive()
    }
}

/// Reading end of a synthetic playback queue.
pub struct PlaybackReader {
    consumer: HeapCons<f32>,
    state: Arc<PlaybackState>,
    sample_rate_hz: u32,
}

impl PlaybackReader {
    fn new(consumer: HeapCons<f32>, state: Arc<PlaybackState>, sample_rate_hz: u32) -> Self {
        Self {
            consumer,
            state,
            sample_rate_hz,
        }
    }

    /// Returns the synthetic playback rate in hertz.
    pub const fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    /// Moves available mono samples into `buffer` without blocking.
    pub fn read(&mut self, buffer: &mut [f32]) -> usize {
        let count = self.consumer.pop_slice(buffer);
        record_played(&self.state, count);
        count
    }

    /// Returns whether the producer finished and this reader consumed its queue.
    pub fn is_complete(&self) -> bool {
        self.state.finished.load(Ordering::Acquire)
            && self.state.written.load(Ordering::Acquire)
                <= self.state.played.load(Ordering::Acquire)
    }
}

impl core::fmt::Debug for PlaybackReader {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("PlaybackReader")
            .field("sample_rate_hz", &self.sample_rate_hz)
            .finish_non_exhaustive()
    }
}

/// Creates an in-memory playback queue with no device behind it.
pub fn synthetic_playback(
    sample_rate_hz: u32,
    capacity_samples: usize,
) -> Result<(PlaybackWriter, PlaybackReader), AudioError> {
    validate(sample_rate_hz, capacity_samples)?;
    let (producer, consumer) = ringbuf::HeapRb::<f32>::new(capacity_samples).split();
    let state = Arc::new(PlaybackState::default());
    Ok((
        PlaybackWriter::new(producer, Arc::clone(&state), sample_rate_hz),
        PlaybackReader::new(consumer, state, sample_rate_hz),
    ))
}

pub(crate) fn queue(
    sample_rate_hz: u32,
    capacity_samples: usize,
) -> Result<(PlaybackWriter, HeapCons<f32>, Arc<PlaybackState>), AudioError> {
    validate(sample_rate_hz, capacity_samples)?;
    let (producer, consumer) = ringbuf::HeapRb::<f32>::new(capacity_samples).split();
    let state = Arc::new(PlaybackState::default());
    Ok((
        PlaybackWriter::new(producer, Arc::clone(&state), sample_rate_hz),
        consumer,
        state,
    ))
}

fn validate(sample_rate_hz: u32, capacity_samples: usize) -> Result<(), AudioError> {
    if capacity_samples == 0 {
        return Err(AudioError::EmptyCapacity);
    }
    if sample_rate_hz < crate::MINIMUM_SAMPLE_RATE_HZ {
        return Err(AudioError::UnsupportedConfiguration(format!(
            "{sample_rate_hz} Hz"
        )));
    }
    Ok(())
}

fn record_played(state: &PlaybackState, count: usize) {
    state.played.fetch_add(count as u64, Ordering::Release);
}

fn render<T>(output: &mut [T], channels: usize, consumer: &mut HeapCons<f32>, state: &PlaybackState)
where
    T: Sample + FromSample<f32>,
{
    if channels == 0 {
        return;
    }
    for frame in output.chunks_mut(channels) {
        let sample = match consumer.try_pop() {
            Some(sample) => {
                record_played(state, 1);
                sample
            }
            None => {
                if !state.finished.load(Ordering::Acquire)
                    && !state.cancelled.load(Ordering::Acquire)
                {
                    state.underrun.fetch_add(1, Ordering::Relaxed);
                }
                0.0
            }
        };
        let converted = T::from_sample(sample);
        frame.fill(converted);
    }
}

pub(crate) fn playback_callback<T>(
    channels: usize,
    mut consumer: HeapCons<f32>,
    state: Arc<PlaybackState>,
) -> impl FnMut(&mut [T], &cpal::OutputCallbackInfo) + Send + 'static
where
    T: Sample + FromSample<f32> + Send + 'static,
{
    move |output, _| render(output, channels, &mut consumer, &state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_queue_is_bounded_and_reports_completion() {
        let (mut writer, mut reader) = synthetic_playback(48_000, 3).unwrap();
        assert_eq!(writer.write(&[0.1, 0.2, 0.3, 0.4]), 3);
        assert_eq!(writer.vacant(), 0);
        writer.finish();
        assert!(!reader.is_complete());

        let mut output = [0.0; 4];
        assert_eq!(reader.read(&mut output), 3);
        assert_eq!(&output[..3], &[0.1, 0.2, 0.3]);
        assert!(reader.is_complete());
    }

    #[test]
    fn callback_duplicates_mono_and_fills_underflow_with_silence() {
        let (mut writer, mut consumer, state) = queue(48_000, 4).unwrap();
        writer.write(&[0.25, -0.5]);
        let mut output = [1.0_f32; 6];

        render(&mut output, 2, &mut consumer, &state);

        assert_eq!(output, [0.25, 0.25, -0.5, -0.5, 0.0, 0.0]);
        assert_eq!(state.played.load(Ordering::Acquire), 2);
        assert_eq!(state.underrun.load(Ordering::Acquire), 1);
    }

    #[test]
    fn finished_queue_does_not_count_trailing_silence_as_underrun() {
        let (writer, mut consumer, state) = queue(48_000, 2).unwrap();
        writer.finish();
        let mut output = [1_i16; 4];

        render(&mut output, 2, &mut consumer, &state);

        assert_eq!(output, [0; 4]);
        assert_eq!(state.underrun.load(Ordering::Acquire), 0);
    }

    #[test]
    fn invalid_synthetic_configuration_is_rejected() {
        assert!(matches!(
            synthetic_playback(48_000, 0),
            Err(AudioError::EmptyCapacity)
        ));
        assert!(matches!(
            synthetic_playback(4_000, 1),
            Err(AudioError::UnsupportedConfiguration(_))
        ));
    }
}
