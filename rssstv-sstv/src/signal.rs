use crate::time::TxInstant;

/// 1900 Hz: leader tone and Robot 36 chrominance porch.
pub const LEADER_HZ: u32 = 1900;
/// 1200 Hz: sync pulse, break tone, and VIS start/stop bits.
pub const SYNC_HZ: u32 = 1200;
/// 1500 Hz: porch, image black reference, and Robot 36 luminance selector.
pub const PORCH_HZ: u32 = 1500;
/// 1100 Hz: VIS one bit and FSK identification mark.
pub const VIS_MARK_HZ: u32 = 1100;
/// 1300 Hz: VIS zero bit and FSK identification space.
pub const VIS_SPACE_HZ: u32 = 1300;

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

/// The protocol role of a generated tone.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TxComponent {
    /// Audio framing intended to trigger voice-operated transmit control.
    VoiceActivation,
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
    /// Post-image framing before station identification.
    Footer,
    /// FSK station identification.
    StationIdentification,
    /// An explicit silent interval.
    Silence,
}

/// A tone, its protocol role, and its absolute end deadline.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TimedTone {
    component: TxComponent,
    frequency: Frequency,
    until: TxInstant,
}

impl TimedTone {
    /// Constructs a timed tone.
    pub const fn new(component: TxComponent, frequency: Frequency, until: TxInstant) -> Self {
        Self {
            component,
            frequency,
            until,
        }
    }

    /// Returns the tone's protocol role.
    pub const fn component(self) -> TxComponent {
        self.component
    }

    /// Returns the tone frequency.
    pub const fn frequency(self) -> Frequency {
        self.frequency
    }

    /// Returns the absolute deadline measured from transmission start.
    pub const fn until(self) -> TxInstant {
        self.until
    }
}
