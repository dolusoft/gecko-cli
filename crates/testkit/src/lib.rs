//! Test support for black-box tests of the gecko CLI: the real binary as a process, the
//! real development hub, and a fake Gecko tab on the hub's WebSocket.
//!
//! - [`Hub`]: the development hub on its own thread and tokio runtime, so a test that
//!   blocks on a process never stops it. This is the one file that touches the hub's
//!   Rust API.
//! - [`Tab`]: a fake tab of user `u1`. It reads its socket only inside its own calls; in
//!   between it is silent, and a silent tab answers no heartbeat ping.
//! - [`envelope`]: the only place tab replies are built. Each one is read back through
//!   `gecko-wire` and must come out as the kind it was built as.
//! - [`Cli`]: the binary whose path the test gives (`CARGO_BIN_EXE_gecko` exists only in
//!   the binary's own package), with its own config directory.
//! - [`connect`] and [`disconnect`]: a binding made from an attach code, and its end.
//!
//! Every wait has an upper bound ([`WAIT`], [`EXIT_WAIT`]); a wait that runs out fails
//! the test with what was seen so far. Nothing waits on a fixed sleep.

mod cli;
pub mod envelope;
mod fixtures;
mod flow;
mod hub;
mod tab;

use std::time::Duration;

pub use cli::{Call, Cli, Output, Process};
pub use envelope::Envelope;
pub use fixtures::{capabilities, capabilities_with, fixture, sequences};
pub use flow::{connect, disconnect};
pub use hub::{Hub, Options, Timings, User};
pub use tab::{Request, Tab};

/// The longest a test waits for one thing: a frame, an event, a line on stderr.
pub const WAIT: Duration = Duration::from_secs(20);

/// The longest a test waits for a CLI process to end.
pub const EXIT_WAIT: Duration = Duration::from_secs(60);

/// Milliseconds since the epoch: the tab's clock, which is this computer's.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}
