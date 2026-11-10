//! The command context's rules (spec §4.1): what a line for the bound tab may be, what
//! the tab's capability list allows, how a sent request ends (`run`) and how a help
//! answer reads as the CLI's result (`help`). No I/O, no clock: the time of an event and
//! this CLI's version come in as values (`clippy.toml`).

pub mod help;
pub mod line;
pub mod run;
