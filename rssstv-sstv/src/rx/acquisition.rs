use alloc::vec::Vec;

use crate::SstvError;
use crate::time::SstvDuration;

use super::clock::RasterClock;
use super::config::sync_detector_delay_samples;
use super::input::SampleBuffer;
use super::raster::RasterProfile;
use super::sync::RUN_THRESHOLD;

#[cfg(test)]
pub(super) const ACQUISITION_PERIODS: u64 = 32;
pub(super) const STARTUP_PERIODS: u64 = 5;
const REQUIRED_SYNCS: usize = 4;
const MAX_RATE_ERROR: f64 = 0.08;

#[cfg(test)]
pub(super) fn window_samples(profile: RasterProfile, sample_rate_hz: u32) -> u64 {
    period_samples(profile, sample_rate_hz, ACQUISITION_PERIODS)
}

pub(super) fn startup_window_samples(profile: RasterProfile, sample_rate_hz: u32) -> u64 {
    period_samples(profile, sample_rate_hz, STARTUP_PERIODS)
}

fn period_samples(profile: RasterProfile, sample_rate_hz: u32, periods: u64) -> u64 {
    let product = u128::from(profile.period_ps) * u128::from(sample_rate_hz) * u128::from(periods);
    product.div_ceil(1_000_000_000_000) as u64
}

pub(super) fn acquire_startup(
    input: &SampleBuffer,
    profile: RasterProfile,
    sample_rate_hz: u32,
    sync_detector_delay: SstvDuration,
    fit_rate: bool,
) -> Result<RasterClock, SstvError> {
    acquire_inner(
        input,
        profile,
        sample_rate_hz,
        sync_detector_delay,
        STARTUP_PERIODS,
        fit_rate,
        true,
    )
}

#[cfg(test)]
pub(super) fn acquire(
    input: &SampleBuffer,
    profile: RasterProfile,
    sample_rate_hz: u32,
    sync_detector_delay: SstvDuration,
) -> Result<RasterClock, SstvError> {
    acquire_inner(
        input,
        profile,
        sample_rate_hz,
        sync_detector_delay,
        ACQUISITION_PERIODS,
        true,
        false,
    )
}

fn acquire_inner(
    input: &SampleBuffer,
    profile: RasterProfile,
    sample_rate_hz: u32,
    sync_detector_delay: SstvDuration,
    periods: u64,
    fit_rate: bool,
    startup: bool,
) -> Result<RasterClock, SstvError> {
    let mut centers = Vec::new();
    let sync = input.sync_values();
    let mut index = 0;
    while index < sync.len() {
        if sync[index] < RUN_THRESHOLD {
            index += 1;
            continue;
        }
        let start = index;
        let mut weighted = 0.0_f64;
        let mut total = 0.0_f64;
        while index < sync.len() && sync[index] >= RUN_THRESHOLD {
            weighted += index as f64 * f64::from(sync[index]);
            total += f64::from(sync[index]);
            index += 1;
        }
        let relative = if total > 0.0 {
            (weighted / total + 0.5) as u64
        } else {
            ((start + index) / 2) as u64
        };
        centers.push(input.first() + relative);
    }

    let nominal = profile.period_ps as f64 * f64::from(sample_rate_hz) / 1_000_000_000_000.0;
    let tolerance = nominal * 0.08 + 2.0;
    let mut best: Option<(Vec<u64>, f64, f64)> = None;
    for &first in &centers {
        let mut sequence = Vec::with_capacity(periods as usize + 1);
        sequence.push(first);
        let mut previous = first;
        for _ in 1..=periods as usize {
            let previous_offset = (previous - first) as f64;
            let target_offset = previous_offset + nominal;
            let target = first as f64 + target_offset;
            let start = centers.partition_point(|&center| center <= previous);
            let remaining = &centers[start..];
            let insertion = remaining.partition_point(|&center| (center as f64) < target);
            let found = [insertion.checked_sub(1), Some(insertion)]
                .into_iter()
                .flatten()
                .filter_map(|index| remaining.get(index).copied())
                .min_by(|left, right| {
                    (*left as f64 - target)
                        .abs()
                        .total_cmp(&(*right as f64 - target).abs())
                });
            let Some(found) = found else {
                break;
            };
            let difference = ((found as i128 - first as i128) as f64 - target_offset).abs();
            if difference > tolerance {
                break;
            }
            sequence.push(found);
            previous = found;
        }
        if sequence.len() < REQUIRED_SYNCS {
            continue;
        }

        let first_sample = sequence[0];
        let count = sequence.len() as f64;
        let mean_step = (sequence.len() - 1) as f64 / 2.0;
        let mean_offset = sequence
            .iter()
            .map(|sample| (*sample - first_sample) as f64)
            .sum::<f64>()
            / count;
        let (numerator, denominator) = sequence.iter().enumerate().fold(
            (0.0, 0.0),
            |(numerator, denominator), (step, sample)| {
                let x = step as f64 - mean_step;
                let y = (*sample - first_sample) as f64 - mean_offset;
                (numerator + x * y, denominator + x * x)
            },
        );
        let slope = numerator / denominator;
        let intercept = mean_offset - slope * mean_step;
        let residual_squared = sequence
            .iter()
            .enumerate()
            .map(|(step, sample)| {
                let fitted = intercept + slope * step as f64;
                let error = (*sample - first_sample) as f64 - fitted;
                error * error
            })
            .sum::<f64>()
            / count;
        let candidate = (sequence, slope, residual_squared);
        if best.as_ref().is_none_or(|current| {
            candidate.0.len() > current.0.len()
                || (candidate.0.len() == current.0.len() && candidate.2 < current.2)
        }) {
            best = Some(candidate);
        }
    }
    let (sequence, fitted_samples_per_period, residual_squared) =
        best.ok_or(SstvError::RasterNotAcquired)?;
    let samples_per_period = if fit_rate {
        fitted_samples_per_period
    } else {
        nominal
    };
    let effective = samples_per_period * 1.0e12 / profile.period_ps as f64;
    let rate_error = (effective / f64::from(sample_rate_hz) - 1.0).abs();
    if rate_error > MAX_RATE_ERROR || residual_squared > tolerance * tolerance / 4.0 {
        return Err(SstvError::RasterNotAcquired);
    }
    let fitted_first = if !startup {
        let count = sequence.len() as f64;
        let mean_step = (sequence.len() - 1) as f64 / 2.0;
        let mean_offset = sequence
            .iter()
            .map(|sample| (*sample - sequence[0]) as f64)
            .sum::<f64>()
            / count;
        sequence[0] as f64 + mean_offset - samples_per_period * mean_step
    } else {
        sequence[1] as f64 - nominal
    };
    let epoch = fitted_first
        - effective * profile.sync_center_ps as f64 / 1.0e12
        - sync_detector_delay_samples(sample_rate_hz, sync_detector_delay);
    RasterClock::from_estimate(epoch, effective)
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::mode::Mode;
    use crate::rx::input::DemodulatedBlock;

    #[test]
    fn acquisition_selects_longest_sequence_and_fits_effective_rate() {
        let profile = RasterProfile::for_mode(Mode::Martin2).unwrap();
        let physical_rate = 10_000;
        let effective_rate = 10_200.0;
        let period = profile.period_ps as f64 * effective_rate / 1.0e12;
        let count = window_samples(profile, physical_rate) as usize;
        let frequency = vec![1900.0; count];
        let mut sync = vec![0.0; count];
        for center in [100_usize, 100 + period as usize] {
            sync[center] = 1.0;
        }
        let epoch = 400.0;
        for unit in 0..15 {
            let center = epoch
                + effective_rate * profile.sync_center_ps as f64 / 1.0e12
                + period * unit as f64;
            sync[center as usize] = 1.0;
        }
        let block = DemodulatedBlock::new(0, &frequency, &sync);
        let mut input = SampleBuffer::new(0);
        input.append(block, frequency.len());
        let clock = acquire(&input, profile, physical_rate, SstvDuration::ZERO).unwrap();
        assert!((clock.effective_sample_rate_hz() - effective_rate).abs() < 2.0);
        assert!(
            clock.source_epoch().abs_diff(epoch as u64) <= 1,
            "epoch={}",
            clock.source_epoch()
        );
    }

    #[test]
    fn acquisition_rate_does_not_accumulate_pixel_scale_drift() {
        let profile = RasterProfile::for_mode(Mode::Martin2).unwrap();
        let sample_rate = 48_000;
        let epoch = 400.0;
        let count = window_samples(profile, sample_rate) as usize;
        let frequency = vec![1900.0; count];
        let mut sync = vec![0.0; count];
        for unit in 0..ACQUISITION_PERIODS {
            let center = epoch
                + f64::from(sample_rate) * profile.sync_center_ps as f64 / 1.0e12
                + f64::from(sample_rate) * profile.period_ps as f64 / 1.0e12 * unit as f64;
            sync[center.round() as usize] = 1.0;
        }
        let block = DemodulatedBlock::new(0, &frequency, &sync);
        let mut input = SampleBuffer::new(0);
        input.append(block, frequency.len());
        let clock = acquire(&input, profile, sample_rate, SstvDuration::ZERO).unwrap();
        let last_protocol = profile.period_ps * 255 + profile.sync_center_ps;
        let fitted = clock.position_at(last_protocol).unwrap();
        let expected = epoch + f64::from(sample_rate) * last_protocol as f64 / 1.0e12;
        assert!(
            (fitted - expected).abs() < 1.0,
            "drift={}",
            fitted - expected
        );
    }

    #[test]
    fn startup_can_keep_the_nominal_rate_when_slant_is_disabled() {
        let profile = RasterProfile::for_mode(Mode::Martin2).unwrap();
        let physical_rate = 10_000;
        let effective_rate = 10_050.0;
        let count = startup_window_samples(profile, physical_rate) as usize;
        let frequency = vec![1900.0; count];
        let mut sync = vec![0.0; count];
        let period = profile.period_ps as f64 * effective_rate / 1.0e12;
        for unit in 0..STARTUP_PERIODS {
            let center = 100.0 + period * unit as f64;
            sync[center.round() as usize] = 1.0;
        }
        let block = DemodulatedBlock::new(10_000, &frequency, &sync);
        let mut input = SampleBuffer::new(10_000);
        input.append(block, frequency.len());

        let clock =
            acquire_startup(&input, profile, physical_rate, SstvDuration::ZERO, false).unwrap();
        assert_eq!(clock.effective_sample_rate_hz(), f64::from(physical_rate));
    }
}
