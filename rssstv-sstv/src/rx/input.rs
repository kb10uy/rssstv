use alloc::vec::Vec;

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

    pub(super) fn validate(self, expected: Option<u64>) -> Result<(), SstvError> {
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
        for (offset, (&frequency, &sync)) in
            self.frequency_hz.iter().zip(self.sync_strength).enumerate()
        {
            if !frequency.is_finite()
                || frequency < 0.0
                || !sync.is_finite()
                || !(0.0..=1.0).contains(&sync)
            {
                return Err(SstvError::InvalidDemodulatedSample { offset });
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(super) struct SampleBuffer {
    first: u64,
    frequency: Vec<f32>,
    sync: Vec<f32>,
}

impl SampleBuffer {
    pub(super) fn new(first: u64) -> Self {
        Self {
            first,
            frequency: Vec::new(),
            sync: Vec::new(),
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
        self.frequency
            .extend_from_slice(&block.frequency_hz[..count]);
        self.sync.extend_from_slice(&block.sync_strength[..count]);
    }

    pub(super) fn frequency(&self, sample: u64) -> Option<f32> {
        let index = usize::try_from(sample.checked_sub(self.first)?).ok()?;
        self.frequency.get(index).copied()
    }

    pub(super) fn sync_values(&self) -> &[f32] {
        &self.sync
    }

    pub(super) fn sync(&self, sample: u64) -> Option<f32> {
        let index = usize::try_from(sample.checked_sub(self.first)?).ok()?;
        self.sync.get(index).copied()
    }

    pub(super) fn discard_before(&mut self, sample: u64) {
        let count = sample.saturating_sub(self.first).min(self.len() as u64) as usize;
        self.frequency.drain(..count);
        self.sync.drain(..count);
        self.first += count as u64;
    }
}
