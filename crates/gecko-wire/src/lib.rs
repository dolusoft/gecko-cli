//! What travels between the gecko CLI and the Gecko tab (docs/rust-migration.md §5),
//! and the ASCII JSON writer for `--json` output.
//!
//! Enums read from the tab carry `#[serde(other)]`, so a value added later falls back
//! to a defined default instead of failing the whole envelope (§5.7 rule 4).

pub mod ascii;
pub mod class;

pub use ascii::{to_ascii_json, to_ascii_string};
pub use class::{Danger, ErrorClass};
