use thiserror::Error;

/// Failure reported by a rig transport.
///
/// Hamlib's own types are deliberately absent: this crate speaks to `rigctld`
/// over a socket, and the transport is an implementation detail of it.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum RigError {
    /// The configured address named no host this crate could resolve.
    #[error("`{0}` is not an address rig control can reach")]
    Address(String),
    /// Nothing was listening, or the connection could not be established.
    #[error("rig control could not reach {address}: {detail}")]
    Connect { address: String, detail: String },
    /// The socket failed while a command was in flight.
    #[error("the connection to rig control failed: {0}")]
    Transport(String),
    /// `rigctld` hung up.
    #[error("rigctld closed the connection")]
    Closed,
    /// The command reached Hamlib and Hamlib rejected it.
    ///
    /// The code is Hamlib's own `RIG_E*` value, negated as `rigctld` reports
    /// it. It is passed through rather than translated: the script wrote the
    /// command, so the number that can be looked up is more use than a guess
    /// at what it meant.
    #[error("`{command}` was refused by the rig with status {code}")]
    Refused { command: String, code: i32 },
    /// The answer did not end the way the protocol says it must.
    #[error("rigctld answered `{command}` with nothing this crate could read")]
    Unreadable { command: String },
    /// The command held a line break, or nothing at all.
    ///
    /// One call sends one command. A line break would frame two, and the
    /// second would be answered separately, leaving an answer in the stream
    /// that every later command would then read one behind.
    #[error("a rig command has to be one non-empty line")]
    NotOneLine,
}

impl RigError {
    /// Whether the connection is still worth holding on to after this.
    ///
    /// A command the rig refused is about that command; it can be corrected
    /// and tried again over the same connection. A transport that failed is
    /// not something the next command would survive either.
    pub const fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::Address(_) | Self::Connect { .. } | Self::Transport(_) | Self::Closed
        )
    }
}
