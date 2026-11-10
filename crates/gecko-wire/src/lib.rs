//! What travels between the gecko CLI and the Gecko tab (docs/rust-migration.md §5;
//! command-console 1.7.0 `docs/remote-contract.md`), and the ASCII JSON writer for
//! `--json` output.
//!
//! Values read from the tab fall back to a defined default when a later version adds
//! one, instead of failing the whole envelope (§5.7 rule 4). The examples published
//! with command-console run against this crate (`tests/fixtures.rs`).

pub mod ascii;
pub mod capabilities;
pub mod class;
pub mod reply;
pub mod request;

pub use ascii::{to_ascii_json, to_ascii_string};
pub use class::{Danger, ErrorClass};
