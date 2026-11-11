//! The exit code of every failure code the CLI writes (spec §2): a table test of
//! `exit_code_of`, and a scan of the CLI's sources for codes the table misses. The
//! sources scanned are listed once, in [`SOURCE_ROOTS`]; a crate that writes failures
//! (a context's `domain` or `module`) joins that list.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use gecko_kernel::exit_code_of;

/// Every failure code the CLI writes itself, with its exit code. A row changes only
/// with a decision: exit codes are part of the contract.
const EXIT_CODES: &[(&str, u8)] = &[
    ("failed", 1),
    ("usage", 2),
    ("unsupported-version", 2),
    ("unknown-command", 2),
    ("too-large", 2),
    ("not-allowed", 3),
    ("no-terminal-access", 3),
    ("untrusted-hub", 3),
    ("insecure-hub", 3),
    ("approval-dropped", 3),
    ("unreachable", 4),
    ("connection-lost", 4),
    ("busy", 4),
    ("no-binding", 4),
    ("gone", 4),
    ("tab-unreachable", 4),
    ("not-delivered", 4),
    ("rate-limited", 4),
    ("too-many-attempts", 4),
    ("unknown-outcome", 5),
    ("invalid-code", 6),
    ("unauthorized", 6),
    ("revoked", 6),
    ("cancelled", 130),
    ("cancel-requested", 130),
    ("cancelled-before-start", 130),
];

/// The source folders of every crate that writes the CLI's failures, from the
/// workspace root. Each is scanned with its subfolders.
const SOURCE_ROOTS: &[&str] = &[
    "crates/app/src",
    "crates/kernel/src",
    "crates/protocol/hub/src",
    "crates/contexts/binding/contract/src",
    "crates/contexts/binding/domain/src",
    "crates/contexts/binding/module/src",
    "crates/contexts/command/contract/src",
    "crates/contexts/command/domain/src",
    "crates/contexts/command/module/src",
];

#[test]
fn every_failure_code_has_its_exit_code() {
    for (code, exit) in EXIT_CODES {
        assert_eq!(exit_code_of(code), *exit, "{code}");
    }
    // Codes from the hub this table does not know.
    assert_eq!(exit_code_of("something-new"), 1);
    assert_eq!(exit_code_of("not-bound"), 1);
}

/// The string literals that open `Failure::new(` and `Outcome::error(` in `text`.
fn codes_written(text: &str) -> Vec<String> {
    let mut codes = Vec::new();
    for opener in ["Failure::new(", "Outcome::error("] {
        for (at, _) in text.match_indices(opener) {
            let rest = text[at + opener.len()..].trim_start();
            if let Some(rest) = rest.strip_prefix('"')
                && let Some(end) = rest.find('"')
            {
                codes.push(rest[..end].to_owned());
            }
        }
    }
    codes
}

/// Every `.rs` file under `dir`, its subfolders included.
fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn the_table_has_every_code_the_cli_writes() {
    assert_eq!(
        codes_written("Failure::new(\n  \"busy\", x) Outcome::error(code, m, 1)"),
        ["busy"]
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for dir in SOURCE_ROOTS {
        let dir = root.join(dir);
        assert!(dir.is_dir(), "{} is not a folder", dir.display());
        rust_files(&dir, &mut files);
    }
    let mut missing = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        for code in codes_written(&text) {
            if !EXIT_CODES.iter().any(|(c, _)| *c == code) {
                missing.push(format!("{}: {code}", path.display()));
            }
        }
    }
    assert!(missing.is_empty(), "codes without a row: {missing:?}");
}

/// command-console puts `too-large` in the class `usage` (exit 2), and so did the
/// TypeScript CLI; so does the CLI's own `too-large` (a line over the limits).
#[test]
fn too_large_is_a_usage_error() {
    assert_eq!(exit_code_of("too-large"), 2);
}
