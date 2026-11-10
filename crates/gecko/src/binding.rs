//! The binding commands: `attach`, `detach` and `status`. Where the bound tab is comes
//! from the hub (`binding.where`, published by the tab, §5.9); the CLI does not ask the
//! tab for it.

use std::time::Instant;

use gecko_hub_client::Transport;
use gecko_hub_client::hub::{Binding, BindingState, User, Where};
use serde::Serialize;

use crate::args::Options;
use crate::config::Config;
use crate::interrupt::Interrupts;
use crate::output::{Failure, Io, Outcome};
use crate::own::{Flag, env_of, own_option};
use crate::session::{self, Session, call_failure, now_ms};

/// Why a binding ended, for the reader; the hub's `reason` stays the machine value.
pub fn gone_message(reason: Option<&str>) -> String {
    let why = match reason.unwrap_or("") {
        "replaced" => "the binding moved to another tab or CLI install",
        "closed" => "the bound tab was closed or reloaded",
        "detached" => "the binding was removed",
        "logged-out" => "the browser session was signed out",
        "revoked" => "the CLI grant was revoked in the profile",
        "suspended-timeout" => "the bound tab did not come back within 60 s",
        _ => "the binding ended",
    };
    format!("{why}; to bind again: gecko attach")
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

async fn close(session: Session) {
    session.hub.close().await;
}

pub async fn attach(
    args: &[String],
    options: &Options,
    config: &Config,
    io: &mut Io<'_>,
    interrupts: &mut Interrupts,
) -> Result<Outcome, Failure> {
    let env = env_of(args, options, config)?;
    let mut session = session::open(&env, config).await?;
    let fresh = own_option(args, "new") != Flag::Absent;
    if let Some(current) = session.status.binding.as_ref()
        && current.state == BindingState::Active
        && !fresh
    {
        let name =
            place(current.location.as_ref()).unwrap_or_else(|| format!("page {}", current.page_id));
        close(session).await;
        return Ok(Outcome::done(format!(
            "Already bound: {name}. For a new tab: gecko attach --new"
        )));
    }
    let paired = session::pair(&mut session, io, interrupts).await;
    close(session).await;
    let binding = paired?;
    Ok(Outcome::done(match place(binding.location.as_ref()) {
        Some(name) => format!("Attached: {name}"),
        None => "Attached".to_owned(),
    }))
}

pub async fn detach(
    args: &[String],
    options: &Options,
    config: &Config,
) -> Result<Outcome, Failure> {
    let env = env_of(args, options, config)?;
    let mut session = session::open(&env, config).await?;
    let bound = session
        .status
        .binding
        .as_ref()
        .is_some_and(|binding| binding.state != BindingState::Gone);
    if !bound {
        // The hub says ok either way; the CLI says what was there.
        close(session).await;
        return Ok(Outcome::done("There was no binding"));
    }
    let detached = session.hub.detach().await;
    close(session).await;
    detached.map_err(|error| call_failure(error, true))?;
    Ok(Outcome::done("Detached"))
}

/// What `gecko status --json` carries in `data`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusData {
    pub user: Option<User>,
    pub env: String,
    pub hub: HubReach,
    pub binding: Option<BindingData>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HubReach {
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
            approved_at: binding.approved_at,
            page_id: Some(binding.page_id.clone()),
            tab: (binding.state == BindingState::Active && published).then_some("ok"),
            title: location.title,
            path: location.path,
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

/// The three lines of the plain status.
pub fn status_lines(data: &StatusData, now: u64) -> [String; 3] {
    let who = data
        .user
        .as_ref()
        .map(|user| format!(" · {}", user.name))
        .unwrap_or_default();
    let tab = match &data.binding {
        None => "none · to bind: gecko attach".to_owned(),
        Some(b) if b.tab == Some("busy") => "unknown · another gecko call is running".to_owned(),
        Some(b) => match b.state {
            "suspended" => {
                "suspended (its connection dropped; it may come back within 60 s)".to_owned()
            }
            "gone" => format!("ended: {}", gone_message(b.reason.as_deref())),
            "active" => {
                let approved = b
                    .approved_at
                    .map(|at| format!(" (approved {})", ago(at, now)))
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
    let hub = if data.hub.reachable {
        match data.hub.latency_ms {
            Some(ms) => format!("reachable · {ms} ms"),
            None => "reachable".to_owned(),
        }
    } else {
        "unreachable".to_owned()
    };
    [
        format!("env     {}{who}", data.env),
        format!("tab     {tab}"),
        format!("hub     {hub}"),
    ]
}

/// The status as a result: `--json` gets the fields and the tab line, plain text all
/// three lines.
pub fn status_outcome(data: &StatusData, json: bool, now: u64) -> Outcome {
    let lines = status_lines(data, now);
    if !json {
        return Outcome::done(lines.join("\n"));
    }
    let [_, tab, _] = lines;
    let mut outcome = Outcome::done(tab);
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

/// Gathers the status. Never pairs. Not signed in: exit 6; hub unreachable: exit 4;
/// `busy` (another gecko call holds the grant's one live connection): the local part
/// and a line that says so, exit 0.
pub async fn status(
    args: &[String],
    options: &Options,
    config: &Config,
) -> Result<Outcome, Failure> {
    let env = env_of(args, options, config)?;
    let Some(token) = config.read_token(&env) else {
        return Err(Failure::new(
            "not-logged-in",
            format!("not signed in: run gecko login --env {env}"),
        ));
    };
    let started = Instant::now();
    let data = match session::open(&env, config).await {
        Ok(session) => {
            let latency = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let data = StatusData {
                user: Some(session.status.user.clone()),
                env: env.clone(),
                hub: HubReach {
                    reachable: true,
                    latency_ms: Some(latency),
                },
                binding: session.status.binding.as_ref().map(BindingData::of),
            };
            close(session).await;
            data
        }
        // The 4409 close is the hub's own answer: it is up.
        Err(failure) if failure.code == "busy" => StatusData {
            user: Some(User {
                id: token.user.id,
                name: token.user.name,
            }),
            env: env.clone(),
            hub: HubReach {
                reachable: true,
                latency_ms: None,
            },
            binding: Some(BindingData {
                state: "unknown",
                reason: None,
                approved_at: None,
                page_id: None,
                title: None,
                path: None,
                tab: Some("busy"),
            }),
        },
        Err(failure) => return Err(failure),
    };
    Ok(status_outcome(&data, options.json, now_ms()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(binding: Option<BindingData>) -> StatusData {
        StatusData {
            user: Some(User {
                id: "u1".into(),
                name: "Zahit".into(),
            }),
            env: "dev".into(),
            hub: HubReach {
                reachable: true,
                latency_ms: Some(12),
            },
            binding,
        }
    }

    fn active() -> BindingData {
        BindingData {
            state: "active",
            reason: None,
            approved_at: Some(1_000),
            page_id: Some("p1".into()),
            title: Some("Tasks".into()),
            path: Some("/tasks".into()),
            tab: Some("ok"),
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
                "env     dev · Zahit".to_owned(),
                "tab     bound (approved 7 min ago) · Tasks · /tasks".to_owned(),
                "hub     reachable · 12 ms".to_owned(),
            ]
        );
        assert_eq!(
            status_lines(&data(None), now)[1],
            "tab     none · to bind: gecko attach"
        );
        let gone = BindingData {
            state: "gone",
            reason: Some("closed".into()),
            title: None,
            path: None,
            tab: None,
            ..active()
        };
        assert_eq!(
            status_lines(&data(Some(gone)), now)[1],
            "tab     ended: the bound tab was closed or reloaded; to bind again: gecko attach"
        );
    }

    #[test]
    fn json_carries_data_and_the_tab_line() {
        let outcome = status_outcome(&data(Some(active())), true, 1_000);
        assert_eq!(
            outcome.summary.as_deref(),
            Some("tab     bound (approved just now) · Tasks · /tasks")
        );
        assert_eq!(
            outcome.untrusted,
            vec!["data.binding.title", "data.binding.path"]
        );
        let value = outcome.data.unwrap();
        assert_eq!(value["binding"]["approvedAt"], 1_000);
        assert_eq!(value["binding"]["pageId"], "p1");
        assert_eq!(value["hub"]["latencyMs"], 12);
        assert!(value["binding"].get("reason").is_none());
    }

    #[test]
    fn binding_data_of_a_hub_binding() {
        let binding: Binding = serde_json::from_str(
            r#"{"bindingId":"b","state":"active","pageId":"p","approvedAt":5}"#,
        )
        .unwrap();
        let data = BindingData::of(&binding);
        assert_eq!((data.state, data.tab), ("active", None));
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
