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
        "cancelled" | "cancel-requested" | "cancelled-before-start" => 130,
        _ => 1,
    }
}

/// The view a command ran in (`finished.view`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViewInfo {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// One result, as `--json` writes it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub v: u8,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Why a binding ended (`gone`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<ViewInfo>,
    /// Whether the page was hidden when it accepted the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    /// The tab's data age: a number, or `null` when it does not know.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_age_ms: Option<serde_json::Value>,
    /// The process exit code; written on an error only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<u8>,
    pub preview: bool,
    /// Argv the tab offers (a preview's confirmed form, the nearest name); never run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggest: Option<Vec<String>>,
    /// Argv of the next page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<Vec<String>>,
    /// The command whose help an `invalid-args` points to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    pub complete: bool,
    pub untrusted: Vec<&'static str>,
}

impl Outcome {
    fn base(status: &'static str) -> Outcome {
        Outcome {
            v: 1,
            status,
            code: None,
            message: None,
            reason: None,
            summary: None,
            data: None,
            view: None,
            hidden: None,
            data_age_ms: None,
            exit_code: None,
            preview: false,
            suggest: None,
            next: None,
            help: None,
            complete: true,
            untrusted: Vec::new(),
        }
    }

    /// A finished command with nothing but its summary line.
    pub fn done(summary: impl Into<String>) -> Outcome {
        Outcome {
            summary: Some(summary.into()),
            ..Outcome::base("ok")
        }
    }

    /// An ok result with no fields set yet.
    pub fn ok() -> Outcome {
        Outcome::base("ok")
    }

    /// A failure with its exit code given (the tab's class, or the CLI's own
    /// observation, §5.6): 5 is `unknown` and incomplete, 130 `cancelled`.
    pub fn error(code: impl Into<String>, message: impl Into<String>, exit: u8) -> Outcome {
        Outcome {
            code: Some(code.into()),
            message: Some(message.into()),
            exit_code: Some(exit),
            complete: exit != 5,
            ..Outcome::base(match exit {
                5 => "unknown",
                130 => "cancelled",
                _ => "error",
            })
        }
    }

    pub fn failed(failure: &Failure) -> Outcome {
        Outcome::error(
            failure.code.clone(),
            failure.message.clone(),
            exit_code_of(&failure.code),
        )
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

/// One argument as the reader's shell takes it back: PowerShell on Windows, a POSIX
/// shell elsewhere. A plain word stays as it is; anything else goes in single quotes.
pub fn quote_arg(arg: &str) -> String {
    if cfg!(windows) {
        quote_powershell(arg)
    } else {
        quote_posix(arg)
    }
}

fn plain_word(arg: &str, extra: &str) -> bool {
    !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-/:=+".contains(c) || extra.contains(c))
}

pub fn quote_powershell(arg: &str) -> String {
    // `@` and `,` mean something to PowerShell; `--%` stops its parsing.
    if plain_word(arg, "") && arg != "--%" {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', "''"))
    }
}

pub fn quote_posix(arg: &str) -> String {
    if plain_word(arg, ",@%") {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// An argv as one line for the reader's shell.
pub fn quote_argv(argv: &[String]) -> String {
    argv.iter()
        .map(|a| quote_arg(a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The plain text of an ok result for stdout.
fn plain_text(outcome: &Outcome) -> String {
    let mut lines = Vec::new();
    let summary = outcome.summary.as_deref().map(escape_controls);
    if outcome.preview {
        let head = format!("Preview: {}", summary.unwrap_or_default());
        lines.push(match &outcome.suggest {
            Some(argv) => format!(
                "{head}. To run it: gecko {}",
                escape_controls(&quote_argv(argv))
            ),
            None => head,
        });
    } else if let Some(summary) = summary {
        lines.push(summary);
    }
    if let Some(data) = &outcome.data {
        let text = serde_json::to_string_pretty(data).unwrap_or_default();
        lines.push(escape_controls(&text));
    }
    if !outcome.complete {
        lines.push("(the result arrived incomplete; no data printed)".to_owned());
    }
    if let Some(next) = &outcome.next {
        lines.push(format!(
            "More: gecko {}",
            escape_controls(&quote_argv(next))
        ));
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
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
            let _ = self.out.write_all(plain_text(outcome).as_bytes());
        } else {
            let head = match &outcome.code {
                Some(code) => format!("error ({code})"),
                None => "error".to_owned(),
            };
            let head = self.paint(31, &head);
            let message = escape_controls(outcome.message.as_deref().unwrap_or(""));
            let _ = writeln!(self.err, "{head}: {message}");
            if let Some(suggest) = &outcome.suggest {
                let _ = writeln!(
                    self.err,
                    "Did you mean: gecko {}",
                    escape_controls(&quote_argv(suggest))
                );
            }
            if let Some(help) = &outcome.help {
                let _ = writeln!(
                    self.err,
                    "Usage: gecko help {}",
                    escape_controls(&quote_argv(std::slice::from_ref(help)))
                );
            }
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
    fn quoting() {
        assert_eq!(quote_posix("tasks.list"), "tasks.list");
        assert_eq!(quote_posix("a b"), "'a b'");
        assert_eq!(quote_posix("it's"), "'it'\\''s'");
        assert_eq!(quote_posix(""), "''");
        assert_eq!(quote_powershell("--limit=5"), "--limit=5");
        assert_eq!(quote_powershell("@x"), "'@x'");
        assert_eq!(quote_powershell("it's"), "'it''s'");
        assert_eq!(quote_powershell("--%"), "'--%'");
    }

    #[test]
    fn plain_result_lines() {
        let mut preview = Outcome::ok();
        preview.preview = true;
        preview.summary = Some("3 tasks would close".into());
        preview.suggest = Some(vec!["tasks.close".into(), "--confirm".into()]);
        let (_, out, _) = render(&preview, false);
        assert_eq!(
            out,
            "Preview: 3 tasks would close. To run it: gecko tasks.close --confirm\n"
        );

        let mut page = Outcome::ok();
        page.data = Some(serde_json::json!({ "n": 1 }));
        page.next = Some(vec!["tasks.list".into(), "--cursor".into(), "2".into()]);
        let (_, out, _) = render(&page, false);
        assert_eq!(out, "{\n  \"n\": 1\n}\nMore: gecko tasks.list --cursor 2\n");

        let mut unknown = Outcome::error("unknown-command", "unknown command: tsks", 2);
        unknown.suggest = Some(vec!["tasks.list".into()]);
        unknown.help = Some("tasks.list".into());
        let (code, out, err) = render(&unknown, false);
        assert_eq!((code, out.as_str()), (2, ""));
        assert_eq!(
            err,
            "error (unknown-command): unknown command: tsks\nDid you mean: gecko tasks.list\nUsage: gecko help tasks.list\n"
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
