//! The built binary, run as a process.

use std::process::{Command, Output};

fn gecko(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gecko"))
        .args(args)
        .output()
        .expect("gecko runs")
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).expect("UTF-8 output")
}

#[test]
fn help_is_english_and_exits_zero() {
    let output = gecko(&["--help"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = text(output.stdout);
    assert!(stdout.starts_with("gecko: remote control of the Gecko web terminal"));
    assert!(stdout.is_ascii());
    assert!(output.stderr.is_empty());
}

#[test]
fn version_prints_only_the_version() {
    let output = gecko(&["--version"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        text(output.stdout),
        format!("{}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn unknown_option_is_a_usage_error() {
    let output = gecko(&["--nope"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(text(output.stderr).contains("unknown option --nope"));
}

// Runs once per build: with the default features it sees the dev hub, and CI runs the
// suite again with --no-default-features, as the release build is made.
#[test]
fn dev_hub_follows_the_feature() {
    let output = gecko(&["dev-hub"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = text(output.stderr);
    if cfg!(feature = "dev-hub") {
        assert!(stderr.contains("not written yet"), "{stderr}");
    } else {
        assert!(stderr.contains("this build has no dev hub"), "{stderr}");
    }
}

#[cfg(windows)]
#[test]
fn invalid_unicode_argument_does_not_crash() {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    let output = Command::new(env!("CARGO_BIN_EXE_gecko"))
        .arg(OsString::from_wide(&[0x61, 0xd800]))
        .output()
        .expect("gecko runs");
    assert_eq!(output.status.code(), Some(2));
    assert!(text(output.stderr).contains("not valid Unicode"));
}

#[cfg(unix)]
#[test]
fn invalid_unicode_argument_does_not_crash() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let output = Command::new(env!("CARGO_BIN_EXE_gecko"))
        .arg(OsString::from_vec(vec![0x61, 0xff]))
        .output()
        .expect("gecko runs");
    assert_eq!(output.status.code(), Some(2));
    assert!(text(output.stderr).contains("not valid Unicode"));
}
