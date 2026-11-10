//! A line for the bound tab (§5.1–§5.3): the CLI's pre-filter, the capability list
//! from the hub, the request, and the wait for its replies, driven through `run.rs`.
//!
//! The pre-filter is not a permission; the tab decides. It keeps two promises: a
//! sensitive line never leaves the CLI, and a command the tab does not list never
//! reaches it with its arguments (§5.2).

use std::time::Duration;

use gecko_hub_protocol::failure::{call_failure, lost_failure};
use gecko_hub_protocol::hub::{Binding, BindingState};
use gecko_hub_protocol::{CallError, Closed, HubConnection, HubEvent};
use gecko_kernel::{Clock, Failure, IdSource, Interrupts, Outcome, Reporter, gone_message};
use gecko_wire::capabilities::{Capabilities, Effect, read_parts, version_below};
use gecko_wire::reply::{Read, Status, read_value};
use gecko_wire::request::{self, Input, check_limits, command_name, fits};
use serde::Deserialize;
use serde_json::Value;
use tokio::time::{Instant, sleep_until};

use crate::args::Options;
use crate::config::Config;
use crate::help;
use crate::ports::Ports;
use crate::run::{Event, Run, Step};
use crate::session::{self, Session, no_binding};

/// The most a send waits for the hub's rate limit in all.
const RATE_WAIT_MAX: Duration = Duration::from_secs(5);

/// The capability list the hub keeps for the binding: `{ revision, parts }`.
#[derive(Deserialize)]
struct Published {
    parts: Vec<String>,
}

fn too_old() -> Failure {
    Failure::new(
        "unsupported-version",
        "the Gecko tab is too old for this CLI; reload the page or wait for the environment to be updated",
    )
}

/// The binding's capability list, checked against what this CLI (`cli_version`)
/// needs (§5.7, §5.8).
pub(crate) fn capabilities_of(
    binding: &Binding,
    cli_version: &str,
) -> Result<Capabilities, Failure> {
    let unreadable = || {
        Failure::new(
            "tab-unreachable",
            "the bound tab has not published its command list; reload the Gecko page",
        )
    };
    let raw = binding.capabilities.as_ref().ok_or_else(unreadable)?;
    let value: Value = serde_json::from_str(raw.get()).map_err(|_| unreadable())?;
    if value.get("kind").is_some() {
        // The list as a `capabilities` reply: a tab from before command-console 1.7.0.
        return Err(too_old());
    }
    let published: Published = serde_json::from_value(value).map_err(|_| unreadable())?;
    let caps = read_parts(&published.parts).map_err(|_| unreadable())?;
    if !caps.missing_features().is_empty() || caps.envelope().is_none() {
        return Err(too_old());
    }
    if let Some(min) = &caps.min_cli
        && version_below(cli_version, min) == Some(true)
    {
        return Err(Failure::new(
            "unsupported-version",
            format!(
                "this gecko ({cli_version}) is older than the tab supports ({min}); update gecko"
            ),
        ));
    }
    Ok(caps)
}

/// The binding to send to, or why there is none. A suspended one counts: while its page
/// comes back, the hub holds the send for a few seconds (attach spec §4).
pub(crate) fn live_binding<H>(session: &Session<H>) -> Result<&Binding, Failure> {
    match &session.status.binding {
        None => Err(no_binding()),
        Some(b) if b.state == BindingState::Gone => {
            Err(Failure::new("gone", gone_message(b.reason.as_deref())))
        }
        Some(b) => Ok(b),
    }
}

/// The input of a line: argv, `--line`, or `--line -` from stdin.
pub fn input_of(argv: Option<Vec<String>>, line: Option<String>) -> Result<Input, Failure> {
    match (argv, line) {
        (Some(argv), _) => Ok(Input::Argv(argv)),
        (None, Some(line)) if line == "-" => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).map_err(|error| {
                Failure::new("usage", format!("cannot read the line from stdin: {error}"))
            })?;
            let text = text.strip_suffix('\n').unwrap_or(&text);
            let text = text.strip_suffix('\r').unwrap_or(text);
            Ok(Input::Line(text.to_owned()))
        }
        (None, Some(line)) => Ok(Input::Line(line)),
        (None, None) => Err(Failure::new("usage", "no command")),
    }
}

/// Checks that need no network: the name rule and the limits (§5.1, §5.3).
pub fn precheck(input: &Input) -> Result<String, Failure> {
    let name = command_name(input).ok_or_else(|| {
        Failure::new(
            "usage",
            "the command name is written plain, without quotes or spaces (letters, digits, . _ -)",
        )
    })?;
    check_limits(input).map_err(|why| Failure::new("too-large", why))?;
    Ok(name.to_owned())
}

/// Sends a line to the bound tab. Without a binding it ends with `no-binding` at once:
/// a binding starts only from a code copied out of a tab (attach spec §3.1).
pub async fn run(
    options: &Options,
    config: &Config,
    ports: &Ports<impl Clock, impl IdSource>,
    input: Input,
    io: &mut impl Reporter,
    interrupts: &mut impl Interrupts,
) -> Result<Outcome, Failure> {
    let name = precheck(&input)?;
    let mut session = session::open(config, ports).await?;
    let result = run_in(options, ports, &mut session, &input, &name, io, interrupts).await;
    session.hub.close().await;
    result
}

/// The note under a server change that ran without the card (attach spec §5.4).
const NO_APPROVAL_NOTE: &str = "(ran without approval: binding level never-ask)";

async fn refresh<H: HubConnection>(session: &mut Session<H>) -> Result<(), Failure> {
    let raw = session
        .hub
        .status()
        .await
        .map_err(|error| call_failure(error, false))?;
    session.status = serde_json::from_str(raw.get()).map_err(|error| {
        Failure::new(
            "failed",
            format!("the hub's status answer is not understood: {error}"),
        )
    })?;
    Ok(())
}

async fn run_in<H: HubConnection>(
    options: &Options,
    ports: &Ports<impl Clock, impl IdSource>,
    session: &mut Session<H>,
    input: &Input,
    name: &str,
    io: &mut impl Reporter,
    interrupts: &mut impl Interrupts,
) -> Result<Outcome, Failure> {
    let mut caps = capabilities_of(live_binding(session)?, ports.cli_version)?;
    if caps.is_sensitive(name) {
        return Err(Failure::new(
            "not-allowed",
            format!(
                "{name} is a sensitive command; it does not run from the CLI, type it in the browser"
            ),
        ));
    }
    if caps.command(name).is_none() {
        // Only the name goes to the tab, for its suggestion (§5.2 step 3).
        if let Some(outcome) = unknown_name(session, ports, &caps, name, interrupts).await? {
            return Ok(outcome);
        }
        // The tab knows it: the list was stale. Read it once more.
        refresh(session).await?;
        caps = capabilities_of(live_binding(session)?, ports.cli_version)?;
        if caps.is_sensitive(name) || caps.command(name).is_none() {
            return Ok(Outcome::error(
                "unknown-command",
                format!("unknown command: {name}"),
                2,
            ));
        }
    }
    let Some(command) = caps.command(name) else {
        return Err(Failure::new(
            "unknown-command",
            format!("unknown command: {name}"),
        ));
    };
    let danger_change = command.danger.is_change();
    let server_change = danger_change && command.effect() == Effect::Server;
    let v = caps.envelope().unwrap_or(1);
    let id = ports.ids.new_id();
    let envelope = request::cli_request(v, &id, input, options.view.as_deref());
    if !fits(&envelope) {
        return Err(Failure::new("too-large", "the request is over 16 KB"));
    }
    let mut run = Run::new(danger_change);
    let stamp = match send(session, &envelope).await {
        Ok(stamp) => stamp,
        Err(Sent::Refused(failure)) => return Err(failure),
        Err(Sent::Lost(why)) => return Ok(end_of(run.on(Event::Lost(why), ports.clock.now_ms()))),
        Err(Sent::Revoked) => return Ok(end_of(run.on(Event::Revoked, ports.clock.now_ms()))),
    };
    let delivery = stamp.delivery_deadline(ports.settings.delivery_margin);
    let mut cancel_by: Option<Instant> = None;
    let far = Instant::now() + Duration::from_secs(365 * 24 * 3600);
    let mut delivered_checked = false;
    let started = Instant::now();
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    let mut interrupted = false;
    loop {
        let event = tokio::select! {
            _ = ticker.tick(), if io.shows_progress() && !interrupted => {
                indicate(io, &run, started, ports.clock.now_ms());
                continue;
            }
            event = session.hub.next_event() => match event {
                Ok(HubEvent::Reply(raw)) => {
                    let Ok(value) = serde_json::from_str::<Value>(raw.get()) else { continue };
                    if value.get("id").and_then(Value::as_str) != Some(id.as_str()) {
                        continue;
                    }
                    match read_value(&value) {
                        Read::Reply(reply) => Event::Reply(reply),
                        Read::Ignored | Read::Invalid(_) => continue,
                    }
                }
                Ok(HubEvent::Binding(raw)) => match serde_json::from_str::<Binding>(raw.get()) {
                    Ok(b) if b.state == BindingState::Gone && b.reason.as_deref() == Some("revoked") => Event::Revoked,
                    Ok(b) if b.state == BindingState::Gone => Event::Lost(gone_message(b.reason.as_deref())),
                    Ok(b) if b.state == BindingState::Suspended => Event::TabLeft,
                    _ => continue,
                },
                Ok(_) => continue,
                Err(Closed::Revoked) => Event::Revoked,
                Err(closed) => Event::Lost(lost_failure(closed).message),
            },
            () = sleep_until(delivery), if !delivered_checked => {
                delivered_checked = true;
                Event::DeliveryTimeout
            }
            () = sleep_until(cancel_by.unwrap_or(far)), if cancel_by.is_some() => {
                cancel_by = None;
                Event::CancelWaitOver
            }
            () = interrupts.next() => Event::Interrupt,
        };
        match run.on(event, ports.clock.now_ms()) {
            Step::Wait => {}
            Step::Info(text) => io.info(&text),
            Step::Approval(text) => {
                if io.shows_progress() {
                    indicate(io, &run, started, ports.clock.now_ms());
                } else {
                    io.info(&text);
                }
            }
            Step::SendCancel => {
                interrupted = true;
                io.clear_progress();
                let cancel = request::cancel(v, &id);
                let sent = session.hub.send(&raw(&cancel)).await.is_ok();
                cancel_by = Some(if sent {
                    Instant::now() + ports.settings.cancel_wait
                } else {
                    Instant::now()
                });
            }
            Step::End(outcome) => {
                if !io.json()
                    && (options.view.is_some()
                        || name.to_ascii_lowercase().starts_with("dataview."))
                    && let Some(view) = &outcome.view
                {
                    io.info(&format!(
                        "View: {}",
                        view.title.as_deref().unwrap_or(&view.id)
                    ));
                }
                if !io.json()
                    && server_change
                    && outcome.status == "ok"
                    && outcome.approval.as_deref() == Some("none")
                {
                    io.info(NO_APPROVAL_NOTE);
                }
                return Ok(*outcome);
            }
        }
    }
}

/// After this long a call shows the wait indicator (UX U3).
const PROGRESS_AFTER: Duration = Duration::from_millis(700);

/// The wait indicator's text at `now` (ms since the epoch): the approval's time left,
/// or how long the call runs.
fn indicate(io: &mut impl Reporter, run: &Run, started: Instant, now: u64) {
    if let Some(expires_at) = run.approval_expires_at() {
        let left = expires_at - now as f64;
        io.progress(&format!(
            "Waiting for approval ({}): approve it in the Gecko tab in the browser",
            crate::run::approx_left(left)
        ));
        return;
    }
    let elapsed = started.elapsed();
    if elapsed < PROGRESS_AFTER {
        return;
    }
    let state = if run.received() {
        "running in the tab"
    } else {
        "reaching the tab"
    };
    io.progress(&format!("{state}… {:.1} s", elapsed.as_secs_f64()));
}

fn end_of(step: Step) -> Outcome {
    match step {
        Step::End(outcome) => *outcome,
        _ => Outcome::error("failed", "the request ended without a result", 1),
    }
}

pub(crate) fn raw(value: &Value) -> Box<serde_json::value::RawValue> {
    serde_json::value::to_raw_value(value).unwrap_or_else(|_| {
        serde_json::value::RawValue::from_string("{}".to_owned()).unwrap_or_default()
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Stamp {
    pub stamped_at: u64,
    pub expires_at: u64,
}

impl Stamp {
    /// When an unanswered request counts as not delivered: the hub's window plus
    /// `margin` (`GECKO_DELIVERY_MARGIN_MS`).
    pub fn delivery_deadline(&self, margin: Duration) -> Instant {
        Instant::now()
            + Duration::from_millis(self.expires_at.saturating_sub(self.stamped_at))
            + margin
    }
}

pub(crate) enum Sent {
    /// The hub refused it: nothing reached the tab.
    Refused(Failure),
    /// The connection ended while sending: it may have gone out.
    Lost(String),
    Revoked,
}

/// `send`, retried while the hub answers `rate-limited`; a `busy` is never retried.
pub(crate) async fn send<H: HubConnection>(
    session: &mut Session<H>,
    envelope: &Value,
) -> Result<Stamp, Sent> {
    let body = raw(envelope);
    let mut waited = Duration::ZERO;
    loop {
        match session.hub.send(&body).await {
            Ok(answer) => {
                return serde_json::from_str(answer.get()).map_err(|_| {
                    Sent::Refused(Failure::new(
                        "failed",
                        "the hub's send answer is not understood",
                    ))
                });
            }
            Err(CallError::Hub(error)) if error.code == "rate-limited" => {
                let wait = Duration::from_millis(error.retry_after_ms.unwrap_or(200).max(1));
                if waited + wait > RATE_WAIT_MAX {
                    return Err(Sent::Refused(Failure::new(
                        "busy",
                        "rate limit: a few requests a second per grant; try again shortly",
                    )));
                }
                waited += wait;
                tokio::time::sleep(wait).await;
            }
            Err(CallError::Hub(error)) => {
                return Err(Sent::Refused(match error.code.as_str() {
                    "gone" => Failure::new("gone", gone_message(error.reason.as_deref())),
                    "tab-unreachable" => Failure::new(
                        "tab-unreachable",
                        "the bound tab did not answer in time (it is reloading or its connection dropped); safe to run again",
                    ),
                    "no-binding" => no_binding(),
                    "busy" => Failure::new("busy", "the terminal is busy; try again"),
                    _ => Failure::new(error.code.clone(), error.message.unwrap_or(error.code)),
                }));
            }
            Err(CallError::Closed(Closed::Revoked)) => return Err(Sent::Revoked),
            Err(CallError::Closed(closed)) => return Err(Sent::Lost(lost_failure(closed).message)),
            Err(CallError::TimedOut) => {
                return Err(Sent::Lost(
                    "the hub did not answer the send in time".to_owned(),
                ));
            }
        }
    }
}

/// Asks the tab about a name it does not list: its `unknown-command` and suggestion,
/// or `None` when the tab does know it.
async fn unknown_name<H: HubConnection>(
    session: &mut Session<H>,
    ports: &Ports<impl Clock, impl IdSource>,
    caps: &Capabilities,
    name: &str,
    interrupts: &mut impl Interrupts,
) -> Result<Option<Outcome>, Failure> {
    let v = caps.envelope().unwrap_or(1);
    let answer = help::ask(
        session,
        ports,
        v,
        &[name.to_owned()],
        true,
        None,
        interrupts,
    )
    .await?;
    if let Some(outcome) = help::ended(&answer) {
        return Ok(Some(outcome));
    }
    let help::Answer::Result(help) = answer else {
        return Ok(None);
    };
    if help.status == Status::Ok {
        return Ok(None);
    }
    let code = help
        .failure
        .code
        .clone()
        .unwrap_or_else(|| "unknown-command".to_owned());
    let message = help
        .failure
        .message
        .clone()
        .unwrap_or_else(|| format!("unknown command: {name}"));
    let mut outcome = Outcome::error(code, message, help.exit_code());
    // The suggestion names a command: `gecko <name>`.
    outcome.suggest = help.suggest.clone();
    if help.failure.message.is_some() {
        outcome.untrusted.push("message");
    }
    Ok(Some(outcome))
}
