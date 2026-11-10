//! Opens an address in the default browser when there is a desktop; without one (SSH, a
//! server) the caller prints the address and the code instead (Node: src/cli/browser.ts).

use std::process::{Command, Stdio};

/// Whether this session can show a browser. `GECKO_BROWSER=none` says no.
pub fn has_desktop() -> bool {
    let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    if std::env::var_os("GECKO_BROWSER").is_some_and(|v| v == "none") {
        return false;
    }
    if set("SSH_CONNECTION") || set("SSH_TTY") {
        return false;
    }
    if cfg!(any(windows, target_os = "macos")) {
        return true;
    }
    set("DISPLAY") || set("WAYLAND_DISPLAY")
}

/// Starts the default browser on `url`; `false` when it could not. The address is built
/// by the CLI from the hub's answer, never typed by the user.
pub fn open(url: &str) -> bool {
    // Only a web address: the hub's answer must not start a local program or file.
    let web = url::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https"));
    if !web || !has_desktop() {
        return false;
    }
    let mut command = if cfg!(windows) {
        let mut command = Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler").arg(url);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(url);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}
