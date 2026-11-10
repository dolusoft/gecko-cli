//! What the CLI sends the tab (command-console 1.7.0, "From the CLI"): the command
//! name rule (§5.3), the limits checked before sending (§5.1) and the envelopes.

use serde_json::{Value, json};

use crate::reply::ENVELOPE_MAX_BYTES;

/// Code points of one `argv` item, of all items together, and of a `line`.
pub const LINE_MAX: usize = 4000;
/// Items of one `argv` (`remoteArgvMaxItems`).
pub const ARGV_MAX_ITEMS: usize = 256;
/// Room the hub's stamp takes; the CLI's envelope stays this far below 16 KB.
pub const STAMP_ALLOWANCE: usize = 512;

/// A request's input: the words as the shell split them, or `--line` as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Argv(Vec<String>),
    Line(String),
}

/// The ECMAScript `WhiteSpace` and `LineTerminator` set: U+FEFF is in it, U+0085 is not.
pub fn is_space(c: char) -> bool {
    matches!(
        c,
        '\u{9}' | '\u{a}' | '\u{b}' | '\u{c}' | '\u{d}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// `^[A-Za-z][A-Za-z0-9._-]*$`
fn plain_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// The command name of a request, or `None` when it is not a plain name (then nothing
/// is sent and the CLI exits 2).
pub fn command_name(input: &Input) -> Option<&str> {
    let name = match input {
        Input::Argv(argv) => argv.first()?.as_str(),
        Input::Line(line) => {
            let rest = line.trim_start_matches(is_space);
            rest.split(is_space).next().unwrap_or("")
        }
    };
    plain_name(name).then_some(name)
}

fn code_points(text: &str) -> usize {
    text.chars().count()
}

/// The limits of §5.1, checked before sending; the error says which.
pub fn check_limits(input: &Input) -> Result<(), String> {
    match input {
        Input::Argv(argv) => {
            if argv.is_empty() || argv.len() > ARGV_MAX_ITEMS {
                return Err(format!("at most {ARGV_MAX_ITEMS} arguments"));
            }
            let total: usize = argv.iter().map(|item| code_points(item)).sum();
            if total > LINE_MAX {
                return Err(format!("the arguments are over {LINE_MAX} characters"));
            }
        }
        Input::Line(line) => {
            if code_points(line) > LINE_MAX {
                return Err(format!("the line is over {LINE_MAX} characters"));
            }
        }
    }
    Ok(())
}

/// Whether an envelope leaves room for the hub's stamp.
pub fn fits(envelope: &Value) -> bool {
    serde_json::to_string(envelope)
        .is_ok_and(|text| text.len() <= ENVELOPE_MAX_BYTES - STAMP_ALLOWANCE)
}

pub fn cli_request(v: u64, id: &str, input: &Input, view: Option<&str>) -> Value {
    let mut envelope = json!({ "v": v, "kind": "cli-request", "id": id });
    match input {
        Input::Argv(argv) => envelope["argv"] = json!(argv),
        Input::Line(line) => envelope["line"] = json!(line),
    }
    if let Some(view) = view {
        envelope["view"] = json!(view);
    }
    envelope
}

/// `want` is `text` or `json`; `width` only with `text`, bounded to 20..=400.
pub fn help_request(
    v: u64,
    id: &str,
    argv: &[String],
    json_wanted: bool,
    width: Option<u16>,
) -> Value {
    let mut envelope = json!({
        "v": v, "kind": "help-request", "id": id, "argv": argv,
        "want": if json_wanted { "json" } else { "text" }
    });
    if let (false, Some(width)) = (json_wanted, width) {
        envelope["width"] = json!(width.clamp(20, 400));
    }
    envelope
}

pub fn cancel(v: u64, id: &str) -> Value {
    json!({ "v": v, "kind": "cancel", "id": id })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Input {
        Input::Argv(items.iter().map(|s| (*s).to_owned()).collect())
    }

    #[test]
    fn names() {
        assert_eq!(
            command_name(&argv(&["tasks.list", "x"])),
            Some("tasks.list")
        );
        assert_eq!(
            command_name(&Input::Line("\u{2028} a-b_c d".into())),
            Some("a-b_c")
        );
        assert_eq!(command_name(&argv(&[])), None);
        assert_eq!(command_name(&argv(&["ş"])), None);
    }

    #[test]
    fn limits() {
        assert!(check_limits(&argv(&["a"; 256])).is_ok());
        assert!(check_limits(&argv(&["a"; 257])).is_err());
        let long = "ğ".repeat(4000);
        assert!(check_limits(&Input::Line(long.clone())).is_ok());
        assert!(check_limits(&Input::Line(format!("{long}x"))).is_err());
        assert!(check_limits(&argv(&["s", &"x".repeat(3999)])).is_ok());
        assert!(check_limits(&argv(&["s", &"x".repeat(4000)])).is_err());
    }

    #[test]
    fn envelopes() {
        let request = cli_request(1, "r1", &argv(&["tasks.stop", "--id", "8812"]), None);
        assert_eq!(
            request,
            json!({"v":1,"kind":"cli-request","id":"r1","argv":["tasks.stop","--id","8812"]})
        );
        assert!(fits(&request));
        assert_eq!(
            help_request(1, "h1", &[], false, Some(1000)),
            json!({"v":1,"kind":"help-request","id":"h1","argv":[],"want":"text","width":400})
        );
        assert_eq!(
            help_request(1, "h1", &["tasks".into()], true, Some(100))["width"],
            Value::Null
        );
        let big = cli_request(1, "r", &Input::Line("x".repeat(16_000)), None);
        assert!(!fits(&big));
    }
}
