//! The binding commands: `attach <code>`, `detach` and `status` (attach spec §3, §6).
//! A binding starts only from a code the user copied out of a Gecko tab; its token
//! lives and ends with it. Where the bound tab is comes from the hub (`binding.where`,
//! published by the tab, §5.9); the CLI does not ask the tab for it.

use std::io::IsTerminal;
use std::time::{Duration, Instant};

use gecko_hub_client::Transport;
use gecko_hub_client::hub::{Binding, BindingState, User, Where};
use gecko_hub_client::proxy::ProxyRule;
use serde::Serialize;

use crate::args::Options;
use crate::auth::{Auth, BIND_AGAIN};
use crate::code;
use crate::config::Config;
use crate::interrupt::Interrupts;
use crate::output::{Failure, Io, Outcome, interactive};
use crate::session::{self, now_ms};

/// How long ending the previous binding may take before `attach` goes on without it.
const OLD_BINDING_WAIT: Duration = Duration::from_secs(5);

/// Why a binding ended, for the reader; the hub's `reason` stays the machine value.
pub fn gone_message(reason: Option<&str>) -> String {
    let why = match reason.unwrap_or("") {
        "replaced" => "the tab was bound again with a new code",
        "closed" => "the bound tab was closed",
        "detached" => "the binding was removed",
        "logged-out" => "the browser session was signed out",
        "revoked" => "the CLI access was revoked in the profile",
        "suspended-timeout" => "the bound tab did not come back within 60 s",
        _ => "the binding ended",
    };
    format!("{why}; {BIND_AGAIN}")
}

/// What a level means, in a few words (attach spec §5.2).
pub fn level_text(level: &str) -> String {
    match level {
        "ask-server" => "ask-server (changes on the server ask for approval in the tab)".to_owned(),
        "never-ask" => "never-ask (nothing asks for approval)".to_owned(),
        other => other.to_owned(),
    }
}

/// "Tasks · /tasks": the tab's title and path, whichever it published.
fn place(location: Option<&Where>) -> Option<String> {
    let location = location?;
    let parts: Vec<&str> = [location.title.as_deref(), location.path.as_deref()]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// `gecko attach <code>`, `gecko attach -`, either with `--trust-hub`.
#[derive(Debug, PartialEq, Eq)]
pub struct AttachArgs {
    /// The code, or `-` for stdin.
    pub code: String,
    pub trust_hub: bool,
}

pub fn attach_args(args: &[String]) -> Result<AttachArgs, Failure> {
    let usage = |why: &str| Failure::new("usage", format!("{why}; usage: gecko attach <code>"));
    let mut code = None;
    let mut trust_hub = false;
    for word in args {
        if word == "--trust-hub" {
            trust_hub = true;
            continue;
        }
        if word.starts_with('-') && word != "-" {
            return Err(usage(&format!("unknown option {word}")));
        }
        if code.is_some() {
            return Err(usage("one code at a time"));
        }
        code = Some(word.clone());
    }
    let code = code.ok_or_else(|| usage("no code: copy the line from the Gecko tab"))?;
    Ok(AttachArgs { code, trust_hub })
}

fn read_stdin() -> Result<String, Failure> {
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).map_err(|error| {
        Failure::new("usage", format!("cannot read the code from stdin: {error}"))
    })?;
    Ok(text.trim().to_owned())
}

fn untrusted(hub: &str) -> Failure {
    Failure::new(
        "untrusted-hub",
        format!(
            "{hub} has not been used from this computer; to trust it without a prompt: gecko attach --trust-hub <code>"
        ),
    )
}

/// Asks once on the terminal whether to trust a new hub address.
async fn ask_trust(
    hub: &str,
    io: &mut Io<'_>,
    interrupts: &mut Interrupts,
) -> Result<bool, Failure> {
    io.clear_progress();
    let _ = write!(
        io.err,
        "{hub} has not been used from this computer. Connect to it? [y/N] "
    );
    let _ = io.err.flush();
    let answer = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map(|_| line)
    });
    tokio::select! {
        answer = answer => {
            let line = answer.ok().and_then(Result::ok).unwrap_or_default();
            let _ = writeln!(io.err);
            Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
        }
        () = interrupts.next() => Err(Failure::new("cancelled", "cancelled")),
    }
}

/// Ends the binding of `config`, if any: the hub's `detach` when it answers in time,
/// then the token file goes either way.
async fn end_old_binding(config: &Config) -> bool {
    let Some(old) = config.read_token() else {
        return false;
    };
    let _ = tokio::time::timeout(OLD_BINDING_WAIT, async {
        if let Ok(mut session) = session::open_with(config, old).await {
            let _ = session.hub.detach().await;
            session.hub.close().await;
        }
    })
    .await;
    let _ = config.delete_token();
    true
}

pub async fn attach(
    args: &[String],
    options: &Options,
    config: &Config,
    io: &mut Io<'_>,
    interrupts: &mut Interrupts,
) -> Result<Outcome, Failure> {
    let args = attach_args(args)?;
    let from_stdin = args.code == "-";
    let text = if from_stdin {
        read_stdin()?
    } else {
        args.code.clone()
    };
    let code = code::open(&text)?;
    io.info(&format!("Connecting to {}", code.hub));
    if !config.trusts(&code.hub) {
        let prompt = !options.json
            && !from_stdin
            && interactive(std::io::stdin().is_terminal())
            && interactive(std::io::stderr().is_terminal());
        let trusted = if args.trust_hub {
            true
        } else if prompt {
            ask_trust(&code.hub, io, interrupts).await?
        } else {
            return Err(untrusted(&code.hub));
        };
        if !trusted {
            return Err(Failure::new(
                "untrusted-hub",
                format!("not connected to {}", code.hub),
            ));
        }
        config.trust(&code.hub).map_err(|error| {
            Failure::new("failed", format!("cannot save the trusted hub: {error}"))
        })?;
    }
    let auth = Auth::new(&code.hub, ProxyRule::from_env())?;
    let token = auth.attach(&code.secret).await?;
    // One binding at a time: once the new one stands, the old one and its token end
    // (attach spec §3.4). A refused code leaves the old binding as it was.
    if end_old_binding(config).await {
        io.info("The previous binding ended");
    }
    config
        .write_token(&token)
        .map_err(|error| Failure::new("failed", format!("cannot save the token: {error}")))?;
    let mut outcome = Outcome::done(format!(
        "Attached: {} · level {}",
        token.user.name,
        level_text(&token.level)
    ));
    // The fields are for `--json`; in text the line says it all.
    outcome.data = options.json.then(|| {
        serde_json::json!({
            "hub": token.hub,
            "user": { "id": token.user.id, "name": token.user.name },
            "bindingId": token.binding_id,
            "level": token.level,
        })
    });
    Ok(outcome)
}

pub async fn detach(config: &Config) -> Result<Outcome, Failure> {
    let Some(token) = config.read_token() else {
        return Ok(Outcome::done("There was no binding"));
    };
    let ended = match session::open_with(config, token).await {
        Ok(mut session) => {
            let detached = session.hub.detach().await.is_ok();
            session.hub.close().await;
            detached
        }
        // The hub had ended it already.
        Err(failure) => failure.code == "gone",
    };
    config
        .delete_token()
        .map_err(|error| Failure::new("failed", format!("cannot delete the token: {error}")))?;
    Ok(Outcome::done(if ended {
        "Detached"
    } else {
        "Detached here; the hub was not reached, so the tab keeps the binding until it ends it (CLI bindings in the Gecko profile)"
    }))
}

/// What `gecko status --json` carries in `data`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusData {
    pub user: Option<User>,
    pub hub: Option<HubReach>,
    pub binding: Option<BindingData>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HubReach {
    pub url: String,
    pub reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingData {
    /// `active`, `suspended`, `gone` or `unknown`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `ask-server` or `never-ask` (attach spec §5.2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `ok` (the tab published where it is) or `busy` (another gecko call holds the hub).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab: Option<&'static str>,
}

fn state_name(state: BindingState) -> &'static str {
    match state {
        BindingState::Active => "active",
        BindingState::Suspended => "suspended",
        BindingState::Gone => "gone",
        BindingState::Unknown => "unknown",
    }
}

impl BindingData {
    fn of(binding: &Binding) -> BindingData {
        let location = binding.location.clone().unwrap_or_default();
        let published = location.title.is_some() || location.path.is_some();
        BindingData {
            state: state_name(binding.state),
            reason: binding.reason.clone(),
            level: binding.level.clone(),
            approved_at: binding.approved_at,
            page_id: Some(binding.page_id.clone()),
            tab: (binding.state == BindingState::Active && published).then_some("ok"),
            title: location.title,
            path: location.path,
        }
    }

    fn only(state: &'static str, level: Option<String>, tab: Option<&'static str>) -> BindingData {
        BindingData {
            state,
            reason: None,
            level,
            approved_at: None,
            page_id: None,
            title: None,
            path: None,
            tab,
        }
    }
}

/// "5 min ago": how long before `now` a time on the hub's clock was.
pub fn ago(then: u64, now: u64) -> String {
    let seconds = now.saturating_sub(then) / 1000;
    match seconds {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86_400),
    }
}

fn tab_line(binding: Option<&BindingData>, now: u64) -> String {
    let text = match binding {
        None => "none · to bind: gecko attach <code> (copy the line from the Gecko tab)".to_owned(),
        Some(b) if b.tab == Some("busy") => "unknown · another gecko call is running".to_owned(),
        Some(b) => match b.state {
            "suspended" => {
                "suspended (the tab is reloading or its connection dropped; the binding lasts if it comes back)"
                    .to_owned()
            }
            "gone" => format!("ended: {}", gone_message(b.reason.as_deref())),
            "active" => {
                let approved = b
                    .approved_at
                    .map(|at| format!(" (attached {})", ago(at, now)))
                    .unwrap_or_default();
                let location = Where {
                    title: b.title.clone(),
                    path: b.path.clone(),
                };
                let place = place(Some(&location))
                    .map(|place| format!(" · {place}"))
                    .unwrap_or_default();
                format!("bound{approved}{place}")
            }
            _ => "in a state this gecko does not know; update gecko".to_owned(),
        },
    };
    format!("tab     {text}")
}

/// The lines of the plain status: who, the tab, its level, the hub.
pub fn status_lines(data: &StatusData, now: u64) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(user) = &data.user {
        lines.push(format!("user    {}", user.name));
    }
    lines.push(tab_line(data.binding.as_ref(), now));
    if let Some(binding) = &data.binding
        && binding.state != "gone"
        && let Some(level) = &binding.level
    {
        lines.push(format!("level   {}", level_text(level)));
    }
    if let Some(hub) = &data.hub {
        let reach = if hub.reachable {
            match hub.latency_ms {
                Some(ms) => format!("reachable · {ms} ms"),
                None => "reachable".to_owned(),
            }
        } else {
            "unreachable".to_owned()
        };
        lines.push(format!("hub     {} · {reach}", hub.url));
    }
    lines
}

/// The status as a result: `--json` gets the fields and the tab line, plain text all
/// the lines.
pub fn status_outcome(data: &StatusData, json: bool, now: u64) -> Outcome {
    if !json {
        return Outcome::done(status_lines(data, now).join("\n"));
    }
    let mut outcome = Outcome::done(tab_line(data.binding.as_ref(), now));
    outcome.data = serde_json::to_value(data).ok();
    // The title and the path are the tab's own words.
    if let Some(binding) = &data.binding {
        if binding.title.is_some() {
            outcome.untrusted.push("data.binding.title");
        }
        if binding.path.is_some() {
            outcome.untrusted.push("data.binding.path");
        }
    }
    outcome
}

/// Gathers the status. Never binds. No binding: exit 0 and says how to bind; hub
/// unreachable: exit 4; `busy` (another gecko call holds the grant's one live
/// connection): the local part and a line that says so, exit 0.
pub async fn status(options: &Options, config: &Config) -> Result<Outcome, Failure> {
    let Some(token) = config.read_token() else {
        let data = StatusData {
            user: None,
            hub: None,
            binding: None,
        };
        return Ok(status_outcome(&data, options.json, now_ms()));
    };
    let user = User {
        id: token.user.id.clone(),
        name: token.user.name.clone(),
    };
    let (url, level) = (token.hub.clone(), token.level.clone());
    let started = Instant::now();
    let data = match session::open_with(config, token).await {
        Ok(session) => {
            let latency = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let data = StatusData {
                user: Some(session.status.user.clone()),
                hub: Some(HubReach {
                    url,
                    reachable: true,
                    latency_ms: Some(latency),
                }),
                binding: session.status.binding.as_ref().map(BindingData::of),
            };
            session.hub.close().await;
            data
        }
        // The 4409 close is the hub's own answer: it is up.
        Err(failure) if failure.code == "busy" => StatusData {
            user: Some(user),
            hub: Some(HubReach {
                url,
                reachable: true,
                latency_ms: None,
            }),
            binding: Some(BindingData::only("unknown", Some(level), Some("busy"))),
        },
        // The refresh was refused: the binding ended, and its token is gone.
        Err(failure) if failure.code == "gone" => StatusData {
            user: Some(user),
            hub: Some(HubReach {
                url,
                reachable: true,
                latency_ms: None,
            }),
            binding: Some(BindingData::only("gone", None, None)),
        },
        Err(failure) => return Err(failure),
    };
    Ok(status_outcome(&data, options.json, now_ms()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|w| (*w).to_owned()).collect()
    }

    fn data(binding: Option<BindingData>) -> StatusData {
        StatusData {
            user: Some(User {
                id: "u1".into(),
                name: "Zahit".into(),
            }),
            hub: Some(HubReach {
                url: "http://127.0.0.1:4400".into(),
                reachable: true,
                latency_ms: Some(12),
            }),
            binding,
        }
    }

    fn active() -> BindingData {
        BindingData {
            state: "active",
            reason: None,
            level: Some("ask-server".into()),
            approved_at: Some(1_000),
            page_id: Some("p1".into()),
            title: Some("Tasks".into()),
            path: Some("/tasks".into()),
            tab: Some("ok"),
        }
    }

    #[test]
    fn attach_arguments() {
        assert_eq!(
            attach_args(&words(&["gk1_x"])).unwrap(),
            AttachArgs {
                code: "gk1_x".into(),
                trust_hub: false
            }
        );
        assert_eq!(
            attach_args(&words(&["--trust-hub", "-"])).unwrap(),
            AttachArgs {
                code: "-".into(),
                trust_hub: true
            }
        );
        assert!(
            attach_args(&words(&["gk1_x", "--trust-hub"]))
                .unwrap()
                .trust_hub
        );
        for bad in [&[][..], &["--new"], &["a", "b"], &["--trust-hub"]] {
            assert_eq!(
                attach_args(&words(bad)).unwrap_err().code,
                "usage",
                "{bad:?}"
            );
        }
    }

    #[test]
    fn relative_times() {
        assert_eq!(ago(1_000, 30_000), "just now");
        assert_eq!(ago(0, 5 * 60_000 + 10), "5 min ago");
        assert_eq!(ago(0, 2 * 3_600_000), "2 h ago");
        assert_eq!(ago(0, 3 * 86_400_000), "3 d ago");
        assert_eq!(ago(10, 5), "just now");
    }

    #[test]
    fn plain_lines() {
        let now = 1_000 + 7 * 60_000;
        assert_eq!(
            status_lines(&data(Some(active())), now),
            [
                "user    Zahit",
                "tab     bound (attached 7 min ago) · Tasks · /tasks",
                "level   ask-server (changes on the server ask for approval in the tab)",
                "hub     http://127.0.0.1:4400 · reachable · 12 ms",
            ]
        );
        let none = StatusData {
            user: None,
            hub: None,
            binding: None,
        };
        assert_eq!(
            status_lines(&none, now),
            ["tab     none · to bind: gecko attach <code> (copy the line from the Gecko tab)"]
        );
        let gone = BindingData {
            state: "gone",
            reason: Some("closed".into()),
            title: None,
            path: None,
            tab: None,
            ..active()
        };
        let lines = status_lines(&data(Some(gone)), now);
        assert_eq!(
            lines[1],
            "tab     ended: the bound tab was closed; to bind again: gecko attach <code> (copy the line from the Gecko tab)"
        );
        assert!(!lines.iter().any(|l| l.starts_with("level")));
    }

    #[test]
    fn json_carries_data_and_the_tab_line() {
        let outcome = status_outcome(&data(Some(active())), true, 1_000);
        assert_eq!(
            outcome.summary.as_deref(),
            Some("tab     bound (attached just now) · Tasks · /tasks")
        );
        assert_eq!(
            outcome.untrusted,
            vec!["data.binding.title", "data.binding.path"]
        );
        let value = outcome.data.unwrap();
        assert_eq!(value["binding"]["approvedAt"], 1_000);
        assert_eq!(value["binding"]["level"], "ask-server");
        assert_eq!(value["binding"]["pageId"], "p1");
        assert_eq!(value["hub"]["latencyMs"], 12);
        assert_eq!(value["hub"]["url"], "http://127.0.0.1:4400");
        assert!(value["binding"].get("reason").is_none());
    }

    #[test]
    fn binding_data_of_a_hub_binding() {
        let binding: Binding = serde_json::from_str(
            r#"{"bindingId":"b","state":"active","pageId":"p","approvedAt":5,"level":"never-ask"}"#,
        )
        .unwrap();
        let data = BindingData::of(&binding);
        assert_eq!((data.state, data.tab), ("active", None));
        assert_eq!(data.level.as_deref(), Some("never-ask"));
        assert_eq!(place(None), None);
        assert_eq!(
            place(Some(&Where {
                title: Some(String::new()),
                path: Some("/x".into())
            })),
            Some("/x".into())
        );
    }
}
