use core::num::NonZeroU32;
use rssstv_dsp::{
    filter::{Fir, FirDesign, FirKind, Iir, IirLowPassDesign, IirResponse, Resonator},
    frequency::ZeroCrossingFrequency,
};

use rssstv_fskid::{FskDecoder, FskRecord, FskTone};
use rssstv_sstv::mode::Mode;

use crate::{
    DemodulatorError,
    afc::Afc,
    hilbert::HilbertDiscriminator,
    sync::{SyncIntervalDetector, SyncStart},
    vis::VisDecoder,
};

const DETECTORS: [(f64, f64); 5] = [
    (1_080.0, 80.0),
    (1_200.0, 100.0),
    (1_320.0, 80.0),
    (1_900.0, 100.0),
    (2_100.0, 100.0),
];
const FSK_MINIMUM_CONTRAST: f64 = 0.125;

/// What identified the mode of a reception.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Detection {
    /// A conventional VIS header, which names the mode outright.
    Header,
    /// The spacing of the raster's own sync pulses, with no header to read.
    SyncSpacing,
}

pub(crate) struct FrontEndOutput {
    pub(crate) frequency_hz: f64,
    pub(crate) sync_strength: f64,
    pub(crate) mode: Option<(Mode, Detection)>,
    pub(crate) fsk_record: Option<FskRecord>,
}

pub(crate) struct FrontEnd {
    previous_input: f64,
    band_pass: Fir,
    hilbert: HilbertDiscriminator,
    zero_crossing: ZeroCrossingFrequency,
    level_peak: f64,
    level_decay: f64,
    detectors: Vec<ToneDetector>,
    vis: VisDecoder,
    sync_intervals: SyncIntervalDetector,
    sample_rate_hz: f64,
    fsk: FskDecoder,
    afc: Afc,
}

impl FrontEnd {
    pub(crate) fn new(sample_rate: u32) -> Result<Self, DemodulatorError> {
        let fsk = FskDecoder::new(
            NonZeroU32::new(sample_rate).ok_or(DemodulatorError::SampleRateTooLow(sample_rate))?,
        );
        let sample_rate_hz = f64::from(sample_rate);
        let mut order = (24.0 * sample_rate_hz / 11_025.0).round() as usize;
        order = order.max(12);
        if !order.is_multiple_of(2) {
            order += 1;
        }
        let upper = 2_600.0_f64.min(sample_rate_hz * 0.5 - 100.0);
        let band_pass = Fir::from_design(FirDesign {
            kind: FirKind::BandPass,
            order,
            sample_rate_hz,
            lower_frequency_hz: 1_000.0,
            upper_frequency_hz: upper,
            attenuation_db: 20.0,
            gain: 1.0,
        })?;
        let detectors = DETECTORS
            .into_iter()
            .map(|(frequency, bandwidth)| ToneDetector::new(frequency, bandwidth, sample_rate_hz))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            previous_input: 0.0,
            band_pass,
            hilbert: HilbertDiscriminator::new(sample_rate_hz)?,
            zero_crossing: ZeroCrossingFrequency::new(sample_rate_hz)?,
            level_peak: 1.0e-6,
            level_decay: (-1.0 / (sample_rate_hz * 0.1)).exp(),
            detectors,
            vis: VisDecoder::new(sample_rate_hz),
            sync_intervals: SyncIntervalDetector::new(sample_rate_hz),
            sample_rate_hz,
            fsk,
            afc: Afc::new(sample_rate_hz),
        })
    }

    pub(crate) fn process(&mut self, input: f64) -> Result<FrontEndOutput, DemodulatorError> {
        let averaged = (input + self.previous_input) * 0.5;
        self.previous_input = input;
        let filtered = self.band_pass.process_sample(averaged);

        self.level_peak = (self.level_peak * self.level_decay).max(filtered.abs());
        let detector_input = (filtered / self.level_peak.max(1.0e-6)).clamp(-1.0, 1.0);
        let mut envelopes = [0.0; 5];
        for (envelope, detector) in envelopes.iter_mut().zip(&mut self.detectors) {
            *envelope = detector.process(detector_input);
        }
        let competing = envelopes[0]
            .max(envelopes[2])
            .max(envelopes[3])
            .max(envelopes[4])
            .max(1.0e-6);
        let sync_strength = (envelopes[1] / (envelopes[1] + competing)).clamp(0.0, 1.0);
        // A header identifies the mode outright, so it is preferred over an
        // inference drawn from the raster's timing.
        let mode = self
            .vis
            .process(envelopes)
            .map(|mode| (mode, Detection::Header))
            .or_else(|| {
                self.sync_intervals
                    .process(self.sample_rate_hz, sync_strength)
                    .map(|mode| (mode, Detection::SyncSpacing))
            });
        let difference = (envelopes[3] - envelopes[4]).abs();
        let fsk_tone = if difference < FSK_MINIMUM_CONTRAST {
            FskTone::Ambiguous
        } else if envelopes[3] > envelopes[4] {
            FskTone::Mark
        } else {
            FskTone::Space
        };
        let fsk_record = self.fsk.process(fsk_tone);

        let measured = self.zero_crossing.process_sample(filtered);
        let changed = self.afc.process(sync_strength, measured);
        if changed {
            for (detector, (nominal, _)) in self.detectors.iter_mut().zip(DETECTORS) {
                detector.retune(nominal + self.afc.offset_hz())?;
            }
        }

        let frequency = self.hilbert.process(filtered);
        Ok(FrontEndOutput {
            frequency_hz: (frequency - self.afc.offset_hz()).clamp(0.0, 3_000.0),
            sync_strength,
            mode,
            fsk_record,
        })
    }

    pub(crate) const fn enable_afc(&mut self) {
        self.afc.enable();
    }

    pub(crate) fn set_sync_start(&mut self, scope: SyncStart) {
        self.sync_intervals.set_scope(scope);
    }

    pub(crate) const fn offset_hz(&self) -> f64 {
        self.afc.offset_hz()
    }

    pub(crate) fn finish_afc(&mut self) -> Result<(), DemodulatorError> {
        if self.afc.finish_run() {
            for (detector, (nominal, _)) in self.detectors.iter_mut().zip(DETECTORS) {
                detector.retune(nominal + self.afc.offset_hz())?;
            }
        }
        Ok(())
    }
}

struct ToneDetector {
    resonator: Resonator,
    envelope: Iir,
}

impl ToneDetector {
    fn new(
        frequency_hz: f64,
        bandwidth_hz: f64,
        sample_rate_hz: f64,
    ) -> Result<Self, rssstv_dsp::DspError> {
        Ok(Self {
            resonator: Resonator::new(sample_rate_hz, frequency_hz, bandwidth_hz)?,
            envelope: Iir::from_low_pass(IirLowPassDesign {
                order: 2,
                sample_rate_hz,
                cutoff_hz: 50.0,
                response: IirResponse::Butterworth,
            })?,
        })
    }

    fn process(&mut self, sample: f64) -> f64 {
        self.envelope
            .process_sample(self.resonator.process_sample(sample).abs())
            .max(0.0)
    }

    fn retune(&mut self, frequency_hz: f64) -> Result<(), rssstv_dsp::DspError> {
        self.resonator.set_frequency(frequency_hz)
    }
}
