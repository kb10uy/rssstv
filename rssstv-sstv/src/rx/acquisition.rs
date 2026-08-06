use alloc::vec::Vec;

use crate::{SstvError, time::SstvDuration};

use super::{
    clock::RasterClock,
    config::sync_detector_delay_samples,
    input::SampleBuffer,
    raster::RasterProfile,
    sync::{RUN_THRESHOLD, refine_center},
};

pub(super) const ACQUISITION_PERIODS: u64 = 32;
pub(super) const STARTUP_PERIODS: u64 = 5;
const REQUIRED_SYNCS: usize = 4;
const MAX_RATE_ERROR: f64 = 0.08;

#[derive(Clone, Copy)]
struct AcquisitionOptions {
    periods: u64,
    startup: bool,
    max_samples: Option<u64>,
}

pub(super) fn window_samples(profile: RasterProfile, sample_rate_hz: u32) -> u64 {
    period_samples(profile, sample_rate_hz, ACQUISITION_PERIODS)
}

pub(super) fn startup_window_samples(profile: RasterProfile, sample_rate_hz: u32) -> u64 {
    period_samples(profile, sample_rate_hz, STARTUP_PERIODS)
}

fn period_samples(profile: RasterProfile, sample_rate_hz: u32, periods: u64) -> u64 {
    SstvDuration::from_picos(profile.period_ps * periods).to_samples_ceil(sample_rate_hz)
}

/// Acquires the raster phase of a starting reception on the configured rate.
///
/// The startup window spans too few periods to estimate a sample rate that is
/// better than the configured one, so it only fixes the phase. MMSSTV likewise
/// begins a reception on its calibrated `SSTVSET.m_SampFreq` and leaves the rate
/// to real-time slant tracking and to the staged pass.
///
/// The phase is averaged over the window's pulses, leaving out the first one
/// because the buffer can begin part way through it.
pub(super) fn acquire_startup(
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
        AcquisitionOptions {
            periods: STARTUP_PERIODS,
            startup: true,
            max_samples: None,
        },
    )
}

pub(super) fn acquire_full(
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
        AcquisitionOptions {
            periods: ACQUISITION_PERIODS,
            startup: false,
            max_samples: Some(window_samples(profile, sample_rate_hz)),
        },
    )
}

fn acquire_inner(
    input: &SampleBuffer,
    profile: RasterProfile,
    sample_rate_hz: u32,
    sync_detector_delay: SstvDuration,
    options: AcquisitionOptions,
) -> Result<RasterClock, SstvError> {
    let mut centers = Vec::new();
    let retained = input.sync_len();
    let sync_len = options
        .max_samples
        .and_then(|count| usize::try_from(count).ok())
        .map_or(retained, |count| count.min(retained));
    let strength = |index: usize| input.sync_at(index).unwrap_or(0.0);
    let mut index = 0;
    while index < sync_len {
        if strength(index) < RUN_THRESHOLD {
            index += 1;
            continue;
        }
        let start = index;
        let mut weighted = 0.0_f64;
        let mut total = 0.0_f64;
        while index < sync_len && strength(index) >= RUN_THRESHOLD {
            weighted += index as f64 * f64::from(strength(index));
            total += f64::from(strength(index));
            index += 1;
        }
        // A pulse the window cuts short has no usable center: both its envelope
        // and its frequency span end early, which would pull the fit forward.
        if index == sync_len {
            break;
        }
        let relative = if total > 0.0 {
            (weighted / total + 0.5) as u64
        } else {
            ((start + index) / 2) as u64
        };
        let envelope_center = input.first() + relative;
        centers.push(
            refine_center(
                input,
                profile,
                envelope_center,
                sample_rate_hz,
                sync_detector_delay,
            )
            .unwrap_or_else(|| {
                (envelope_center as f64
                    - sync_detector_delay_samples(sample_rate_hz, sync_detector_delay))
                .max(0.0) as u64
            }),
        );
    }

    let nominal = profile.period_ps as f64 * f64::from(sample_rate_hz) / 1_000_000_000_000.0;
    let tolerance = nominal * 0.08 + 2.0;
    let mut best: Option<(Vec<u64>, f64, f64)> = None;
    for &first in &centers {
        let mut sequence = Vec::with_capacity(options.periods as usize + 1);
        sequence.push(first);
        let mut previous = first;
        for _ in 1..=options.periods as usize {
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
        let points: Vec<(f64, f64)> = sequence
            .iter()
            .enumerate()
            .map(|(step, sample)| (step as f64, (*sample - first_sample) as f64))
            .collect();
        let Some((slope, _, residual_squared)) = super::fit::least_squares_line(&points) else {
            continue;
        };
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
    let samples_per_period = if options.startup {
        nominal
    } else {
        fitted_samples_per_period
    };
    let effective = samples_per_period * 1.0e12 / profile.period_ps as f64;
    let rate_error = (effective / f64::from(sample_rate_hz) - 1.0).abs();
    if rate_error > MAX_RATE_ERROR || residual_squared > tolerance * tolerance / 4.0 {
        return Err(SstvError::RasterNotAcquired);
    }
    let fitted_first = if !options.startup {
        let count = sequence.len() as f64;
        let mean_step = (sequence.len() - 1) as f64 / 2.0;
        let mean_offset = sequence
            .iter()
            .map(|sample| (*sample - sequence[0]) as f64)
            .sum::<f64>()
            / count;
        sequence[0] as f64 + mean_offset - samples_per_period * mean_step
    } else {
        let steps = (sequence.len() - 1) as f64;
        let mean_offset = sequence[1..]
            .iter()
            .enumerate()
            .map(|(step, sample)| {
                (*sample - sequence[0]) as f64 - samples_per_period * (step + 1) as f64
            })
            .sum::<f64>()
            / steps;
        sequence[0] as f64 + mean_offset
    };
    let epoch = fitted_first - effective * profile.sync_center_ps as f64 / 1.0e12;
    RasterClock::from_estimate(epoch, effective)
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::{mode::Mode, rx::input::DemodulatedBlock};

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
        let clock = acquire_full(&input, profile, physical_rate, SstvDuration::ZERO).unwrap();
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
        let clock = acquire_full(&input, profile, sample_rate, SstvDuration::ZERO).unwrap();
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
    fn startup_keeps_the_configured_rate_and_averages_the_phase() {
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

        let clock = acquire_startup(&input, profile, physical_rate, SstvDuration::ZERO).unwrap();
        assert_eq!(clock.effective_sample_rate_hz(), f64::from(physical_rate));
    }

    #[test]
    fn startup_phase_survives_a_jittered_first_pulse() {
        let profile = RasterProfile::for_mode(Mode::Martin2).unwrap();
        let physical_rate = 10_000;
        let count = startup_window_samples(profile, physical_rate) as usize;
        let frequency = vec![1900.0; count];
        let period = profile.period_ps as f64 * f64::from(physical_rate) / 1.0e12;
        let epoch = 400.0;
        let center = |unit: u64| {
            epoch
                + f64::from(physical_rate) * profile.sync_center_ps as f64 / 1.0e12
                + period * unit as f64
        };
        let mut sync = vec![0.0; count];
        for unit in 0..STARTUP_PERIODS {
            sync[center(unit).round() as usize] = 1.0;
        }
        sync[center(0).round() as usize] = 0.0;
        sync[center(0).round() as usize + 60] = 1.0;
        let block = DemodulatedBlock::new(0, &frequency, &sync);
        let mut input = SampleBuffer::new(0);
        input.append(block, frequency.len());

        let clock = acquire_startup(&input, profile, physical_rate, SstvDuration::ZERO).unwrap();
        assert!(
            clock.source_epoch().abs_diff(epoch as u64) <= 1,
            "epoch={}",
            clock.source_epoch()
        );
    }
}
