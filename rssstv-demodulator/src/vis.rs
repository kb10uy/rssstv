use rssstv_sstv::mode::Mode;

/// Shortest leader tone that opens or closes the VIS break.
const LEADER_MIN_SECONDS: f64 = 0.22;

/// Shortest break tone that separates the two leaders.
const BREAK_MIN_SECONDS: f64 = 0.004;

/// Longest break tone that still separates them rather than being a sync pulse.
const BREAK_MAX_SECONDS: f64 = 0.030;

/// How long one VIS bit is held.
const CELL_SECONDS: f64 = 0.030;

#[derive(Clone, Copy)]
enum VisState {
    Search,
    FirstLeader { samples: usize },
    Break { samples: usize },
    SecondLeader { samples: usize },
    Cells { sample: usize, cell: usize },
}

pub(crate) struct VisDecoder {
    state: VisState,
    sample_rate_hz: f64,
    cell_sums: [[f64; 3]; 10],
}

impl VisDecoder {
    pub(crate) fn new(sample_rate_hz: f64) -> Self {
        Self {
            state: VisState::Search,
            sample_rate_hz,
            cell_sums: [[0.0; 3]; 10],
        }
    }

    pub(crate) fn process(&mut self, envelope: [f64; 5]) -> Option<Mode> {
        let dominant_1900 = envelope[3] > envelope[1]
            && envelope[3] > envelope[0]
            && envelope[3] > envelope[2]
            && envelope[3] > envelope[4];
        let dominant_1200 = envelope[1] > envelope[3]
            && envelope[1] > envelope[0]
            && envelope[1] > envelope[2]
            && envelope[1] > envelope[4];
        let leader_min = (self.sample_rate_hz * LEADER_MIN_SECONDS) as usize;
        let break_min = (self.sample_rate_hz * BREAK_MIN_SECONDS) as usize;
        let break_max = (self.sample_rate_hz * BREAK_MAX_SECONDS) as usize;
        self.state = match self.state {
            VisState::Search if dominant_1900 => VisState::FirstLeader { samples: 1 },
            VisState::Search => VisState::Search,
            VisState::FirstLeader { samples } if dominant_1900 => VisState::FirstLeader {
                samples: samples + 1,
            },
            VisState::FirstLeader { samples } if dominant_1200 && samples >= leader_min => {
                VisState::Break { samples: 1 }
            }
            VisState::FirstLeader { .. } => VisState::Search,
            VisState::Break { samples } if dominant_1200 && samples < break_max => {
                VisState::Break {
                    samples: samples + 1,
                }
            }
            VisState::Break { samples } if dominant_1900 && samples >= break_min => {
                VisState::SecondLeader { samples: 1 }
            }
            VisState::Break { .. } => VisState::Search,
            VisState::SecondLeader { samples } if dominant_1900 => VisState::SecondLeader {
                samples: samples + 1,
            },
            VisState::SecondLeader { samples } if dominant_1200 && samples >= leader_min => {
                self.cell_sums = [[0.0; 3]; 10];
                VisState::Cells { sample: 0, cell: 0 }
            }
            VisState::SecondLeader { .. } => VisState::Search,
            VisState::Cells { sample, cell } => {
                let cell_samples = (self.sample_rate_hz * CELL_SECONDS).round() as usize;
                let within = sample % cell_samples;
                if within >= cell_samples / 4 && within < cell_samples * 3 / 4 {
                    self.cell_sums[cell][0] += envelope[0];
                    self.cell_sums[cell][1] += envelope[1];
                    self.cell_sums[cell][2] += envelope[2];
                }
                let next_sample = sample + 1;
                if next_sample == cell_samples {
                    if cell == 9 {
                        let mode = self.finish_cells();
                        self.state = VisState::Search;
                        return mode;
                    }
                    VisState::Cells {
                        sample: 0,
                        cell: cell + 1,
                    }
                } else {
                    VisState::Cells {
                        sample: next_sample,
                        cell,
                    }
                }
            }
        };
        None
    }

    fn finish_cells(&self) -> Option<Mode> {
        if self.cell_sums[0][1] <= self.cell_sums[0][0].max(self.cell_sums[0][2])
            || self.cell_sums[9][1] <= self.cell_sums[9][0].max(self.cell_sums[9][2])
        {
            return None;
        }
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
