//! The development hub behind `gecko dev-hub` (K1). It is compiled into the binary only
//! with the `dev-hub` feature: developer and CI builds have it, release builds do not.
//!
//! The Rust port of the Node hub (src/hub/) comes after step 4b; until then this crate
//! only reports that.

/// Runs `gecko dev-hub <args>` and returns the exit code.
pub fn run(_args: &[String], out: &mut dyn std::io::Write) -> u8 {
    let _ = writeln!(
        out,
        "gecko dev-hub: the Rust development hub is not written yet; use the Node build: node bin/gecko.js dev-hub"
    );
    2
}
