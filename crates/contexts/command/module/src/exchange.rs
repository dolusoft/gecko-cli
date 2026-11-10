//! What both slices do with the hub: `send` (retried while the hub's rate limit
//! answers), reading the replies to one request, re-reading the binding's status, and
//! one `help-request` with the wait for its `help-result`.

use std::time::Duration;

use gecko_command_domain::help::Answer;
use gecko_command_domain::line::Stamp;
use gecko_hub_protocol::failure::{call_failure, lost_failure};
use gecko_hub_protocol::hub::HubStatus;
use gecko_hub_protocol::{CallError, Closed, HubConnection, HubEvent};
use gecko_kernel::{Clock, Failure, IdSource, Interrupts, gone_message, no_binding};
use gecko_wire::reply::{Read, Reply, read_value};
use gecko_wire::request;
use serde_json::Value;
use serde_json::value::RawValue;
use tokio::time::{Instant, sleep_until};

use crate::Ports;

/// The most a send waits for the hub's rate limit in all.
const RATE_WAIT_MAX: Duration = Duration::from_secs(5);

pub(crate) fn raw(value: &Value) -> Box<RawValue> {
    serde_json::value::to_raw_value(value)
        .unwrap_or_else(|_| RawValue::from_string("{}".to_owned()).unwrap_or_default())
}

/// When an unanswered request sent now counts as not delivered.
pub(crate) fn delivery_deadline(stamp: &Stamp, margin: Duration) -> Instant {
    Instant::now() + stamp.delivery_window(margin)
}

/// The reply in a hub event's payload when it answers request `id`; other ids, kinds
/// the CLI does not read and invalid replies are passed over.
pub(crate) fn reply_to(payload: &RawValue, id: &str) -> Option<Reply> {
    let value = serde_json::from_str::<Value>(payload.get()).ok()?;
    if value.get("id").and_then(Value::as_str) != Some(id) {
        return None;
    }
    match read_value(&value) {
        Read::Reply(reply) => Some(reply),
        Read::Ignored | Read::Invalid(_) => None,
    }
}

/// Reads the hub's `status` once more (a stale capability list).
pub(crate) async fn refresh<H: HubConnection>(
    hub: &mut H,
    status: &mut HubStatus,
) -> Result<(), Failure> {
    let raw = hub
        .status()
        .await
        .map_err(|error| call_failure(error, false))?;
    *status = serde_json::from_str(raw.get()).map_err(|error| {
        Failure::new(
            "failed",
            format!("the hub's status answer is not understood: {error}"),
        )
    })?;
    Ok(())
}

pub(crate) enum Sent {
    /// The hub refused it: nothing reached the tab.
    Refused(Failure),
    /// The connection ended while sending: it may have gone out.
    Lost(String),
    Revoked,
}

/// `send`, retried while the hub answers `rate-limited`; a `busy` is never retried.
pub(crate) async fn send<H: HubConnection>(hub: &mut H, envelope: &Value) -> Result<Stamp, Sent> {
    let body = raw(envelope);
    let mut waited = Duration::ZERO;
    loop {
        match hub.send(&body).await {
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

/// Sends one `help-request` and waits for its `help-result`. Help changes nothing,
/// so a lost answer is safe to ask again.
pub(crate) async fn ask<H: HubConnection>(
    hub: &mut H,
    ports: &Ports<'_, impl Clock, impl IdSource>,
    v: u64,
    argv: &[String],
    json_wanted: bool,
    width: Option<u16>,
    interrupts: &mut impl Interrupts,
) -> Result<Answer, Failure> {
    let id = ports.new_id();
    let envelope = request::help_request(v, &id, argv, json_wanted, width);
    let stamp = match send(hub, &envelope).await {
        Ok(stamp) => stamp,
        Err(Sent::Refused(failure)) => return Err(failure),
        Err(Sent::Lost(why)) => return Ok(Answer::Lost(why)),
        Err(Sent::Revoked) => return Ok(Answer::Revoked),
    };
    let deadline = delivery_deadline(&stamp, ports.settings.delivery_margin);
    loop {
        tokio::select! {
            event = hub.next_event() => match event {
                Ok(HubEvent::Reply(payload)) => {
                    if let Some(Reply::HelpResult(help)) = reply_to(&payload, &id) {
                        return Ok(Answer::Result(help));
                    }
                }
                Ok(_) => {}
                Err(Closed::Revoked) => return Ok(Answer::Revoked),
                Err(closed) => return Ok(Answer::Lost(lost_failure(closed).message)),
            },
            () = sleep_until(deadline) => return Ok(Answer::NotDelivered),
            () = interrupts.next() => return Ok(Answer::Interrupted),
        }
    }
}
