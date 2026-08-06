use alloc::collections::VecDeque;

use crate::SstvError;

/// Parallel demodulator output arrays at contiguous absolute sample positions.
#[derive(Clone, Copy, Debug)]
pub struct DemodulatedBlock<'a> {
    first_sample: u64,
    frequency_hz: &'a [f32],
    sync_strength: &'a [f32],
}

impl<'a> DemodulatedBlock<'a> {
    /// Constructs a block. The decoder validates lengths, values, and continuity.
    pub const fn new(first_sample: u64, frequency_hz: &'a [f32], sync_strength: &'a [f32]) -> Self {
        Self {
            first_sample,
            frequency_hz,
            sync_strength,
        }
    }

    /// Returns the absolute position of the first sample.
    pub const fn first_sample(self) -> u64 {
        self.first_sample
    }

    /// Returns the actual demodulated frequencies in hertz.
    pub const fn frequency_hz(self) -> &'a [f32] {
        self.frequency_hz
    }

    /// Returns normalized synchronization strengths.
    pub const fn sync_strength(self) -> &'a [f32] {
        self.sync_strength
    }

    pub(super) fn validate_continuity(self, expected: Option<u64>) -> Result<(), SstvError> {
        if self.frequency_hz.len() != self.sync_strength.len() {
            return Err(SstvError::DemodulatedLengthMismatch);
        }
        if let Some(expected) = expected
            && self.first_sample != expected
        {
            return Err(SstvError::DemodulatedGap {
                expected,
                actual: self.first_sample,
            });
        }
        self.first_sample
            .checked_add(self.frequency_hz.len() as u64)
            .ok_or(SstvError::SamplePositionOverflow)?;
        Ok(())
    }

    pub(super) fn validate_range(self, offset: usize, count: usize) -> Result<(), SstvError> {
        for (relative, (&frequency, &sync)) in self.frequency_hz[offset..offset + count]
            .iter()
            .zip(&self.sync_strength[offset..offset + count])
            .enumerate()
        {
            if !frequency.is_finite()
                || frequency < 0.0
                || !sync.is_finite()
                || !(0.0..=1.0).contains(&sync)
            {
                return Err(SstvError::InvalidDemodulatedSample {
                    offset: offset + relative,
                });
            }
        }
        Ok(())
    }
}

/// Steps per hertz a retained frequency is stored in.
///
/// A picture is carried between 1500 and 2300 Hz, so a sixteenth of a hertz is
/// a fiftieth of one of the 256 levels a pixel can take, and far below what the
/// demodulator itself resolves. Retaining a whole reception is what costs the
/// most memory a receiver asks for, and storing the pair as two bytes and one
/// rather than as two floats is what makes the longest mode fit on a machine
/// with little to spare.
const FREQUENCY_SCALE: f32 = 16.0;

/// Highest frequency the stored form can carry, above which it saturates.
///
/// Four kilohertz is well clear of the band any SSTV tone occupies, so nothing
/// a reception is decoded from reaches it.
const FREQUENCY_CEILING: f32 = u16::MAX as f32 / FREQUENCY_SCALE;

fn store_frequency(hertz: f32) -> u16 {
    // Rounded by addition rather than by `round`, which the core library does
    // not offer, and safe because the value is clamped non-negative first.
    (hertz.clamp(0.0, FREQUENCY_CEILING) * FREQUENCY_SCALE + 0.5) as u16
}

fn load_frequency(stored: u16) -> f32 {
    f32::from(stored) / FREQUENCY_SCALE
}

fn store_sync(strength: f32) -> u8 {
    (strength.clamp(0.0, 1.0) * f32::from(u8::MAX) + 0.5) as u8
}

fn load_sync(stored: u8) -> f32 {
    f32::from(stored) / f32::from(u8::MAX)
}

/// Demodulated samples at contiguous absolute positions, in a retained form.
///
/// The values are quantized on the way in and read back as the floats every
/// caller works in, so the resolution loss is confined to this type. See
/// [`FREQUENCY_SCALE`] for what that resolution buys.
#[derive(Clone, Debug)]
pub(super) struct SampleBuffer {
    first: u64,
    frequency: VecDeque<u16>,
    sync: VecDeque<u8>,
}

impl SampleBuffer {
    pub(super) fn new(first: u64) -> Self {
        Self {
            first,
            frequency: VecDeque::new(),
            sync: VecDeque::new(),
        }
    }

    /// Builds a buffer holding `samples` before it has to grow.
    ///
    /// A buffer that grows into a long reception doubles its way there, which
    /// both leaves up to half the allocation unused and copies everything
    /// retained so far each time it doubles. Asking for the room once is what
    /// keeps the peak down to what is actually held.
    pub(super) fn with_capacity(first: u64, samples: usize) -> Self {
        Self {
            first,
            frequency: VecDeque::with_capacity(samples),
            sync: VecDeque::with_capacity(samples),
        }
    }

    pub(super) fn first(&self) -> u64 {
        self.first
    }

    pub(super) fn end(&self) -> u64 {
        self.first + self.frequency.len() as u64
    }

    pub(super) fn len(&self) -> usize {
        self.frequency.len()
    }

    pub(super) fn append(&mut self, block: DemodulatedBlock<'_>, count: usize) {
        self.frequency.extend(
            block.frequency_hz[..count]
                .iter()
                .map(|hertz| store_frequency(*hertz)),
        );
        self.sync.extend(
            block.sync_strength[..count]
                .iter()
                .map(|strength| store_sync(*strength)),
        );
    }

    pub(super) fn frequency(&self, sample: u64) -> Option<f32> {
        let index = usize::try_from(sample.checked_sub(self.first)?).ok()?;
        self.frequency.get(index).copied().map(load_frequency)
    }

    /// Returns how many synchronization strengths are retained.
    pub(super) fn sync_len(&self) -> usize {
        self.sync.len()
    }

    /// Returns one retained synchronization strength by position.
    ///
    /// Positional rather than by sample position, because acquisition walks the
    /// whole retained run looking for pulses rather than asking about a sample
    /// it has already located.
    pub(super) fn sync_at(&self, index: usize) -> Option<f32> {
        self.sync.get(index).copied().map(load_sync)
    }

    pub(super) fn sync(&self, sample: u64) -> Option<f32> {
        let index = usize::try_from(sample.checked_sub(self.first)?).ok()?;
        self.sync.get(index).copied().map(load_sync)
    }

    /// Copies the samples at `sample` and later into a new buffer.
    ///
    /// Live slant correction rebuilds the decoding window from retained
    /// samples after the raster clock moves, and only the undecoded remainder
    /// is needed, so this copies a short tail rather than the whole buffer.
    pub(super) fn tail_from(&self, sample: u64) -> Self {
        let skip = usize::try_from(sample.saturating_sub(self.first))
            .unwrap_or(usize::MAX)
            .min(self.len());
        Self {
            first: self.first + skip as u64,
            frequency: self.frequency.iter().skip(skip).copied().collect(),
            sync: self.sync.iter().skip(skip).copied().collect(),
        }
    }

    pub(super) fn discard_before(&mut self, sample: u64) {
        let count = sample.saturating_sub(self.first).min(self.len() as u64) as usize;
        self.frequency.drain(..count);
        self.sync.drain(..count);
        self.first += count as u64;
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn retained(frequency: &[f32], sync: &[f32]) -> SampleBuffer {
        let mut buffer = SampleBuffer::new(0);
        buffer.append(DemodulatedBlock::new(0, frequency, sync), frequency.len());
        buffer
    }

    /// The retained form is what a refinement re-decodes the picture from, so
    /// what it loses has to stay far below what one pixel level spans. A pixel
    /// covers 800 hertz over 256 levels, or a little over three hertz each.
    #[rstest]
    #[case(0.0)]
    #[case(1_200.0)]
    #[case(1_500.0)]
    #[case(1_900.375)]
    #[case(2_300.0)]
    #[case(2_999.97)]
    fn retained_frequencies_round_trip_within_one_step(#[case] hertz: f32) {
        let buffer = retained(&[hertz], &[0.0]);
        let read = buffer.frequency(0).expect("the sample just appended");
        assert!(
            (read - hertz).abs() <= 1.0 / FREQUENCY_SCALE,
            "{hertz} Hz read back as {read} Hz"
        );
    }

    /// Nothing a reception is decoded from reaches the ceiling, but a value
    /// that does must not wrap around into the picture band.
    #[test]
    fn frequencies_above_the_ceiling_saturate() {
        let buffer = retained(&[f32::MAX, 8_000.0], &[0.0, 0.0]);
        assert_eq!(buffer.frequency(0), Some(FREQUENCY_CEILING));
        assert_eq!(buffer.frequency(1), Some(FREQUENCY_CEILING));
    }

    /// Acquisition extracts pulse runs at 0.50 and live synchronization accepts
    /// a peak at 0.35 with a contrast of 0.20, so the step has to be far finer
    /// than the gaps between those.
    #[rstest]
    #[case(0.0)]
    #[case(0.2)]
    #[case(0.35)]
    #[case(0.5)]
    #[case(1.0)]
    fn retained_sync_round_trips_within_one_step(#[case] strength: f32) {
        let buffer = retained(&[0.0], &[strength]);
        let read = buffer.sync(0).expect("the sample just appended");
        assert!(
            (read - strength).abs() <= 1.0 / f32::from(u8::MAX),
            "{strength} read back as {read}"
        );
        assert_eq!(buffer.sync_at(0), Some(read));
    }

    #[test]
    fn a_reserved_buffer_starts_empty() {
        let buffer = SampleBuffer::with_capacity(64, 4_096);
        assert_eq!(buffer.len(), 0);
        assert_eq!(buffer.first(), 64);
        assert_eq!(buffer.sync_len(), 0);
        assert_eq!(buffer.sync_at(0), None);
    }

    /// Copying a tail carries the retained values, not the ones they came from.
    #[test]
    fn a_tail_keeps_what_was_retained() {
        let buffer = retained(&[1_500.0, 1_900.0, 2_300.0], &[0.1, 0.6, 0.9]);
        let tail = buffer.tail_from(1);
        assert_eq!(tail.first(), 1);
        assert_eq!(tail.len(), 2);
        assert_eq!(tail.frequency(1), buffer.frequency(1));
        assert_eq!(tail.sync(2), buffer.sync(2));
    }
}
