//! The command context's use cases (spec §4.1): a line for the bound tab
//! (`features::run_line`) and `gecko help` (`features::help`), over an open hub
//! connection. The hub comes in as a generic `HubConnection`, the reader's stderr as a
//! `Reporter`, Ctrl+C as `Interrupts`; the clock, request ids, settings and this CLI's
//! version come in `gecko_command_contract::Ports`. Opening and closing the connection
//! is the caller's. Monotonic timers (the delivery window, the cancel wait, the wait
//! indicator) are tokio's and stay here; the rules they feed are in the domain.

mod exchange;
pub mod features;

pub use features::help::help;
pub use features::run_line::run_line;
pub use gecko_command_contract::Ports;
