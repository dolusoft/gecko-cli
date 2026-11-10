//! A line for the bound tab (§5.1–§5.3): the pre-filter against the tab's capability
//! list, the request, and the wait for its replies, driven through the domain's `Run`.
//! The rules are the domain's (`gecko_command_domain::line`); this slice owns the
//! connection, the timers and the reader's stderr.

use std::time::Duration;

use gecko_command_domain::help::{Answer, ended};
use gecko_command_domain::line::{
    Verdict, capabilities_of, live_binding, not_listed, pre_filter, sensitive, unknown_command,
};
use gecko_command_domain::run::{Event, Run, Step, approx_left, end_of};
use gecko_hub_protocol::failure::lost_failure;
use gecko_hub_protocol::hub::{Binding, BindingState, HubStatus};
use gecko_hub_protocol::{Closed, HubConnection, HubEvent};
use gecko_kernel::{Clock, Failure, IdSource, Interrupts, Outcome, Reporter, gone_message};
use gecko_wire::capabilities::{Capabilities, Effect};
use gecko_wire::request::{self, Input, fits};
use tokio::time::{Instant, sleep_until};

use crate::Ports;
use crate::exchange::{Sent, ask, delivery_deadline, raw, refresh, reply_to, send};

/// The note under a server change that ran without the card (attach spec §5.4).
const NO_APPROVAL_NOTE: &str = "(ran without approval: binding level never-ask)";

/// After this long a call shows the wait indicator (UX U3).
const PROGRESS_AFTER: Duration = Duration::from_millis(700);

/// Sends `input` (its command `name`, from the domain's `precheck`) to the bound tab
/// on an open connection whose `status` is `status`, and waits for its result. `view`
/// is `--view`.
#[allow(clippy::too_many_arguments)]
pub async fn run_line<H: HubConnection>(
    hub: &mut H,
    status: &mut HubStatus,
    ports: &Ports<'_, impl Clock, impl IdSource>,
    input: &Input,
    name: &str,
    view: Option<&str>,
    io: &mut impl Reporter,
    interrupts: &mut impl Interrupts,
) -> Result<Outcome, Failure> {
    let mut caps = capabilities_of(live_binding(status)?, ports.cli_version)?;
    match pre_filter(&caps, name) {
        Verdict::Send => {}
        Verdict::Sensitive => return Err(sensitive(name)),
        Verdict::NotListed => {
            // Only the name goes to the tab, for its suggestion (§5.2 step 3).
            if let Some(outcome) = unknown_name(hub, ports, &caps, name, interrupts).await? {
                return Ok(outcome);
            }
            // The tab knows it: the list was stale. Read it once more.
            refresh(hub, status).await?;
            caps = capabilities_of(live_binding(status)?, ports.cli_version)?;
            if pre_filter(&caps, name) != Verdict::Send {
                return Ok(unknown_command(name));
            }
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
    let id = ports.new_id();
    let envelope = request::cli_request(v, &id, input, view);
    if !fits(&envelope) {
        return Err(Failure::new("too-large", "the request is over 16 KB"));
    }
    let mut run = Run::new(danger_change);
    let stamp = match send(hub, &envelope).await {
        Ok(stamp) => stamp,
        Err(Sent::Refused(failure)) => return Err(failure),
        Err(Sent::Lost(why)) => return Ok(end_of(run.on(Event::Lost(why), ports.now_ms()))),
        Err(Sent::Revoked) => return Ok(end_of(run.on(Event::Revoked, ports.now_ms()))),
    };
    let delivery = delivery_deadline(&stamp, ports.settings.delivery_margin);
    let mut cancel_by: Option<Instant> = None;
    let far = Instant::now() + Duration::from_secs(365 * 24 * 3600);
    let mut delivered_checked = false;
    let started = Instant::now();
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    let mut interrupted = false;
    loop {
        let event = tokio::select! {
            _ = ticker.tick(), if io.shows_progress() && !interrupted => {
                indicate(io, &run, started, ports.now_ms());
                continue;
            }
            event = hub.next_event() => match event {
                Ok(HubEvent::Reply(payload)) => match reply_to(&payload, &id) {
                    Some(reply) => Event::Reply(reply),
                    None => continue,
                },
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
        match run.on(event, ports.now_ms()) {
            Step::Wait => {}
            Step::Info(text) => io.info(&text),
            Step::Approval(text) => {
                if io.shows_progress() {
                    indicate(io, &run, started, ports.now_ms());
                } else {
                    io.info(&text);
                }
            }
            Step::SendCancel => {
                interrupted = true;
                io.clear_progress();
                let cancel = request::cancel(v, &id);
                let sent = hub.send(&raw(&cancel)).await.is_ok();
                cancel_by = Some(if sent {
                    Instant::now() + ports.settings.cancel_wait
                } else {
                    Instant::now()
                });
            }
            Step::End(outcome) => {
                if !io.json()
                    && (view.is_some() || name.to_ascii_lowercase().starts_with("dataview."))
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

/// The wait indicator's text at `now` (ms since the epoch): the approval's time left,
/// or how long the call runs.
fn indicate(io: &mut impl Reporter, run: &Run, started: Instant, now: u64) {
    if let Some(expires_at) = run.approval_expires_at() {
        let left = expires_at - now as f64;
        io.progress(&format!(
            "Waiting for approval ({}): approve it in the Gecko tab in the browser",
            approx_left(left)
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

/// Asks the tab about a name it does not list: its `unknown-command` and suggestion,
/// or `None` when the tab does know it.
async fn unknown_name<H: HubConnection>(
    hub: &mut H,
    ports: &Ports<'_, impl Clock, impl IdSource>,
    caps: &Capabilities,
    name: &str,
    interrupts: &mut impl Interrupts,
) -> Result<Option<Outcome>, Failure> {
    let v = caps.envelope().unwrap_or(1);
    let answer = ask(hub, ports, v, &[name.to_owned()], true, None, interrupts).await?;
    if let Some(outcome) = ended(&answer) {
        return Ok(Some(outcome));
    }
    let Answer::Result(help) = answer else {
        return Ok(None);
    };
    Ok(not_listed(&help, name))
}
