//! What the CLI prints (§6 of the desktop CLI spec): the result on stdout, everything
//! else on stderr. `--json` is one versioned object with every non-ASCII character
//! escaped (gecko-wire), so it survives any code page and PowerShell 5.1.

use std::io::{IsTerminal, Write};

use serde::Serialize;

/// What a bound command says when its binding has ended.
pub const BIND_AGAIN: &str =
    "to bind again: gecko attach <code> (copy the line from the Gecko tab)";

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
        "usage" | "unsupported-version" | "unknown-command" => 2,
        "not-allowed" | "no-terminal-access" | "untrusted-hub" | "insecure-hub"
        | "approval-dropped" => 3,
        "unreachable" | "connection-lost" | "busy" | "no-binding" | "gone" | "tab-unreachable"
        | "not-delivered" | "rate-limited" | "too-many-attempts" => 4,
        "unknown-outcome" => 5,
        "invalid-code" | "unauthorized" | "revoked" => 6,
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
    /// How a change ran, from the tab's `finished` (attach spec §5.4): `card` or `none`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval: Option<String>,
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
    /// Lines the tab wrapped for the terminal (`help` text); plain output only.
    #[serde(skip)]
    pub lines: Option<Vec<String>>,
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
            approval: None,
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
            lines: None,
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
fn plain_text(outcome: &Outcome, columns: Option<usize>) -> String {
    let mut lines: Vec<String> = outcome
        .lines
        .iter()
        .flatten()
        .map(|line| escape_controls(line))
        .collect();
    let summary = outcome.summary.as_deref().map(escape_controls);
    if outcome.preview {
        // A preview is what `--dry-run` asked for; the change runs without it (attach
        // spec §5.3), so no line to confirm with is printed.
        lines.push(format!("Preview: {}", summary.unwrap_or_default()));
    } else if let Some(summary) = summary {
        lines.push(summary);
    }
    if let Some(data) = &outcome.data {
        lines.push(crate::format::format_data(data, columns));
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
    /// The terminal's width when stdout is one (tables are cut to it).
    pub columns: Option<usize>,
    /// The one-line wait indicator may be drawn on stderr (UX U3).
    pub progress: bool,
    /// The indicator line is on screen now.
    pub progress_shown: bool,
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

    /// Where the CLI writes: colour, the indicator and the table width follow the
    /// streams (a terminal, `--json`, `--out`).
    pub fn new<'b>(
        out: &'b mut dyn Write,
        err: &'b mut dyn Write,
        json: bool,
        to_file: bool,
    ) -> Io<'b> {
        let stderr_tty = interactive(std::io::stderr().is_terminal());
        let columns = if to_file || !std::io::stdout().is_terminal() {
            None
        } else {
            terminal_size::terminal_size().map(|(w, _)| usize::from(w.0))
        };
        Io {
            out,
            err,
            color: Io::stderr_color(),
            json,
            columns,
            progress: !json && stderr_tty,
            progress_shown: false,
        }
    }

    /// Draws the wait indicator over its previous text (UX U3).
    pub fn progress(&mut self, text: &str) {
        if !self.progress {
            return;
        }
        self.progress_shown = true;
        let line = self.paint(2, &escape_controls(text));
        let _ = write!(self.err, "\r\u{1b}[2K{line}");
        let _ = self.err.flush();
    }

    /// Removes the indicator line, if one is shown.
    pub fn clear_progress(&mut self) {
        if self.progress_shown {
            self.progress_shown = false;
            let _ = write!(self.err, "\r\u{1b}[2K");
        }
    }

    /// A line for the reader on stderr (progress, codes, hints).
    pub fn info(&mut self, text: &str) {
        self.clear_progress();
        let line = self.paint(2, &escape_controls(text));
        let _ = writeln!(self.err, "{line}");
    }

    /// Prints a result and returns the exit code.
    pub fn emit(&mut self, outcome: &Outcome) -> u8 {
        self.clear_progress();
        if self.json {
            let text = gecko_wire::ascii::to_ascii_json(outcome)
                .unwrap_or_else(|_| "{\"v\":1,\"status\":\"error\"}".to_owned());
            let _ = writeln!(self.out, "{text}");
        } else if outcome.status == "ok" {
            let _ = self
                .out
                .write_all(plain_text(outcome, self.columns).as_bytes());
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

    /// Every failure code the CLI writes itself, with its exit code. A row changes only
    /// with a decision: exit codes are part of the contract.
    const EXIT_CODES: &[(&str, u8)] = &[
        ("failed", 1),
        ("usage", 2),
        ("unsupported-version", 2),
        ("unknown-command", 2),
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

    /// Codes the CLI writes whose exit code is wrong today; each has an ignored test.
    const WRONG_TODAY: &[&str] = &["too-large"];

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

    #[test]
    fn the_table_has_every_code_the_cli_writes() {
        assert_eq!(
            codes_written("Failure::new(\n  \"busy\", x) Outcome::error(code, m, 1)"),
            ["busy"]
        );
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut missing = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            // This module's tests make codes of their own.
            if path.extension().is_none_or(|ext| ext != "rs") || path.ends_with("output.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for code in codes_written(&text) {
                let known = EXIT_CODES.iter().any(|(c, _)| *c == code)
                    || WRONG_TODAY.contains(&code.as_str());
                if !known {
                    missing.push(format!("{}: {code}", path.display()));
                }
            }
        }
        assert!(missing.is_empty(), "codes without a row: {missing:?}");
    }

    /// command-console puts `too-large` in the class `usage` (exit 2), and so did the
    /// TypeScript CLI; the CLI's own `too-large` (a line over the limits) exits 1.
    #[test]
    #[ignore = "too-large exits 1, the contract says 2 (usage); a user-visible change waits for a decision"]
    fn too_large_is_a_usage_error() {
        assert_eq!(exit_code_of("too-large"), 2);
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
            columns: None,
            progress: false,
            progress_shown: false,
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
        let (code, out, err) = render(&Outcome::done("Detached"), false);
        assert_eq!((code, out.as_str(), err.as_str()), (0, "Detached\n", ""));

        let mut changed = Outcome::done("Görev durdu");
        changed.approval = Some("none".into());
        let (code, out, _) = render(&changed, true);
        assert_eq!(code, 0);
        assert_eq!(
            out,
            "{\"v\":1,\"status\":\"ok\",\"summary\":\"G\\u00f6rev durdu\",\"approval\":\"none\",\"preview\":false,\"complete\":true,\"untrusted\":[]}\n"
        );

        let failure = Failure::new("invalid-code", "the code is invalid");
        let (code, out, err) = render(&Outcome::failed(&failure), false);
        assert_eq!((code, out.as_str()), (6, ""));
        assert_eq!(err, "error (invalid-code): the code is invalid\n");

        let (code, out, err) = render(&Outcome::failed(&failure), true);
        assert_eq!(code, 6);
        assert!(err.is_empty());
        assert_eq!(
            out,
            "{\"v\":1,\"status\":\"error\",\"code\":\"invalid-code\",\"message\":\"the code is invalid\",\"exitCode\":6,\"preview\":false,\"complete\":true,\"untrusted\":[]}\n"
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
        assert_eq!(out, "Preview: 3 tasks would close\n");

        let mut page = Outcome::ok();
        page.data = Some(serde_json::json!({ "n": 1 }));
        page.next = Some(vec!["tasks.list".into(), "--cursor".into(), "2".into()]);
        let (_, out, _) = render(&page, false);
        assert_eq!(out, "n: 1\nMore: gecko tasks.list --cursor 2\n");

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
