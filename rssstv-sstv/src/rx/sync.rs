use alloc::collections::VecDeque;
use alloc::vec::Vec;

use super::clock::RasterClock;
use super::config::sync_detector_delay_samples;
use super::input::SampleBuffer;
use super::raster::RasterProfile;
use crate::time::SstvDuration;

pub(super) const HISTORY_LEN: usize = 16;
pub(super) const MIN_CONFIDENCE: f32 = 0.20;
pub(super) const MIN_CONTRAST: f32 = 0.20;
pub(super) const MIN_PEAK: f32 = 0.35;
pub(super) const RUN_THRESHOLD: f32 = 0.5;

/// Level separating the 1200 Hz sync pulse from the 1500 Hz porch above it.
const SYNC_LEVEL_HZ: f64 = 1_350.0;

/// Synchronization tone, whose discriminator ripple is at twice this.
const SYNC_TONE_HZ: f64 = 1_200.0;

/// A synchronization pulse measured for one raster unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyncObservation {
    /// Zero-based raster unit associated with the pulse.
    pub unit: usize,
    /// Absolute sample at the strongest point of the synchronization envelope.
    pub peak_sample: u64,
    /// Absolute center sample of the pulse in the frequency stream's time base.
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
    sample_rate_hz: u32,
    sync_detector_delay: SstvDuration,
) -> Option<SyncObservation> {
    let protocol = profile
        .period_ps
        .checked_mul(unit as u64)?
        .checked_add(profile.sync_center_ps)?;
    let expected = clock.sample_at(protocol).ok()?.checked_add(
        sync_detector_delay_samples(sample_rate_hz, sync_detector_delay).round() as u64,
    )?;
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
    let envelope_center = if weight > 0.0 {
        (weighted / weight + 0.5) as u64
    } else {
        peak_sample
    };
    let center_sample = refine_center(
        input,
        profile,
        envelope_center,
        sample_rate_hz,
        sync_detector_delay,
    )
    .unwrap_or_else(|| {
        (envelope_center as f64 - sync_detector_delay_samples(sample_rate_hz, sync_detector_delay))
            .max(0.0) as u64
    });
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

/// Locates the sync pulse on the frequency stream near an envelope center.
///
/// The synchronization envelope lags the frequency stream, and how far its
/// weighted center sits from the true pulse center also depends on the picture
/// tones either side of the pulse. The frequency stream carries neither error:
/// its own group delay is shared with the pixel windows read from it, so a
/// raster placed on a frequency-domain center needs no delay compensation and
/// samples every pixel where its content actually is.
///
/// The pulse is the window of one sync duration whose mean frequency is lowest.
/// Sliding a window of the known length beats reading edges off a threshold on
/// two counts: the discriminator ripples at twice the tone it tracks, which
/// breaks a threshold into fragments, and a threshold crossing sits at a
/// different point on each flank whenever the tones either side of the pulse
/// differ. Displacing the window in either direction trades sync samples for
/// higher ones, so its minimum is on the pulse whatever surrounds it.
///
/// Returns `None` when no plausible pulse is present, which leaves the caller
/// with the envelope estimate it already has.
pub(super) fn refine_center(
    input: &SampleBuffer,
    profile: RasterProfile,
    envelope_center: u64,
    sample_rate_hz: u32,
    sync_detector_delay: SstvDuration,
) -> Option<u64> {
    if profile.sync_duration_ps == 0 {
        return None;
    }
    let expected = f64::from(sample_rate_hz) * profile.sync_duration_ps as f64 / 1.0e12;
    let delay = sync_detector_delay_samples(sample_rate_hz, sync_detector_delay);
    let center = (envelope_center as f64 - delay).max(0.0) as u64;
    let reach = (expected * 1.5) as u64 + 2;
    let start = center.saturating_sub(reach).max(input.first());
    let end = center.saturating_add(reach).min(input.end());
    if end <= start {
        return None;
    }

    let length = (expected as usize).max(1);
    if (end - start) as usize <= length {
        return None;
    }
    let mut prefix = Vec::with_capacity((end - start) as usize + 1);
    prefix.push(0.0_f64);
    for sample in start..end {
        prefix.push(prefix[prefix.len() - 1] + f64::from(input.frequency(sample)?));
    }
    let cost: Vec<_> = (0..prefix.len() - length)
        .map(|offset| prefix[offset + length] - prefix[offset])
        .collect();
    // One window is a whole number of ripple cycles only by chance, so what it
    // leaves behind still sways the minimum by a few samples. Averaging the
    // cost over one cycle takes that out, and does so symmetrically.
    let half = (sample_rate_hz as usize / (4 * SYNC_TONE_HZ as usize)).max(1);
    let smoothed = |offset: usize| {
        let from = offset.saturating_sub(half);
        let to = (offset + half + 1).min(cost.len());
        cost[from..to].iter().sum::<f64>() / (to - from) as f64
    };
    let (offset, total) = (0..cost.len())
        .map(|offset| (offset, smoothed(offset)))
        .min_by(|left, right| left.1.total_cmp(&right.1))?;
    if total / length as f64 >= SYNC_LEVEL_HZ {
        return None;
    }
    Some(start + offset as u64 + length as u64 / 2)
}

pub(super) fn push_bounded(history: &mut VecDeque<SyncObservation>, value: SyncObservation) {
    if history.len() == HISTORY_LEN {
        history.pop_front();
    }
    history.push_back(value);
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::mode::Mode;
    use crate::rx::input::DemodulatedBlock;

    #[test]
    fn refinement_finds_the_pulse_center_in_the_frequency_stream() {
        let profile = RasterProfile::for_mode(Mode::Martin2).unwrap();
        let rate = 48_000;
        let delay_ps = 6_000_000_000;
        let count = 4_000;
        let mut frequency = vec![1_900.0_f32; count];
        let mut sync = vec![0.0_f32; count];
        let pulse = (f64::from(rate) * profile.sync_duration_ps as f64 / 1.0e12) as usize;
        let start = 1_000;
        frequency[start..start + pulse].fill(1_200.0);
        let envelope = start + pulse / 2 + (f64::from(rate) * 8.0e9 / 1.0e12) as usize;
        sync[envelope - 40..envelope + 40].fill(1.0);
        let mut input = SampleBuffer::new(0);
        input.append(DemodulatedBlock::new(0, &frequency, &sync), count);

        let center = refine_center(
            &input,
            profile,
            envelope as u64,
            rate,
            SstvDuration::from_picos(delay_ps),
        );
        assert_eq!(center, Some((start + pulse / 2) as u64));
    }

    /// The discriminator ripples at twice the tone, which puts the porch either
    /// side of a pulse below the sync level for part of every cycle. Reading the
    /// edges off the raw stream would stretch the pulse into that porch.
    #[test]
    fn refinement_survives_discriminator_ripple_around_the_pulse() {
        let profile = RasterProfile::for_mode(Mode::Scottie2).unwrap();
        let rate = 48_000;
        let count = 8_000;
        let pulse = (f64::from(rate) * profile.sync_duration_ps as f64 / 1.0e12) as usize;
        let porch = (f64::from(rate) * 0.0015) as usize;
        let start = 3_000;
        // Only the trailing porch sits at the level the ripple drags under the
        // sync threshold, as in a Scottie unit, so the stretch is one-sided.
        let mut frequency = vec![1_900.0_f32; count];
        frequency[start..start + pulse].fill(1_200.0);
        frequency[start + pulse..start + pulse + porch].fill(1_500.0);
        for (index, value) in frequency.iter_mut().enumerate() {
            let phase = core::f32::consts::TAU * 2_400.0 * index as f32 / rate as f32;
            *value += 250.0 * libm::sinf(phase);
        }
        let mut sync = vec![0.0_f32; count];
        let envelope = start + pulse / 2 + (f64::from(rate) * 8.0e-3) as usize;
        sync[envelope - 40..envelope + 40].fill(1.0);
        let mut input = SampleBuffer::new(0);
        input.append(DemodulatedBlock::new(0, &frequency, &sync), count);

        let center = refine_center(
            &input,
            profile,
            envelope as u64,
            rate,
            SstvDuration::from_picos(6_000_000_000),
        )
        .expect("a rippling pulse is still a pulse");
        assert!(
            center.abs_diff((start + pulse / 2) as u64) <= 4,
            "center landed at {center} instead of {}",
            start + pulse / 2
        );
    }
}
