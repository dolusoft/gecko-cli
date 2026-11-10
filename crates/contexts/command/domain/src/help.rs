//! `gecko help` as a result: how one `help-request` ended, and a `help-result` as the
//! CLI's outcome (command-console 1.7.0). The tab's own `help` command is not open to
//! the CLI; a `help …` line asks the way `gecko help …` does.

use gecko_kernel::{BIND_AGAIN, Outcome};
use gecko_wire::reply::{HelpResult, Status};
use gecko_wire::request;

/// The last line under a list of commands.
pub const FOOTER: &str = "CLI options come before the command name: gecko --json <command> ...; the full output with --json.";

/// How one `help-request` ended.
#[derive(Debug)]
pub enum Answer {
    Result(Box<HelpResult>),
    /// No `help-result` within the hub's window and the margin.
    NotDelivered,
    /// The connection ended; carries the reader's text.
    Lost(String),
    Revoked,
    Interrupted,
}

/// The outcome of an answer that is not a `help-result`. Help changes nothing, so a
/// lost answer is safe to ask again.
pub fn ended(answer: &Answer) -> Option<Outcome> {
    match answer {
        Answer::Result(_) => None,
        Answer::NotDelivered => Some(Outcome::error(
            "not-delivered",
            "the tab did not answer the help request; reload the Gecko page and try again",
            4,
        )),
        Answer::Lost(why) => Some(Outcome::error(
            "connection-lost",
            format!("{why}; help is safe to ask again"),
            4,
        )),
        Answer::Revoked => Some(Outcome::error(
            "revoked",
            format!("the binding ended; {BIND_AGAIN}"),
            6,
        )),
        Answer::Interrupted => Some(Outcome::error("cancelled", "cancelled", 130)),
    }
}

/// A list page of `text` starts with its section heading.
fn is_list(text: &[String]) -> bool {
    matches!(
        text.first().map(String::as_str),
        Some("Namespaces:" | "Commands:")
    )
}

/// A `help-result` as the CLI's result. `suggest` names a help target, so it is
/// printed as `gecko help <target>`.
pub fn outcome_of(help: &HelpResult, json: bool) -> Outcome {
    let mut outcome = match help.status {
        Status::Error => {
            let code = help
                .failure
                .code
                .clone()
                .unwrap_or_else(|| "failed".to_owned());
            let message = help.failure.message.clone().unwrap_or_else(|| code.clone());
            let mut outcome = Outcome::error(code, message, help.exit_code());
            if help.failure.message.is_some() {
                outcome.untrusted.push("message");
            }
            outcome.help = help.help.clone();
            outcome
        }
        Status::Ok => {
            let mut outcome = Outcome::ok();
            if json {
                outcome.data = help.result.clone();
                if outcome.data.is_some() {
                    outcome.untrusted.push("data");
                }
            } else if let Some(text) = &help.text {
                let mut lines = text.clone();
                if is_list(text) {
                    lines.push(FOOTER.to_owned());
                }
                outcome.lines = Some(lines);
            }
            outcome.next = help.next.clone();
            outcome
        }
    };
    outcome.suggest = help.suggest.as_ref().map(|target| {
        let mut argv = vec!["help".to_owned()];
        argv.extend(target.iter().cloned());
        argv
    });
    outcome
}

/// `help …` as a line (`--line "help tasks"`): its words, split at white space.
/// Help targets and page values never hold spaces.
pub fn line_args(line: &str) -> Option<Vec<String>> {
    let mut words = line.split(request::is_space).filter(|w| !w.is_empty());
    (words.next()? == "help").then(|| words.map(str::to_owned).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gecko_wire::reply::{Read, Reply, read_value};

    fn help(value: serde_json::Value) -> HelpResult {
        match read_value(&value) {
            Read::Reply(Reply::HelpResult(help)) => *help,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_list_gets_the_footer_and_next() {
        let result = help(serde_json::json!({
            "v": 1, "kind": "help-result", "id": "h1", "status": "ok",
            "text": ["Commands:", "  tasks.list · read"], "next": ["help", "--cursor", "20"]
        }));
        let outcome = outcome_of(&result, false);
        assert_eq!(
            outcome.lines,
            Some(vec![
                "Commands:".into(),
                "  tasks.list · read".into(),
                FOOTER.into()
            ])
        );
        assert_eq!(
            outcome.next,
            Some(vec!["help".into(), "--cursor".into(), "20".into()])
        );
    }

    #[test]
    fn an_unknown_target_suggests_help() {
        let result = help(serde_json::json!({
            "v": 1, "kind": "help-result", "id": "h1", "status": "error",
            "code": "unknown-command", "class": "usage", "message": "unknown command: tsks",
            "suggest": ["tasks"]
        }));
        let outcome = outcome_of(&result, false);
        assert_eq!(outcome.exit(), 2);
        assert_eq!(outcome.suggest, Some(vec!["help".into(), "tasks".into()]));
    }

    #[test]
    fn json_keeps_the_result() {
        let result = help(serde_json::json!({
            "v": 1, "kind": "help-result", "id": "h1", "status": "ok", "result": { "commands": [] }
        }));
        let outcome = outcome_of(&result, true);
        assert_eq!(outcome.data, Some(serde_json::json!({ "commands": [] })));
        assert_eq!(outcome.untrusted, ["data"]);
    }

    #[test]
    fn help_lines() {
        assert_eq!(
            line_args("help tasks --cursor 2"),
            Some(vec!["tasks".into(), "--cursor".into(), "2".into()])
        );
        assert_eq!(line_args("  help"), Some(vec![]));
        assert_eq!(line_args("helpx"), None);
        assert_eq!(line_args("tasks.list"), None);
    }
}
