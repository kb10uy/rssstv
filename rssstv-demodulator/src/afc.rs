use std::collections::VecDeque;

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
            offsets: VecDeque::with_capacity(15),
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
        if sync_strength >= 0.58 {
            self.active = true;
            self.run_samples += 1;
            let expected = 1_200.0 + self.offset_hz;
            if let Some(value) = measurement.filter(|value| (value - expected).abs() <= 100.0) {
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
            || !(0.003..=0.050).contains(&duration)
            || self.measurements.len() < 2
        {
            self.measurements.clear();
            return false;
        }
        self.measurements.sort_by(f64::total_cmp);
        let measured = self.measurements.iter().sum::<f64>() / self.measurements.len() as f64;
        self.measurements.clear();
        let offset = measured - 1_200.0;
        if offset.abs() > 150.0 {
            return false;
        }
        if self.offsets.len() == 15 {
            self.offsets.pop_front();
        }
        self.offsets.push_back(offset);
        self.offset_hz = self.offsets.iter().sum::<f64>() / 15.0;
        self.inhibit_samples = (self.sample_rate_hz * 0.1) as usize;
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
