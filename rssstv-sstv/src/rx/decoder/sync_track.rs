//! Following the sync pulse while a reception is running.
//!
//! Where the raster sits is decided once at acquisition and then held; these
//! watch each pulse against it, nudge the phase when it has drifted, and stop
//! a reception that has lost the signal altogether.

use super::*;

impl RxDecoder {
    pub(super) fn synchronize(&mut self) -> Result<bool, SstvError> {
        let unit = self.decode.raster_unit;
        let observation = observe(
            self.input.as_ref().expect("input initialized"),
            self.profile,
            self.decode.clock.expect("clock acquired"),
            unit,
            self.sample_rate_hz,
            self.config.sync_detector_delay,
        );
        let Some(observation) = observation else {
            self.phase_displacements.clear();
            return self.note_bad_sync();
        };
        push_bounded(&mut self.observations, observation);
        self.staged_observations.push(observation);
        if observation.confidence < MIN_CONFIDENCE {
            self.phase_displacements.clear();
            return self.note_bad_sync();
        }
        let expected_protocol = self
            .profile
            .period_ps
            .checked_mul(unit as u64)
            .and_then(|value| value.checked_add(self.profile.sync_center_ps))
            .ok_or(SstvError::TimeOverflow)?;
        let expected = self
            .decode
            .clock
            .expect("clock acquired")
            .sample_at(expected_protocol)?;
        let displacement = observation.center_sample as i128 - expected as i128;
        let max_consistent = self
            .decode
            .clock
            .expect("clock acquired")
            .samples_for(self.profile.period_ps)?
            / 4;
        if displacement.unsigned_abs() > u128::from(max_consistent.max(2)) {
            self.phase_displacements.clear();
            return self.note_bad_sync();
        }
        let displacement =
            i64::try_from(displacement).map_err(|_| SstvError::SamplePositionOverflow)?;
        self.note_good_sync();
        if self.config.live_sync {
            if self.phase_displacements.len() == PHASE_AGREEMENT {
                self.phase_displacements.pop_front();
            }
            self.phase_displacements.push_back(displacement);
            let holdoff_done = self
                .last_phase_adjustment
                .is_none_or(|last| unit >= last + PHASE_HOLDOFF_UNITS);
            if self.phase_displacements.len() == PHASE_AGREEMENT && holdoff_done {
                let minimum = *self.phase_displacements.iter().min().expect("non-empty");
                let maximum = *self.phase_displacements.iter().max().expect("non-empty");
                let correction =
                    self.phase_displacements.iter().sum::<i64>() / PHASE_AGREEMENT as i64;
                if maximum - minimum <= 2 && correction.unsigned_abs() >= MIN_PHASE_DISPLACEMENT {
                    self.decode
                        .clock
                        .as_mut()
                        .expect("clock acquired")
                        .adjust_epoch(correction)?;
                    self.last_phase_adjustment = Some(unit);
                    self.phase_displacements.clear();
                    self.queue_event(RxEvent::PhaseAdjusted {
                        unit,
                        displacement_samples: correction,
                        source_epoch: self.decode.clock.expect("clock acquired").source_epoch(),
                    });
                }
            }
        }
        Ok(true)
    }
    pub(super) fn check_auto_stop(&mut self) -> Result<bool, SstvError> {
        if self.config.auto_stop && self.bad_sync_score >= BAD_SYNC_SCORE_LIMIT {
            let reason = StopReason::SynchronizationLost;
            self.decode.state = RxState::Stopped {
                completed_rows: self.decode.delivered_rows,
                reason,
            };
            self.queue_event(RxEvent::Stopped { reason });
            Ok(false)
        } else {
            Ok(true)
        }
    }
    pub(super) fn note_bad_sync(&mut self) -> Result<bool, SstvError> {
        self.sync_checks += 1;
        if self.sync_checks <= AUTO_STOP_WARMUP {
            return Ok(true);
        }
        self.bad_sync_score = self.bad_sync_score.saturating_add(BAD_SYNC_PENALTY);
        self.check_auto_stop()
    }
    pub(super) fn note_good_sync(&mut self) {
        self.sync_checks += 1;
        if self.sync_checks > AUTO_STOP_WARMUP {
            self.bad_sync_score = self.bad_sync_score.saturating_sub(GOOD_SYNC_REWARD);
        }
    }
}
