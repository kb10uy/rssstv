use std::collections::VecDeque;

use rssstv_sstv::signal::SYNC_HZ;

/// Sync-envelope strength a pulse has to reach before it is measured.
const SYNC_THRESHOLD: f64 = 0.58;

/// How far from the expected sync frequency a measurement may sit.
///
/// A discriminator reading further out than this belongs to something other
/// than the sync pulse being measured, so it is not averaged into the offset.
const MEASUREMENT_WINDOW_HZ: f64 = 100.0;

/// How long a run above the threshold has to last to count as a sync pulse.
const RUN_SECONDS: core::ops::RangeInclusive<f64> = 0.003..=0.050;

/// Measurements one run needs before its average is trusted.
const RUN_MEASUREMENTS: usize = 2;

/// The largest correction a single run may propose.
const MAX_OFFSET_HZ: f64 = 150.0;

/// How many runs the offset is smoothed over.
///
/// The sum is divided by this whether or not that many runs have arrived, so
/// the correction ramps in over the first full history rather than jumping to
/// the first measurement.
const HISTORY: usize = 15;

/// How long after an update the next one is held off.
const INHIBIT_SECONDS: f64 = 0.1;

pub(crate) struct Afc {
    enabled: bool,
    sample_rate_hz: f64,
    active: bool,
    run_samples: usize,
    measurements: Vec<f64>,
    offsets: VecDeque<f64>,
    offset_hz: f64,
    inhibit_samples: usize,
}

impl Afc {
    pub(crate) fn new(sample_rate_hz: f64) -> Self {
        Self {
            enabled: false,
            sample_rate_hz,
            active: false,
            run_samples: 0,
            measurements: Vec::new(),
            offsets: VecDeque::with_capacity(HISTORY),
            offset_hz: 0.0,
            inhibit_samples: 0,
        }
    }

    pub(crate) fn process(&mut self, sync_strength: f64, measurement: Option<f64>) -> bool {
        if !self.enabled {
            return false;
        }
        if self.inhibit_samples > 0 {
            self.inhibit_samples -= 1;
        }
        if sync_strength >= SYNC_THRESHOLD {
            self.active = true;
            self.run_samples += 1;
            // A run longer than a sync pulse is something else, a carrier or
            // steady interference, and is discarded whole when it ends. Its
            // measurements stop being kept the moment it is too long, so a
            // tone that never ends cannot grow the buffer without bound.
            if self.run_samples as f64 / self.sample_rate_hz > *RUN_SECONDS.end() {
                self.measurements.clear();
                return false;
            }
            let expected = f64::from(SYNC_HZ) + self.offset_hz;
            if let Some(value) =
                measurement.filter(|value| (value - expected).abs() <= MEASUREMENT_WINDOW_HZ)
            {
                self.measurements.push(value);
            }
            false
        } else if self.active {
            self.finish_run()
        } else {
            false
        }
    }

    pub(crate) fn finish_run(&mut self) -> bool {
        if !self.active {
            return false;
        }
        self.active = false;
        let duration = self.run_samples as f64 / self.sample_rate_hz;
        self.run_samples = 0;
        if self.inhibit_samples > 0
            || !RUN_SECONDS.contains(&duration)
            || self.measurements.len() < RUN_MEASUREMENTS
        {
            self.measurements.clear();
            return false;
        }
        let measured = self.measurements.iter().sum::<f64>() / self.measurements.len() as f64;
        self.measurements.clear();
        let offset = measured - f64::from(SYNC_HZ);
        if offset.abs() > MAX_OFFSET_HZ {
            return false;
        }
        if self.offsets.len() == HISTORY {
            self.offsets.pop_front();
        }
        self.offsets.push_back(offset);
        self.offset_hz = self.offsets.iter().sum::<f64>() / HISTORY as f64;
        self.inhibit_samples = (self.sample_rate_hz * INHIBIT_SECONDS) as usize;
        true
    }

    pub(crate) const fn enable(&mut self) {
        self.enabled = true;
    }

    pub(crate) const fn offset_hz(&self) -> f64 {
        self.offset_hz
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A station tuning up sends the sync frequency for as long as it likes,
    /// and the receiver has to survive that on a fixed amount of memory.
    #[test]
    fn a_persistent_tone_does_not_grow_the_measurement_buffer() {
        let rate = 8_000.0;
        let mut afc = Afc::new(rate);
        afc.enable();
        for _ in 0..(rate * 10.0) as usize {
            afc.process(0.9, Some(1_210.0));
        }
        let bound = 2 * ((rate * RUN_SECONDS.end()) as usize + 1);
        assert!(
            afc.measurements.capacity() <= bound,
            "the buffer grew to {} measurements",
            afc.measurements.capacity()
        );
        assert!(
            !afc.finish_run(),
            "an overlong run must not update the offset"
        );
        assert_eq!(afc.offset_hz(), 0.0);
    }

    #[test]
    fn tracks_repeated_offset_sync_pulses() {
        let rate = 8_000.0;
        let mut afc = Afc::new(rate);
        afc.enable();
        for _ in 0..24 {
            for sample in 0..(rate * 0.009) as usize {
                afc.process(0.8, (sample % 3 == 0).then_some(1_240.0));
            }
            for _ in 0..(rate * 0.11) as usize {
                afc.process(0.1, None);
            }
        }
        assert!(
            (afc.offset_hz() - 40.0).abs() < 1.0,
            "offset was {} Hz",
            afc.offset_hz()
        );
    }
}
