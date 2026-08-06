use alloc::vec::Vec;

use crate::mode::Mode;

use super::{
    raster::RasterProfile,
    sync::{MIN_CONFIDENCE, SyncObservation},
};

const MIN_OBSERVATIONS: usize = 6;

/// Least-squares estimate of raster timing from accepted sync observations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlantEstimate {
    /// Fitted physical samples per protocol second.
    pub effective_sample_rate_hz: f64,
    /// Difference from the configured physical sample rate in parts per million.
    pub error_ppm: f64,
    /// Root-mean-square residual of accepted observations in samples.
    pub residual_samples: f64,
    /// Fitted absolute sample corresponding to protocol raster epoch zero.
    pub source_epoch: f64,
    /// Number of observations retained by confidence and outlier rejection.
    pub observations: usize,
}

/// Robust front end followed by a least-squares raster slant fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlantEstimator {
    configured_sample_rate_hz: f64,
    period_ps: u64,
    sync_center_ps: u64,
}

impl SlantEstimator {
    /// Constructs an estimator for a mode with implemented raster metadata.
    ///
    /// Observations are expected in the frequency stream's time base, which is
    /// what [`super::SyncObservation`] reports, so no detector delay enters the
    /// fitted epoch.
    pub fn for_mode(configured_sample_rate_hz: u32, mode: Mode) -> Option<Self> {
        let profile = RasterProfile::for_mode(mode)?;
        Some(Self::new(
            configured_sample_rate_hz,
            profile.period_ps,
            profile.sync_center_ps,
        ))
    }

    pub(super) fn new(configured_sample_rate_hz: u32, period_ps: u64, sync_center_ps: u64) -> Self {
        Self {
            configured_sample_rate_hz: f64::from(configured_sample_rate_hz),
            period_ps,
            sync_center_ps,
        }
    }

    /// Fits timing, rejecting weak observations and large residual outliers.
    ///
    /// At least six accepted observations are required. A median pairwise slope
    /// seeds outlier rejection; the reported values are then ordinary least squares.
    pub fn estimate(&self, observations: &[SyncObservation]) -> Option<SlantEstimate> {
        if self.configured_sample_rate_hz <= 0.0 || self.period_ps == 0 {
            return None;
        }
        let accepted: Vec<_> = observations
            .iter()
            .copied()
            .filter(|value| value.confidence >= MIN_CONFIDENCE)
            .collect();
        if accepted.len() < MIN_OBSERVATIONS {
            return None;
        }

        let mut slopes = Vec::new();
        for (index, left) in accepted.iter().enumerate() {
            for right in &accepted[index + 1..] {
                let units = right.unit as i64 - left.unit as i64;
                if units != 0 {
                    slopes.push(
                        (right.center_sample as f64 - left.center_sample as f64) / units as f64,
                    );
                }
            }
        }
        if slopes.is_empty() {
            return None;
        }
        slopes.sort_by(f64::total_cmp);
        let seed_slope = slopes[slopes.len() / 2];
        let mut intercepts: Vec<_> = accepted
            .iter()
            .map(|value| value.center_sample as f64 - seed_slope * value.unit as f64)
            .collect();
        intercepts.sort_by(f64::total_cmp);
        let seed_intercept = intercepts[intercepts.len() / 2];
        let nominal_period = self.configured_sample_rate_hz * self.period_ps as f64 / 1.0e12;
        let residual_limit = (nominal_period * 0.03).max(2.0);
        let filtered: Vec<_> = accepted
            .into_iter()
            .filter(|value| {
                let predicted = seed_intercept + seed_slope * value.unit as f64;
                (value.center_sample as f64 - predicted).abs() <= residual_limit
            })
            .collect();
        if filtered.len() < MIN_OBSERVATIONS {
            return None;
        }

        let points: Vec<(f64, f64)> = filtered
            .iter()
            .map(|value| (value.unit as f64, value.center_sample as f64))
            .collect();
        let (slope, intercept, residual_variance) = super::fit::least_squares_line(&points)?;
        let effective_sample_rate_hz = slope * 1.0e12 / self.period_ps as f64;
        if !effective_sample_rate_hz.is_finite() || effective_sample_rate_hz <= 0.0 {
            return None;
        }
        let residual_samples = libm::sqrt(residual_variance);
        let source_epoch =
            intercept - effective_sample_rate_hz * self.sync_center_ps as f64 / 1.0e12;
        if !source_epoch.is_finite() || source_epoch < 0.0 {
            return None;
        }
        Some(SlantEstimate {
            effective_sample_rate_hz,
            error_ppm: (effective_sample_rate_hz / self.configured_sample_rate_hz - 1.0) * 1.0e6,
            residual_samples,
            source_epoch,
            observations: filtered.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observations(rate: f64, epoch: f64) -> Vec<SyncObservation> {
        (0..10)
            .map(|unit| SyncObservation {
                unit,
                peak_sample: 0,
                center_sample: (epoch + rate * 0.2 * unit as f64).round() as u64,
                confidence: 0.9,
                contrast: 0.8,
            })
            .collect()
    }

    #[test]
    fn estimates_zero_positive_and_negative_error() {
        let estimator = SlantEstimator::new(10_000, 200_000_000_000, 0);
        for rate in [10_000.0, 10_010.0, 9_990.0] {
            let estimate = estimator.estimate(&observations(rate, 1000.0)).unwrap();
            assert!((estimate.effective_sample_rate_hz - rate).abs() < 1.0);
        }
    }

    #[test]
    fn rejects_weak_outliers_and_short_sets() {
        let estimator = SlantEstimator::new(10_000, 200_000_000_000, 0);
        let mut values = observations(10_000.0, 1000.0);
        values[3].center_sample += 900;
        values[4].confidence = 0.1;
        assert!(estimator.estimate(&values).is_some());
        assert!(estimator.estimate(&values[..5]).is_none());
    }

    #[test]
    fn phase_offset_is_removed_from_rate_fit() {
        let estimator = SlantEstimator::new(10_000, 200_000_000_000, 50_000_000_000);
        let values = observations(10_000.0, 12_500.0);
        let estimate = estimator.estimate(&values).unwrap();
        assert!((estimate.source_epoch - 12_000.0).abs() < 0.6);
    }

    #[test]
    fn mode_timing_places_the_epoch_ahead_of_the_observed_pulse() {
        let estimator = SlantEstimator::for_mode(10_000, Mode::Martin2).unwrap();
        let profile = RasterProfile::for_mode(Mode::Martin2).unwrap();
        let observed_epoch = 12_000.0 + 10_000.0 * profile.sync_center_ps as f64 / 1.0e12;
        let period_samples = 10_000.0 * profile.period_ps as f64 / 1.0e12;
        let values = (0..10)
            .map(|unit| SyncObservation {
                unit,
                peak_sample: 0,
                center_sample: (observed_epoch + period_samples * unit as f64).round() as u64,
                confidence: 0.9,
                contrast: 0.8,
            })
            .collect::<Vec<_>>();
        let estimate = estimator.estimate(&values).unwrap();
        assert!((estimate.source_epoch - 12_000.0).abs() < 0.6);
    }

    #[test]
    fn nonzero_wrapped_unit_origin_does_not_change_epoch() {
        let estimator = SlantEstimator::new(10_000, 200_000_000_000, 50_000_000_000);
        let values: Vec<_> = (250..258)
            .map(|unit| SyncObservation {
                unit,
                peak_sample: 0,
                center_sample: 12_500 + 2_000 * unit as u64,
                confidence: 1.0,
                contrast: 1.0,
            })
            .collect();
        let estimate = estimator.estimate(&values).unwrap();
        assert!((estimate.source_epoch - 12_000.0).abs() < 0.1);
    }

    #[test]
    fn public_constructor_derives_timing_from_the_mode() {
        assert!(SlantEstimator::for_mode(48_000, Mode::Martin2).is_some());
        assert!(SlantEstimator::for_mode(48_000, Mode::Avt90).is_none());
    }
}
