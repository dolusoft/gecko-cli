//! What travels between the gecko CLI and the Gecko tab (spec §5;
//! command-console 1.7.0 `docs/remote-contract.md`), and the ASCII JSON writer for
//! `--json` output.
//!
//! Values read from the tab fall back to a defined default when a later version adds
//! one, instead of failing the whole envelope (§5.7 rule 4). The examples published
//! with command-console run against this crate in `gecko-testkit`
//! (`tests/wire_fixtures.rs`).
//!
//! The modules are private; every name is exported from the crate root.

mod ascii;
mod capabilities;
mod class;
mod reply;
mod request;

pub use ascii::{to_ascii_json, to_ascii_string};
pub use capabilities::{
    Capabilities, Command, ENVELOPES, Effect, PARTS_MAX, REQUIRED_FEATURES, read_parts, same_name,
    version_below,
};
pub use class::{Danger, ErrorClass};
pub use reply::{
    CHUNKS_MAX, Chunk, ENVELOPE_MAX_BYTES, Failure, Finished, HelpResult, Read, Received, Reply,
    Status, View, read_reply, read_value,
};
pub use request::{
    ARGV_MAX_ITEMS, Input, LINE_MAX, STAMP_ALLOWANCE, cancel, check_limits, cli_request,
    command_name, fits, help_request, is_space,
};
