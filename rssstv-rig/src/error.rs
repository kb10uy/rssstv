use thiserror::Error;

/// Failure reported by rig control.
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
    /// it. It is passed through rather than translated: the operator wrote the
    /// command, so the number they can look up is more use than a guess at
    /// what it meant.
    #[error("`{command}` was refused by the rig with status {code}")]
    Refused { command: String, code: i32 },
    /// The answer did not end the way the protocol says it must.
    #[error("rigctld answered `{command}` with nothing this crate could read")]
    Unreadable { command: String },
    /// A command was configured with no words in it.
    #[error("a rig command has to name something to run")]
    EmptyCommand,
}
