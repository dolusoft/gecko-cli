//! The command context's use cases (spec §4.1): a line for the bound tab
//! (`features::run_line`) and `gecko help` (`features::help`), over an open hub
//! connection. The hub comes in as a generic `HubConnection`, the reader's stderr as a
//! `Reporter`, Ctrl+C as `Interrupts`; the clock, request ids, settings and this CLI's
//! version come in `gecko_command_contract::Ports`. Opening and closing the connection
//! is the caller's. Monotonic timers (the delivery window, the cancel wait, the wait
//! indicator) are tokio's and stay here; the rules they feed are in the domain.
//!
//! The binary reaches the context through this crate only: the two rules it needs
//! before a connection is open (`precheck`, `line_args`) are passed through here.

mod exchange;
mod features;

pub use features::help::help;
pub use features::run_line::run_line;
pub use gecko_command_contract::Ports;
use gecko_kernel::Failure;
use gecko_wire::Input;

/// The line's command name, once the name rule and the limits hold; checked before a
/// connection is opened (the domain's `line::precheck`).
pub fn precheck(input: &Input) -> Result<String, Failure> {
    gecko_command_domain::line::precheck(input)
}

/// `help …` typed as a line: its words after `help` (the domain's `help::line_args`).
pub fn line_args(line: &str) -> Option<Vec<String>> {
    gecko_command_domain::help::line_args(line)
}
