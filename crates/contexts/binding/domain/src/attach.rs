//! `gecko attach` (attach spec §3): when a new hub address may be asked about, what the
//! answer means, and the result once the binding stands.

use gecko_binding_contract::StoredToken;
use gecko_kernel::{Failure, Outcome};

/// What a level means, in a few words (attach spec §5.2).
pub fn level_text(level: &str) -> String {
    match level {
        "ask-server" => "ask-server (changes on the server ask for approval in the tab)".to_owned(),
        "never-ask" => "never-ask (nothing asks for approval)".to_owned(),
        other => other.to_owned(),
    }
}

/// Whether the trust question may be asked: not with `--json`, not with the code from
/// stdin, and only on a terminal.
pub fn may_ask(json: bool, from_stdin: bool, terminal: bool) -> bool {
    !json && !from_stdin && terminal
}

/// A new hub address and no way to ask about it.
pub fn untrusted(hub: &str) -> Failure {
    Failure::new(
        "untrusted-hub",
        format!(
            "{hub} has not been used from this computer; to trust it without a prompt: gecko attach --trust-hub <code>"
        ),
    )
}

/// The user said no.
pub fn declined(hub: &str) -> Failure {
    Failure::new("untrusted-hub", format!("not connected to {hub}"))
}

/// The trust question, answered on the same line.
pub fn trust_question(hub: &str) -> String {
    format!("{hub} has not been used from this computer. Connect to it? [y/N] ")
}

/// Whether a typed answer is a yes.
pub fn agreed(line: &str) -> bool {
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// The result of a binding that stands; the fields are for `--json`, in text the line
/// says it all.
pub fn attached(token: &StoredToken, json: bool) -> Outcome {
    let mut outcome = Outcome::done(format!(
        "Attached: {} · level {}",
        token.user.name,
        level_text(&token.level)
    ));
    outcome.data = json.then(|| {
        serde_json::json!({
            "hub": token.hub,
            "user": { "id": token.user.id, "name": token.user.name },
            "bindingId": token.binding_id,
            "level": token.level,
        })
    });
    outcome
}
