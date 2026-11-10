//! What the CLI prints (§6 of the desktop CLI spec): the result on stdout, everything
//! else on stderr. `--json` is one versioned object with every non-ASCII character
//! escaped (gecko-wire), so it survives any code page and PowerShell 5.1.

use std::io::{IsTerminal, Write};

use serde::Serialize;

/// A failure the CLI itself reports, with a machine code (`not-logged-in`, `usage`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: String,
    pub message: String,
}

impl Failure {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Failure {
        Failure {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// The exit code of one of the CLI's own failure codes. Errors from the tab carry their
/// class instead (§5.6); this table is only for what the CLI decides by itself.
pub fn exit_code_of(code: &str) -> u8 {
    match code {
        "usage" | "unsupported-version" => 2,
        "not-allowed" | "pair-timeout" | "no-terminal-access" => 3,
        "unreachable" | "connection-lost" | "busy" | "no-binding" | "gone" | "tab-unreachable"
        | "not-delivered" | "rate-limited" | "invalid-code" | "expired" | "other-account" => 4,
        "unknown-outcome" => 5,
        "not-logged-in" | "unauthorized" | "token-expired" | "revoked" => 6,
        "cancelled" => 130,
        _ => 1,
    }
}

/// One result, as `--json` writes it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub v: u8,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// The process exit code; written on an error only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<u8>,
    pub preview: bool,
    pub complete: bool,
    pub untrusted: Vec<&'static str>,
}

impl Outcome {
    /// A finished own command: one summary line.
    pub fn done(summary: impl Into<String>) -> Outcome {
        Outcome {
            v: 1,
            status: "ok",
            code: None,
            message: None,
            summary: Some(summary.into()),
            data: None,
            exit_code: None,
            preview: false,
            complete: true,
            untrusted: Vec::new(),
        }
    }

    pub fn failed(failure: &Failure) -> Outcome {
        let exit = exit_code_of(&failure.code);
        Outcome {
            v: 1,
            status: match exit {
                5 => "unknown",
                130 => "cancelled",
                _ => "error",
            },
            code: Some(failure.code.clone()),
            message: Some(failure.message.clone()),
            summary: None,
            data: None,
            exit_code: Some(exit),
            preview: false,
            complete: exit != 5,
            untrusted: Vec::new(),
        }
    }

    pub fn exit(&self) -> u8 {
        self.exit_code.unwrap_or(0)
    }
}

/// Text that cannot drive the terminal: C0 and C1 control characters other than line
/// breaks and tabs are written as `\xHH` (§6.7).
pub fn escape_controls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let n = u32::from(c);
        let control = (n <= 0x1f && c != '\n' && c != '\t') || (0x7f..=0x9f).contains(&n);
        if control {
            out.push_str(&format!("\\x{n:02x}"));
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether a stream counts as a terminal; `GECKO_TEST_TTY=1` says yes (docs/test-hooks.md).
pub fn interactive(is_terminal: bool) -> bool {
    is_terminal || std::env::var_os("GECKO_TEST_TTY").is_some_and(|v| v == "1")
}

/// Where the CLI writes, and whether stderr may be coloured.
pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    pub color: bool,
    pub json: bool,
}

impl std::fmt::Debug for Io<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Io")
            .field("color", &self.color)
            .field("json", &self.json)
            .finish_non_exhaustive()
    }
}

impl Io<'_> {
    /// Colour on stderr: a terminal and no `NO_COLOR`.
    pub fn stderr_color() -> bool {
        interactive(std::io::stderr().is_terminal()) && std::env::var_os("NO_COLOR").is_none()
    }

    fn paint(&self, tone: u8, text: &str) -> String {
        if self.color {
            format!("\u{1b}[{tone}m{text}\u{1b}[0m")
        } else {
            text.to_owned()
        }
    }

    /// A line for the reader on stderr (progress, codes, hints).
    pub fn info(&mut self, text: &str) {
        let line = self.paint(2, &escape_controls(text));
        let _ = writeln!(self.err, "{line}");
    }

    /// Prints a result and returns the exit code.
    pub fn emit(&mut self, outcome: &Outcome) -> u8 {
        if self.json {
            let text = gecko_wire::ascii::to_ascii_json(outcome)
                .unwrap_or_else(|_| "{\"v\":1,\"status\":\"error\"}".to_owned());
            let _ = writeln!(self.out, "{text}");
        } else if outcome.status == "ok" {
            if let Some(summary) = &outcome.summary {
                let _ = writeln!(self.out, "{}", escape_controls(summary));
            }
        } else {
            let head = match &outcome.code {
                Some(code) => format!("error ({code})"),
                None => "error".to_owned(),
            };
            let head = self.paint(31, &head);
            let message = escape_controls(outcome.message.as_deref().unwrap_or(""));
            let _ = writeln!(self.err, "{head}: {message}");
        }
        let _ = self.out.flush();
        outcome.exit()
    }

    pub fn fail(&mut self, failure: &Failure) -> u8 {
        self.emit(&Outcome::failed(failure))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes() {
        assert_eq!(exit_code_of("usage"), 2);
        assert_eq!(exit_code_of("not-allowed"), 3);
        assert_eq!(exit_code_of("unreachable"), 4);
        assert_eq!(exit_code_of("unknown-outcome"), 5);
        assert_eq!(exit_code_of("not-logged-in"), 6);
        assert_eq!(exit_code_of("token-expired"), 6);
        assert_eq!(exit_code_of("cancelled"), 130);
        assert_eq!(exit_code_of("something-new"), 1);
    }

    #[test]
    fn controls_are_escaped() {
        assert_eq!(
            escape_controls("a\u{1b}[31mb\tc\nd\u{7f}\u{85}e"),
            "a\\x1b[31mb\tc\nd\\x7f\\x85e"
        );
        assert_eq!(escape_controls("çğü"), "çğü");
    }

    fn render(outcome: &Outcome, json: bool) -> (u8, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = Io {
            out: &mut out,
            err: &mut err,
            color: false,
            json,
        }
        .emit(outcome);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn text_and_json() {
        let (code, out, err) = render(&Outcome::done("Signed in: Zahit (dev)"), false);
        assert_eq!(
            (code, out.as_str(), err.as_str()),
            (0, "Signed in: Zahit (dev)\n", "")
        );

        let (code, out, _) = render(&Outcome::done("Signed in: Çağrı"), true);
        assert_eq!(code, 0);
        assert_eq!(
            out,
            "{\"v\":1,\"status\":\"ok\",\"summary\":\"Signed in: \\u00c7a\\u011fr\\u0131\",\"preview\":false,\"complete\":true,\"untrusted\":[]}\n"
        );

        let failure = Failure::new("not-logged-in", "not signed in");
        let (code, out, err) = render(&Outcome::failed(&failure), false);
        assert_eq!((code, out.as_str()), (6, ""));
        assert_eq!(err, "error (not-logged-in): not signed in\n");

        let (code, out, err) = render(&Outcome::failed(&failure), true);
        assert_eq!(code, 6);
        assert!(err.is_empty());
        assert_eq!(
            out,
            "{\"v\":1,\"status\":\"error\",\"code\":\"not-logged-in\",\"message\":\"not signed in\",\"exitCode\":6,\"preview\":false,\"complete\":true,\"untrusted\":[]}\n"
        );
    }

    #[test]
    fn cancelled_and_unknown_status() {
        let cancelled = Outcome::failed(&Failure::new("cancelled", "interrupted"));
        assert_eq!((cancelled.status, cancelled.exit()), ("cancelled", 130));
        let unknown = Outcome::failed(&Failure::new("unknown-outcome", "?"));
        assert_eq!((unknown.status, unknown.complete), ("unknown", false));
    }
}
