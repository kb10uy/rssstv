use core::ops::{Add, Sub};

use crate::SstvError;

/// A non-negative SSTV protocol duration with picosecond precision.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SstvDuration(u64);

impl SstvDuration {
    /// A zero-length duration.
    pub const ZERO: Self = Self(0);

    /// Constructs a duration from picoseconds.
    pub const fn from_picos(picos: u64) -> Self {
        Self(picos)
    }

    /// Constructs a duration from microseconds, returning `None` on overflow.
    pub const fn from_micros(micros: u64) -> Option<Self> {
        match micros.checked_mul(1_000_000) {
            Some(picos) => Some(Self(picos)),
            None => None,
        }
    }

    /// Constructs a duration from milliseconds, returning `None` on overflow.
    pub const fn from_millis(millis: u64) -> Option<Self> {
        match millis.checked_mul(1_000_000_000) {
            Some(picos) => Some(Self(picos)),
            None => None,
        }
    }

    /// Returns the duration in picoseconds.
    pub const fn as_picos(self) -> u64 {
        self.0
    }

    /// Adds two durations, returning `None` on overflow.
    pub const fn checked_add(self, rhs: Self) -> Option<Self> {
        match self.0.checked_add(rhs.0) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// Subtracts a duration, returning `None` if the result would be negative.
    pub const fn checked_sub(self, rhs: Self) -> Option<Self> {
        match self.0.checked_sub(rhs.0) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

impl Add for SstvDuration {
    type Output = Result<Self, SstvError>;

    fn add(self, rhs: Self) -> Self::Output {
        self.checked_add(rhs).ok_or(SstvError::TimeOverflow)
    }
}

impl Sub for SstvDuration {
    type Output = Option<Self>;

    fn sub(self, rhs: Self) -> Self::Output {
        self.checked_sub(rhs)
    }
}

/// A monotonic transmit deadline measured from the start of an encoding.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TxInstant(u64);

impl TxInstant {
    /// The start of a transmission.
    pub const ZERO: Self = Self(0);

    /// Constructs a deadline from picoseconds after transmission start.
    pub const fn from_picos(picos: u64) -> Self {
        Self(picos)
    }

    /// Returns picoseconds elapsed since transmission start.
    pub const fn as_picos(self) -> u64 {
        self.0
    }

    /// Advances this deadline, returning `None` on overflow.
    pub const fn checked_add(self, duration: SstvDuration) -> Option<Self> {
        match self.0.checked_add(duration.as_picos()) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// Returns the duration since `earlier`, or `None` if it is later than this instant.
    pub const fn checked_duration_since(self, earlier: Self) -> Option<SstvDuration> {
        match self.0.checked_sub(earlier.0) {
            Some(value) => Some(SstvDuration::from_picos(value)),
            None => None,
        }
    }
}

impl Add<SstvDuration> for TxInstant {
    type Output = Result<Self, SstvError>;

    fn add(self, rhs: SstvDuration) -> Self::Output {
        self.checked_add(rhs).ok_or(SstvError::TimeOverflow)
    }
}

impl Sub for TxInstant {
    type Output = Option<SstvDuration>;

    fn sub(self, rhs: Self) -> Self::Output {
        self.checked_duration_since(rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cumulative_deadlines_are_exact() {
        let line = SstvDuration::from_picos(446_446_000_000);
        let first = TxInstant::ZERO.checked_add(line).unwrap();
        let second = first.checked_add(line).unwrap();
        assert_eq!(second.as_picos(), 892_892_000_000);
        assert_eq!(second.checked_duration_since(first), Some(line));
    }

    #[test]
    fn checked_operations_reject_invalid_ranges() {
        assert_eq!(
            SstvDuration::from_picos(u64::MAX).checked_add(SstvDuration::from_picos(1)),
            None
        );
        assert_eq!(
            TxInstant::ZERO.checked_duration_since(TxInstant::from_picos(1)),
            None
        );
    }

    #[test]
    fn unit_constructors_reject_overflow() {
        assert_eq!(
            SstvDuration::from_micros(u64::MAX / 1_000_000),
            Some(SstvDuration::from_picos((u64::MAX / 1_000_000) * 1_000_000))
        );
        assert_eq!(SstvDuration::from_micros(u64::MAX / 1_000_000 + 1), None);
        assert_eq!(
            SstvDuration::from_millis(u64::MAX / 1_000_000_000),
            Some(SstvDuration::from_picos(
                (u64::MAX / 1_000_000_000) * 1_000_000_000
            ))
        );
        assert_eq!(
            SstvDuration::from_millis(u64::MAX / 1_000_000_000 + 1),
            None
        );
    }
}
