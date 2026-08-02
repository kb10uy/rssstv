use crate::SstvError;

#[derive(Clone, Copy, Debug)]
pub(super) struct RasterClock {
    source_epoch: f64,
    physical_sample_rate_hz: u32,
    effective_sample_rate_hz: f64,
}

impl RasterClock {
    pub(super) fn from_estimate(
        source_epoch: f64,
        physical_sample_rate_hz: u32,
        effective_sample_rate_hz: f64,
    ) -> Result<Self, SstvError> {
        if !source_epoch.is_finite()
            || source_epoch < 0.0
            || !effective_sample_rate_hz.is_finite()
            || effective_sample_rate_hz <= 0.0
        {
            return Err(SstvError::RasterNotAcquired);
        }
        Ok(Self {
            source_epoch,
            physical_sample_rate_hz,
            effective_sample_rate_hz,
        })
    }

    pub(super) fn sample_at(&self, protocol_ps: u64) -> Result<u64, SstvError> {
        let sample =
            self.source_epoch + self.effective_sample_rate_hz * protocol_ps as f64 / 1.0e12;
        if !sample.is_finite() || sample < 0.0 || sample > u64::MAX as f64 {
            return Err(SstvError::SamplePositionOverflow);
        }
        Ok(sample as u64)
    }

    pub(super) fn samples_for(&self, protocol_ps: u64) -> Result<u64, SstvError> {
        let samples = self.effective_sample_rate_hz * protocol_ps as f64 / 1.0e12;
        if !samples.is_finite() || samples < 0.0 || samples > u64::MAX as f64 {
            return Err(SstvError::SamplePositionOverflow);
        }
        let whole = samples as u64;
        Ok(whole + u64::from((whole as f64) < samples))
    }

    pub(super) fn source_epoch(&self) -> u64 {
        (self.source_epoch + 0.5) as u64
    }

    pub(super) const fn physical_sample_rate_hz(&self) -> u32 {
        self.physical_sample_rate_hz
    }

    pub(super) const fn effective_sample_rate_hz(&self) -> f64 {
        self.effective_sample_rate_hz
    }

    pub(super) fn adjust_epoch(&mut self, samples: i64) -> Result<(), SstvError> {
        let epoch = self.source_epoch + samples as f64;
        if !epoch.is_finite() || epoch < 0.0 {
            return Err(SstvError::SamplePositionOverflow);
        }
        self.source_epoch = epoch;
        Ok(())
    }
}
