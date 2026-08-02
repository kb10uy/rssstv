use alloc::collections::VecDeque;

use super::clock::RasterClock;
use super::input::SampleBuffer;
use super::raster::RasterProfile;

pub(super) const HISTORY_LEN: usize = 16;
const MIN_PEAK: f32 = 0.35;
const MIN_CONTRAST: f32 = 0.20;

/// A synchronization pulse measured for one raster unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyncObservation {
    /// Zero-based raster unit associated with the pulse.
    pub unit: usize,
    /// Absolute sample at the strongest point in the search window.
    pub peak_sample: u64,
    /// Strength-weighted absolute center sample of the pulse.
    pub center_sample: u64,
    /// Normalized acceptance confidence (`0.0..=1.0`).
    pub confidence: f32,
    /// Peak strength minus local background strength (`0.0..=1.0`).
    pub contrast: f32,
}

pub(super) fn observe(
    input: &SampleBuffer,
    profile: RasterProfile,
    clock: RasterClock,
    unit: usize,
) -> Option<SyncObservation> {
    let protocol = profile.period_ps.checked_mul(unit as u64)?;
    let expected = clock
        .sample_at(protocol.checked_add(profile.sync_center_ps)?)
        .ok()?;
    let half_period = clock
        .samples_for(profile.period_ps)
        .ok()?
        .div_ceil(2)
        .max(2);
    let start = expected.saturating_sub(half_period).max(input.first());
    let end = expected
        .saturating_add(half_period)
        .saturating_add(1)
        .min(input.end());
    if end <= start + 2 {
        return None;
    }

    let mut peak = 0.0_f32;
    let mut peak_sample = start;
    let mut sum = 0.0_f64;
    let mut count = 0_u64;
    for sample in start..end {
        let value = input.sync(sample)?;
        sum += f64::from(value);
        count += 1;
        if value > peak {
            peak = value;
            peak_sample = sample;
        }
    }
    let background = (sum / count as f64) as f32;
    let contrast = (peak - background).max(0.0);
    let threshold = background + contrast * 0.5;
    let mut weighted = 0.0_f64;
    let mut weight = 0.0_f64;
    for sample in start..end {
        let value = input.sync(sample)?;
        if value >= threshold {
            let relative = f64::from(value - threshold);
            weighted += sample as f64 * relative;
            weight += relative;
        }
    }
    let center_sample = if weight > 0.0 {
        (weighted / weight + 0.5) as u64
    } else {
        peak_sample
    };
    Some(SyncObservation {
        unit,
        peak_sample,
        center_sample,
        confidence: if peak >= MIN_PEAK && contrast >= MIN_CONTRAST {
            (contrast / (1.0 - background).max(0.001)).clamp(0.0, 1.0)
        } else {
            0.0
        },
        contrast,
    })
}

pub(super) fn push_bounded(history: &mut VecDeque<SyncObservation>, value: SyncObservation) {
    if history.len() == HISTORY_LEN {
        history.pop_front();
    }
    history.push_back(value);
}
