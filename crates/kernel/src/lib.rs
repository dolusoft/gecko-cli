//! The shared kernel of the gecko CLI: what a call ends with (`Outcome`, `Failure`),
//! the exit code of each of the CLI's own failure codes and the messages several
//! commands share. No I/O, no clock, no environment (`clippy.toml`).

mod failure;
mod messages;
mod outcome;

pub use failure::{Failure, exit_code_of};
pub use messages::{BIND_AGAIN, gone_message};
pub use outcome::{Outcome, ViewInfo};
