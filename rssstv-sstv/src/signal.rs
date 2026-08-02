use crate::time::TxInstant;

/// An integer audio frequency in hertz.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Frequency(u32);

impl Frequency {
    /// Constructs an integer frequency in hertz.
    pub const fn from_hz(hz: u32) -> Self {
        Self(hz)
    }

    /// Returns the integer frequency in hertz.
    pub const fn as_hz(self) -> u32 {
        self.0
    }
}

/// A constant-frequency SSTV tone.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Tone(Frequency);

impl Tone {
    /// Constructs a tone at `frequency`.
    pub const fn new(frequency: Frequency) -> Self {
        Self(frequency)
    }

    /// Returns this tone's frequency.
    pub const fn frequency(self) -> Frequency {
        self.0
    }
}

/// The protocol role of a generated tone.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TxComponent {
    /// Leader or break framing before mode identification.
    Leader,
    /// Conventional VIS, extended VIS, or narrow mode identification.
    Identification,
    /// Horizontal or AVT synchronization.
    Sync,
    /// Porch or component separator.
    Porch,
    /// A red image sample.
    Red,
    /// A green image sample.
    Green,
    /// A blue image sample.
    Blue,
    /// A luminance image sample.
    Luminance,
    /// An R-Y chrominance sample.
    RedDifference,
    /// A B-Y chrominance sample.
    BlueDifference,
    /// Robot 36's chrominance-selection tone.
    ChrominanceSelector,
}

/// A tone, its protocol role, and its absolute end deadline.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TimedTone {
    component: TxComponent,
    tone: Tone,
    until: TxInstant,
}

impl TimedTone {
    /// Constructs a timed tone.
    pub const fn new(component: TxComponent, tone: Tone, until: TxInstant) -> Self {
        Self {
            component,
            tone,
            until,
        }
    }

    /// Returns the tone's protocol role.
    pub const fn component(self) -> TxComponent {
        self.component
    }

    /// Returns the tone.
    pub const fn tone(self) -> Tone {
        self.tone
    }

    /// Returns the absolute deadline measured from transmission start.
    pub const fn until(self) -> TxInstant {
        self.until
    }
}
