//! The shared kernel of the gecko CLI: what a call ends with (`Outcome`, `Failure`),
//! the exit code of each of the CLI's own failure codes and the messages several
//! commands share; the ports the binary implements (`Reporter`, `Interrupts`, `Clock`,
//! `IdSource`) and the `Settings` it reads. No I/O, no clock, no environment
//! (`clippy.toml`).

mod failure;
mod messages;
mod outcome;
mod ports;

pub use failure::{Failure, exit_code_of};
pub use messages::{BIND_AGAIN, gone_message, no_binding};
pub use outcome::{Outcome, ViewInfo};
pub use ports::{Clock, IdSource, Interrupts, Reporter, Settings};
