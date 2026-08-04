use std::cmp::Reverse;

use rssstv_sstv::mode::{Mode, Support};

/// Measured sync intervals kept for mode matching, as in MMSSTV's `CSYNCINT`.
const INTERVAL_HISTORY: usize = 8;

/// Retained intervals that must agree before a mode is reported.
const REQUIRED_AGREEMENT: usize = 6;

/// Line multiples tested against each candidate period.
///
/// A sync pulse lost to noise leaves a gap a whole number of lines wide, so
/// reception can still begin when some pulses were missed.
const MAX_LINE_MULTIPLE: u64 = 3;

/// Fractional tolerance when matching an interval to a candidate period.
///
/// This has to be tighter than the closest pair of candidates: Martin 2 at two
/// lines is 1.6 % away from Martin 1 at one. It also has to be looser than a
/// receiver's clock error and the jitter in locating a pulse, both of which are
/// under a tenth of that.
const INTERVAL_TOLERANCE: f64 = 0.006;

/// Normalized sync strength at which a pulse is considered present.
///
/// The raster decoder extracts its acquisition pulse runs at the same level.
const PULSE_THRESHOLD: f64 = 0.5;

/// Shortest run accepted as a pulse rather than a noise spike, in seconds.
///
/// The narrowest sync pulse among the supported modes is a little over 4 ms.
const MIN_PULSE_SECONDS: f64 = 0.002;

/// Interval below which a second pulse is treated as part of the first.
///
/// The shortest candidate line period is Robot 36's 150 ms, so this is well
/// clear of any real interval while still merging a pulse that wobbles across
/// the threshold.
const MIN_INTERVAL_SECONDS: f64 = 0.05;

/// Which modes a reception may be started in without a VIS header.
///
/// A transmission that was joined after its header, or that never sent one,
/// can only be identified by its raster: the spacing of horizontal sync pulses
/// is the mode's line period. Detection from that spacing is deliberately
/// scoped by the caller, because a period can be matched by more than one mode
/// and an operator who has chosen a mode should not be overridden by a guess.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SyncStart {
    /// Only a VIS header starts a reception.
    #[default]
    Disabled,
    /// Any decode-supported mode whose line period matches.
    Any,
    /// This mode alone, and only when its line period matches.
    Only(Mode),
}

/// Ordering key for a candidate: agreement first, then the smallest multiple,
/// then the shortest period.
type MatchRank = (usize, Reverse<u64>, Reverse<u64>);

/// Mode detection from the spacing of horizontal sync pulses.
///
/// This is MMSSTV's `CSYNCINT`. It keeps a short history of measured intervals
/// and compares them against every candidate's line period at one-, two-, and
/// three-line multiples, so a reception can begin despite missed sync pulses or
/// a missed VIS header.
///
/// Only the spacing of pulses matters, so each is located by the start of its
/// run: the measurement is differential and a consistent reference point
/// cancels out.
pub(crate) struct SyncIntervalDetector {
    scope: SyncStart,
    min_pulse_samples: u64,
    min_interval_samples: u64,
    /// Length of the run currently above the threshold, zero when below it.
    run: u64,
    /// Whether the current run has already been accepted as a pulse.
    counted: bool,
    /// Samples since the accepted pulse that opened the current interval.
    since_pulse: u64,
    /// Whether any pulse has been accepted, which opens the first interval.
    started: bool,
    intervals: [u64; INTERVAL_HISTORY],
    filled: usize,
    next: usize,
}

impl SyncIntervalDetector {
    pub(crate) fn new(sample_rate_hz: f64) -> Self {
        Self {
            scope: SyncStart::default(),
            min_pulse_samples: (sample_rate_hz * MIN_PULSE_SECONDS) as u64,
            min_interval_samples: (sample_rate_hz * MIN_INTERVAL_SECONDS) as u64,
            run: 0,
            counted: false,
            since_pulse: 0,
            started: false,
            intervals: [0; INTERVAL_HISTORY],
            filled: 0,
            next: 0,
        }
    }

    pub(crate) fn set_scope(&mut self, scope: SyncStart) {
        if self.scope != scope {
            self.scope = scope;
            self.filled = 0;
            self.next = 0;
            self.started = false;
        }
    }

    /// Feeds one sync strength and reports a mode once the history agrees.
    pub(crate) fn process(&mut self, sample_rate_hz: f64, sync_strength: f64) -> Option<Mode> {
        if self.scope == SyncStart::Disabled {
            return None;
        }
        self.since_pulse = self.since_pulse.saturating_add(1);
        if sync_strength >= PULSE_THRESHOLD {
            self.run += 1;
            if !self.counted && self.run >= self.min_pulse_samples {
                self.counted = true;
                // The pulse is placed at the start of its run, which is where
                // the threshold was crossed `run` samples ago.
                return self.accept_pulse(sample_rate_hz);
            }
        } else {
            self.run = 0;
            self.counted = false;
        }
        None
    }

    fn accept_pulse(&mut self, sample_rate_hz: f64) -> Option<Mode> {
        let interval = self.since_pulse.saturating_sub(self.run);
        self.since_pulse = self.run;
        if !self.started {
            self.started = true;
            return None;
        }
        if interval < self.min_interval_samples {
            return None;
        }
        self.intervals[self.next] = interval;
        self.next = (self.next + 1) % INTERVAL_HISTORY;
        self.filled = (self.filled + 1).min(INTERVAL_HISTORY);
        if self.filled < REQUIRED_AGREEMENT {
            return None;
        }
        self.resolve(sample_rate_hz)
    }

    /// Returns the candidate whose period best explains the retained intervals.
    ///
    /// Ties are broken towards the smallest line multiple and then the shortest
    /// period, because the multiples exist to tolerate pulses that were missed:
    /// a period that explains the spacing directly is the stronger claim than
    /// one that has to assume a gap.
    fn resolve(&self, sample_rate_hz: f64) -> Option<Mode> {
        let mut best: Option<(MatchRank, Mode)> = None;
        for mode in candidates(self.scope) {
            let period = mode.spec().period().as_picos();
            let period_samples = period as f64 * sample_rate_hz / 1.0e12;
            for multiple in 1..=MAX_LINE_MULTIPLE {
                let expected = period_samples * multiple as f64;
                let tolerance = expected * INTERVAL_TOLERANCE;
                let agreed = self.intervals[..self.filled]
                    .iter()
                    .filter(|&&interval| (interval as f64 - expected).abs() <= tolerance)
                    .count();
                if agreed < REQUIRED_AGREEMENT {
                    continue;
                }
                let rank = (agreed, Reverse(multiple), Reverse(period));
                if best.is_none_or(|(best_rank, _)| rank > best_rank) {
                    best = Some((rank, mode));
                }
            }
        }
        best.map(|(_, mode)| mode)
    }
}

/// Returns the modes a sync-interval match may report.
fn candidates(scope: SyncStart) -> impl Iterator<Item = Mode> {
    let only = match scope {
        SyncStart::Only(mode) => Some(mode),
        _ => None,
    };
    Mode::ALL.into_iter().filter(move |mode| {
        mode.spec().decode_support() == Support::Supported
            && only.is_none_or(|selected| *mode == selected)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    /// Drives the interval detector with a pulse train whose gaps are given in
    /// seconds, and returns the first mode it reports.
    ///
    /// The train opens with a pulse, so `gaps` closes one interval each and
    /// [`REQUIRED_AGREEMENT`] of them are needed before anything is reported.
    fn detect_from_gaps(scope: SyncStart, rate: f64, gaps: &[f64]) -> Option<Mode> {
        let mut detector = SyncIntervalDetector::new(rate);
        detector.set_scope(scope);
        let pulse_samples = (rate * 0.006).round() as usize;
        let mut detected = None;
        let feed = |detector: &mut SyncIntervalDetector,
                    samples: usize,
                    strength: f64,
                    detected: &mut Option<Mode>| {
            for _ in 0..samples {
                if let Some(mode) = detector.process(rate, strength) {
                    detected.get_or_insert(mode);
                }
            }
        };
        feed(&mut detector, pulse_samples, 1.0, &mut detected);
        for gap in gaps {
            let quiet = (rate * gap).round() as usize - pulse_samples;
            feed(&mut detector, quiet, 0.0, &mut detected);
            feed(&mut detector, pulse_samples, 1.0, &mut detected);
        }
        detected
    }

    fn period_seconds(mode: Mode) -> f64 {
        mode.spec().period().as_picos() as f64 / 1.0e12
    }

    /// A transmission joined after its header has nothing but its raster to
    /// identify it, and the spacing of sync pulses is the mode's line period.
    #[rstest]
    #[case(Mode::Robot36)]
    #[case(Mode::Martin1)]
    #[case(Mode::Martin2)]
    #[case(Mode::Scottie1)]
    #[case(Mode::Pd120)]
    #[case(Mode::Pd290)]
    fn a_line_rate_pulse_train_identifies_its_mode(#[case] mode: Mode) {
        let gaps = vec![period_seconds(mode); INTERVAL_HISTORY];
        assert_eq!(detect_from_gaps(SyncStart::Any, 8_000.0, &gaps), Some(mode));
    }

    #[test]
    fn no_mode_is_reported_while_sync_start_is_disabled() {
        let gaps = vec![period_seconds(Mode::Martin1); INTERVAL_HISTORY];
        assert_eq!(detect_from_gaps(SyncStart::Disabled, 8_000.0, &gaps), None);
    }

    /// The operator's choice is confirmed, not overridden: a period that
    /// belongs to some other mode starts nothing.
    #[test]
    fn a_scoped_detector_ignores_a_period_that_is_not_its_own() {
        let gaps = vec![period_seconds(Mode::Martin1); INTERVAL_HISTORY];
        assert_eq!(
            detect_from_gaps(SyncStart::Only(Mode::Martin1), 8_000.0, &gaps),
            Some(Mode::Martin1)
        );
        assert_eq!(
            detect_from_gaps(SyncStart::Only(Mode::Scottie1), 8_000.0, &gaps),
            None
        );
    }

    /// Pulses lost to noise leave a gap a whole number of lines wide, which is
    /// why the multiples are tested at all.
    #[test]
    fn a_train_with_missed_pulses_still_identifies_its_mode() {
        let gaps = vec![period_seconds(Mode::Martin1) * 3.0; INTERVAL_HISTORY];
        assert_eq!(
            detect_from_gaps(SyncStart::Any, 8_000.0, &gaps),
            Some(Mode::Martin1)
        );
    }

    /// Robot 36 at two lines is exactly Robot 72 at one. Nothing in the signal
    /// separates them, so the reading that assumes no missed pulse wins.
    #[test]
    fn an_ambiguous_period_resolves_to_the_smallest_line_multiple() {
        let gaps = vec![period_seconds(Mode::Robot72); INTERVAL_HISTORY];
        assert_eq!(
            detect_from_gaps(SyncStart::Any, 8_000.0, &gaps),
            Some(Mode::Robot72)
        );
    }

    /// The tolerance has to separate the closest pair of candidates, which is
    /// Martin 2 at two lines against Martin 1 at one, 1.6 % apart.
    #[test]
    fn neighbouring_candidates_are_not_confused() {
        let gaps = vec![period_seconds(Mode::Martin2) * 2.0; INTERVAL_HISTORY];
        assert_eq!(
            detect_from_gaps(SyncStart::Any, 8_000.0, &gaps),
            Some(Mode::Martin2)
        );
    }

    /// Random spacing is not a raster.
    #[test]
    fn unrelated_intervals_report_nothing() {
        let gaps = [0.21, 0.33, 0.19, 0.41, 0.27, 0.36, 0.23, 0.31];
        assert_eq!(detect_from_gaps(SyncStart::Any, 8_000.0, &gaps), None);
    }

    /// A receiver's clock is never exact, so a match cannot demand one.
    #[test]
    fn a_mistimed_raster_is_still_identified() {
        let gaps = vec![period_seconds(Mode::Scottie1) * 1.0003; INTERVAL_HISTORY];
        assert_eq!(
            detect_from_gaps(SyncStart::Any, 8_000.0, &gaps),
            Some(Mode::Scottie1)
        );
    }
}
