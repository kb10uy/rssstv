use rssstv_sstv::mode::Mode;

/// How much of the header framing a VIS detection requires.
///
/// MMSSTV arms its VIS decoder on the start bit alone and never checks the
/// leader, which is what lets it read a header through noise and catch a
/// transmission joined mid-leader. The strict variant keeps the same decoder
/// but admits a start bit only after recent leader tone, for an operator who
/// would rather miss a header than start on a false one.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum VisDetection {
    /// A start bit alone arms the decoder, as in MMSSTV.
    #[default]
    Loose,
    /// A start bit arms the decoder only after recent leader tone.
    Strict,
}

/// Synchronization dominance that counts towards the start-bit trigger.
const START_STRENGTH: f64 = 0.55;

/// How long the start bit has to hold before the decoder arms.
///
/// Longer than the 10 ms break between the leaders, so the break alone cannot
/// arm it, and short enough into the 30 ms bit that the bit cells counted from
/// the trigger's own edge still sample every bit inside its hold time.
const START_TRIGGER_SECONDS: f64 = 0.015;

/// Dominance dropout a trigger run survives.
const START_DROPOUT_SECONDS: f64 = 0.002;

/// How long one VIS bit is held.
const CELL_SECONDS: f64 = 0.030;

/// Accumulated leader-dominant time the strict gate requires.
const LEADER_EVIDENCE_SECONDS: f64 = 0.1;

/// Ceiling on retained leader-dominant time.
///
/// The evidence drains one sample per non-leader sample, so the ceiling is
/// what bounds how long stale evidence can outlive the tone that earned it.
const LEADER_EVIDENCE_CAP_SECONDS: f64 = 0.3;

#[derive(Clone, Copy)]
enum VisState {
    Search,
    Cells { sample: usize, cell: usize },
}

pub(crate) struct VisDecoder {
    detection: VisDetection,
    sample_rate_hz: f64,
    state: VisState,
    run: usize,
    dropout: usize,
    leader_evidence: usize,
    /// Accumulated 1080, 1200, 1320, and 1900 Hz envelope per cell.
    cell_sums: [[f64; 4]; 10],
}

impl VisDecoder {
    pub(crate) fn new(sample_rate_hz: f64) -> Self {
        Self {
            detection: VisDetection::default(),
            sample_rate_hz,
            state: VisState::Search,
            run: 0,
            dropout: 0,
            leader_evidence: 0,
            cell_sums: [[0.0; 4]; 10],
        }
    }

    pub(crate) fn set_detection(&mut self, detection: VisDetection) {
        if self.detection != detection {
            self.detection = detection;
            self.state = VisState::Search;
            self.run = 0;
            self.dropout = 0;
            self.leader_evidence = 0;
        }
    }

    /// Feeds one sample of detector envelopes and the normalized sync
    /// dominance, and reports a mode when a whole header has been read.
    pub(crate) fn process(&mut self, envelope: [f64; 5], sync_strength: f64) -> Option<Mode> {
        let dominant_leader = envelope[3] > envelope[0]
            && envelope[3] > envelope[1]
            && envelope[3] > envelope[2]
            && envelope[3] > envelope[4];
        let cap = (self.sample_rate_hz * LEADER_EVIDENCE_CAP_SECONDS) as usize;
        self.leader_evidence = if dominant_leader {
            (self.leader_evidence + 1).min(cap)
        } else {
            self.leader_evidence.saturating_sub(1)
        };
        match self.state {
            VisState::Search => {
                self.track_start_run(sync_strength);
                let trigger = (self.sample_rate_hz * START_TRIGGER_SECONDS) as usize;
                if self.run == trigger && self.armed() {
                    self.cell_sums = [[0.0; 4]; 10];
                    self.state = VisState::Cells {
                        sample: trigger,
                        cell: 0,
                    };
                    self.run = 0;
                    self.dropout = 0;
                }
                None
            }
            VisState::Cells { sample, cell } => {
                let cell_samples = (self.sample_rate_hz * CELL_SECONDS).round() as usize;
                if sample >= cell_samples / 4 && sample < cell_samples * 3 / 4 {
                    for (sum, value) in self.cell_sums[cell].iter_mut().zip(envelope) {
                        *sum += value;
                    }
                }
                let next_sample = sample + 1;
                if next_sample < cell_samples {
                    self.state = VisState::Cells {
                        sample: next_sample,
                        cell,
                    };
                    return None;
                }
                if !self.cell_is_legible(cell) {
                    // The header was not one. Searching resumes from here
                    // rather than at the end of the ten cells, so a picture
                    // that armed the decoder costs one cell of deafness
                    // instead of three hundred milliseconds of it.
                    self.state = VisState::Search;
                    self.run = 0;
                    self.dropout = 0;
                    return None;
                }
                if cell < 9 {
                    self.state = VisState::Cells {
                        sample: 0,
                        cell: cell + 1,
                    };
                    return None;
                }
                self.state = VisState::Search;
                self.leader_evidence = 0;
                self.finish_cells()
            }
        }
    }

    /// Tracks how long sync dominance has held, riding out brief dropouts.
    fn track_start_run(&mut self, sync_strength: f64) {
        if sync_strength >= START_STRENGTH {
            self.run += 1;
            self.dropout = 0;
        } else if self.run > 0 {
            self.dropout += 1;
            if self.dropout as f64 > self.sample_rate_hz * START_DROPOUT_SECONDS {
                self.run = 0;
                self.dropout = 0;
            } else {
                self.run += 1;
            }
        }
    }

    fn armed(&self) -> bool {
        match self.detection {
            VisDetection::Loose => true,
            VisDetection::Strict => {
                self.leader_evidence as f64 >= self.sample_rate_hz * LEADER_EVIDENCE_SECONDS
            }
        }
    }

    /// Reports whether a completed cell names the tone the header calls for.
    ///
    /// Arming on a start bit alone is cheap, so it is the cells rather than the
    /// trigger that keep a picture from being read as a header. MMSSTV throws a
    /// header away the moment a cell stops looking like one: a bit whose two
    /// detectors are too close to separate, or one the 1900 Hz leader detector
    /// beats outright, ends the attempt. That is what its own 20 ms
    /// synchronization pulses run into, which is why a picture arms its decoder
    /// on every line without ever producing a mode.
    ///
    /// A cell is held to the dominance the trigger itself asked for, rather
    /// than to the original's absolute levels: these envelopes are normalized
    /// by the front end's peak follower and carry no comparable scale.
    fn cell_is_legible(&self, cell: usize) -> bool {
        let sums = self.cell_sums[cell];
        let (dominant, competing) = if cell == 0 || cell == 9 {
            // Framing: sync tone against everything that is not sync tone.
            (sums[1], sums[0].max(sums[2]).max(sums[3]))
        } else {
            // A bit: whichever bit tone won, against the other and the leader.
            (sums[0].max(sums[2]), sums[0].min(sums[2]).max(sums[3]))
        };
        dominant / (dominant + competing).max(f64::MIN_POSITIVE) >= START_STRENGTH
    }

    fn finish_cells(&self) -> Option<Mode> {
        let mut raw = 0_u8;
        for bit in 0..8 {
            let sums = self.cell_sums[bit + 1];
            if sums[0] > sums[2] {
                raw |= 1 << bit;
            }
        }
        Mode::from_raw_vis(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const RATE: f64 = 8_000.0;

    /// Detector envelopes for one steady tone, in the front end's order:
    /// 1080, 1200, 1320, 1900, and 2100 Hz.
    fn envelope(frequency_hz: f64) -> [f64; 5] {
        let centers = [1_080.0, 1_200.0, 1_320.0, 1_900.0, 2_100.0];
        centers.map(|center| {
            let distance = (frequency_hz - center).abs();
            (1.0 - distance / 250.0).max(0.02)
        })
    }

    fn strength(envelope: [f64; 5]) -> f64 {
        let competing = envelope[0]
            .max(envelope[2])
            .max(envelope[3])
            .max(envelope[4])
            .max(1.0e-6);
        (envelope[1] / (envelope[1] + competing)).clamp(0.0, 1.0)
    }

    fn feed(decoder: &mut VisDecoder, frequency_hz: f64, seconds: f64) -> Option<Mode> {
        let mut detected = None;
        for _ in 0..(RATE * seconds).round() as usize {
            let envelope = envelope(frequency_hz);
            if let Some(mode) = decoder.process(envelope, strength(envelope)) {
                detected.get_or_insert(mode);
            }
        }
        detected
    }

    fn feed_vis_bits(decoder: &mut VisDecoder, raw: u8) -> Option<Mode> {
        let mut detected = feed(decoder, 1_200.0, 0.030);
        for bit in 0..8 {
            let frequency = if raw & (1 << bit) != 0 {
                1_100.0
            } else {
                1_300.0
            };
            detected = detected.or(feed(decoder, frequency, 0.030));
        }
        detected.or(feed(decoder, 1_200.0, 0.030))
    }

    fn feed_full_header(decoder: &mut VisDecoder, raw: u8) -> Option<Mode> {
        let mut detected = feed(decoder, 1_900.0, 0.3);
        detected = detected.or(feed(decoder, 1_200.0, 0.010));
        detected = detected.or(feed(decoder, 1_900.0, 0.3));
        detected.or(feed_vis_bits(decoder, raw))
    }

    #[rstest]
    #[case(VisDetection::Loose)]
    #[case(VisDetection::Strict)]
    fn a_full_header_is_detected(#[case] detection: VisDetection) {
        let mut decoder = VisDecoder::new(RATE);
        decoder.set_detection(detection);
        assert_eq!(feed_full_header(&mut decoder, 0xac), Some(Mode::Martin1));
    }

    /// A reception joined after the leaders still carries the whole bit frame,
    /// which is all MMSSTV needs and all the loose gate asks for.
    #[test]
    fn loose_detection_reads_a_header_without_leaders() {
        let mut decoder = VisDecoder::new(RATE);
        feed(&mut decoder, 1_500.0, 0.2);
        assert_eq!(feed_vis_bits(&mut decoder, 0x88), Some(Mode::Robot36));
    }

    #[test]
    fn strict_detection_ignores_a_header_without_leaders() {
        let mut decoder = VisDecoder::new(RATE);
        decoder.set_detection(VisDetection::Strict);
        feed(&mut decoder, 1_500.0, 0.2);
        assert_eq!(feed_vis_bits(&mut decoder, 0x88), None);
    }

    /// The old detector reset on any sample that broke a strict five-way
    /// dominance, so a leader had to be perfect for its whole length. The
    /// evidence gate only accumulates, so a noisy leader still opens it.
    #[test]
    fn strict_detection_survives_a_noisy_leader() {
        let mut decoder = VisDecoder::new(RATE);
        decoder.set_detection(VisDetection::Strict);
        let mut detected = None;
        for index in 0..(RATE * 0.3) as usize {
            let envelope = if index % 5 == 0 {
                [0.9, 0.9, 0.9, 0.2, 0.9]
            } else {
                envelope(1_900.0)
            };
            detected = detected.or(decoder.process(envelope, strength(envelope)));
        }
        detected = detected.or(feed(&mut decoder, 1_200.0, 0.010));
        detected = detected.or(feed(&mut decoder, 1_900.0, 0.3));
        assert_eq!(
            detected.or(feed_vis_bits(&mut decoder, 0xac)),
            Some(Mode::Martin1)
        );
    }

    /// The trigger asks for more 1200 Hz than the break holds, so the break
    /// cannot arm the cells and misalign them onto the second leader.
    #[test]
    fn the_break_between_leaders_does_not_arm_the_decoder() {
        let mut decoder = VisDecoder::new(RATE);
        feed(&mut decoder, 1_900.0, 0.3);
        feed(&mut decoder, 1_200.0, 0.010);
        feed(&mut decoder, 1_900.0, 0.3);
        assert_eq!(feed_vis_bits(&mut decoder, 0xac), Some(Mode::Martin1));
    }

    /// A start bit is cheap to imitate, so an armed decoder that reads ten
    /// cells no matter what they hold will eventually spell a valid code out
    /// of a picture. Each cell is checked as it completes instead: a leader
    /// tone that beats both bit detectors, and a tone sitting between them
    /// that names neither, both end the attempt where they occur.
    #[rstest]
    #[case(1_900.0)]
    #[case(1_200.0)]
    fn a_cell_that_holds_no_bit_ends_the_attempt(#[case] frequency_hz: f64) {
        let mut decoder = VisDecoder::new(RATE);
        assert_eq!(feed(&mut decoder, 1_200.0, 0.030), None);
        assert_eq!(feed(&mut decoder, frequency_hz, 0.040), None);
        // Searching resumed at the failed cell rather than at the end of the
        // ten, so the header that follows is read from its own start bit.
        assert_eq!(feed_vis_bits(&mut decoder, 0xac), Some(Mode::Martin1));
    }

    #[test]
    fn a_start_bit_with_a_brief_dropout_still_arms_the_decoder() {
        let mut decoder = VisDecoder::new(RATE);
        feed(&mut decoder, 1_200.0, 0.005);
        feed(&mut decoder, 1_500.0, 0.001);
        let mut detected = feed(&mut decoder, 1_200.0, 0.024);
        for bit in 0..8 {
            let frequency = if 0xac & (1 << bit) != 0 {
                1_100.0
            } else {
                1_300.0
            };
            detected = detected.or(feed(&mut decoder, frequency, 0.030));
        }
        assert_eq!(
            detected.or(feed(&mut decoder, 1_200.0, 0.030)),
            Some(Mode::Martin1)
        );
    }

    #[test]
    fn an_invalid_code_is_rejected() {
        let mut decoder = VisDecoder::new(RATE);
        assert_eq!(feed_vis_bits(&mut decoder, 0x2c), None);
    }

    /// Raster synchronization pulses are far shorter than the trigger, so a
    /// picture's own pulse train never arms the decoder.
    #[test]
    fn short_sync_pulses_do_not_arm_the_decoder() {
        let mut decoder = VisDecoder::new(RATE);
        for _ in 0..16 {
            feed(&mut decoder, 1_200.0, 0.009);
            assert_eq!(feed(&mut decoder, 1_700.0, 0.140), None);
        }
    }
}
