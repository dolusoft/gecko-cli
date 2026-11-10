//! One request's life after it is sent (§5.2, §5.6; command-console 1.7.0
//! `fixtures/remote/sequence/`), as a pure state machine: replies, the end of the
//! connection, the delivery window and Ctrl+C go in, in order; the result comes out.
//! The driver (`gecko-command-module`'s `run_line`) owns the connection and reads the
//! clock: each event comes with its time (`Run::on`).
//!
//! The CLI's own observation decides between "not run" (4) and "unknown" (5): before
//! `received` the command's `danger` counts, after it the tab's `change` flag does.

use gecko_wire::reply::{Chunk, Finished, Reply, Status};

use gecko_kernel::{Outcome, ViewInfo};

/// Something that happened while the request was out.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A reply for this request (other ids are filtered out before).
    Reply(Reply),
    /// The hub connection or the binding ended; carries the reader's text.
    Lost(String),
    /// The bound page left (reload, close, a dropped connection): the binding is
    /// suspended and the request's page is gone. An open approval card goes with it
    /// (attach spec §4: exit 3).
    TabLeft,
    /// The grant was revoked: an identity error, whatever ran.
    Revoked,
    /// The hub's window (`expiresAt - stampedAt`) and the margin passed with no
    /// `received`.
    DeliveryTimeout,
    /// Ctrl+C.
    Interrupt,
    /// The wait for the tab's answer to a cancel is over (§6.6: 3 s).
    CancelWaitOver,
}

/// What the driver does next.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Wait,
    /// A line for stderr.
    Info(String),
    /// The tab waits for the user's approval; the line for stderr when no wait
    /// indicator counts it down.
    Approval(String),
    /// Send `cancel` and start the cancel wait.
    SendCancel,
    End(Box<Outcome>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CancelAnswer {
    BeforeStart,
    Requested,
}

#[derive(Debug)]
pub struct Run {
    /// The command's `danger` is `change` (or unknown).
    danger_change: bool,
    received: Option<gecko_wire::reply::Received>,
    /// Set by `awaiting-approval`: the tab is about to change data.
    approving: bool,
    /// When the approval runs out, on the tab's clock.
    approval_expires_at: Option<f64>,
    chunks: Vec<Chunk>,
    interrupted: bool,
    cancel_answer: Option<CancelAnswer>,
    ended: bool,
}

/// "~2 min", "~45 s": the time left of an approval, approximate (the tab's clock).
pub fn approx_left(ms: f64) -> String {
    let left = ms.max(0.0);
    if left >= 60_000.0 {
        format!("~{} min", (left / 60_000.0).round())
    } else {
        format!("~{} s", (left / 1000.0).ceil())
    }
}

impl Run {
    pub fn new(danger_change: bool) -> Run {
        Run {
            danger_change,
            received: None,
            approving: false,
            approval_expires_at: None,
            chunks: Vec::new(),
            interrupted: false,
            cancel_answer: None,
            ended: false,
        }
    }

    /// Whether the tab accepted the request (`received`).
    pub fn received(&self) -> bool {
        self.received.is_some()
    }

    /// When the approval the tab waits for runs out (ms since the epoch, its clock).
    pub fn approval_expires_at(&self) -> Option<f64> {
        self.approval_expires_at
    }

    /// Whether the request may have changed data, as far as the CLI can tell.
    fn changes(&self) -> bool {
        match &self.received {
            Some(received) => received.change || self.approving,
            None => self.danger_change || self.approving,
        }
    }

    fn end(&mut self, outcome: Outcome) -> Step {
        self.ended = true;
        Step::End(Box::new(outcome))
    }

    fn cancelled(&mut self, answer: Option<CancelAnswer>) -> Step {
        let (code, message) = match answer {
            Some(CancelAnswer::BeforeStart) => {
                ("cancelled-before-start", "cancelled before the command ran")
            }
            Some(CancelAnswer::Requested) if self.changes() => (
                "cancel-requested",
                "cancel requested; the command was running, backend work it started may go on",
            ),
            Some(CancelAnswer::Requested) => (
                "cancel-requested",
                "cancel requested; the command's local work stopped",
            ),
            None => ("cancelled", "cancelled; the outcome is unknown"),
        };
        let mut outcome = Outcome::error(code, message, 130);
        outcome.complete = false;
        self.end(outcome)
    }

    fn lost(&mut self, why: &str) -> Step {
        if self.interrupted {
            return self.cancelled(None);
        }
        if self.changes() {
            let detail = if self.received.is_some() {
                "the request may have run; check in the browser"
            } else {
                "the request may be on its way; check in the browser"
            };
            return self.end(Outcome::error(
                "unknown-outcome",
                format!("{why}; {detail}"),
                5,
            ));
        }
        self.end(Outcome::error(
            "connection-lost",
            format!("{why}; a read command is safe to run again"),
            4,
        ))
    }

    /// Feeds one event that happened at `now` (ms since the epoch, the CLI's clock);
    /// after an `End` every further event is ignored.
    pub fn on(&mut self, event: Event, now: u64) -> Step {
        if self.ended {
            return Step::Wait;
        }
        match event {
            Event::Reply(reply) => self.on_reply(reply, now),
            Event::Lost(why) => self.lost(&why),
            Event::TabLeft if self.approving && !self.interrupted => self.end(Outcome::error(
                "approval-dropped",
                "the Gecko tab reloaded or closed while the approval card was open; the command did not run",
                3,
            )),
            Event::TabLeft => self.lost("the bound tab reloaded, closed or lost its connection"),
            Event::Revoked => self.end(Outcome::error(
                "revoked",
                format!(
                    "the binding ended; work that started may go on. {}",
                    gecko_kernel::BIND_AGAIN
                ),
                6,
            )),
            Event::DeliveryTimeout => {
                if self.received.is_some() || self.interrupted {
                    return Step::Wait;
                }
                if self.changes() {
                    // Past `expiresAt` the tab does not run it, but it may have run it
                    // before, with its `received` lost on the way.
                    return self.end(Outcome::error(
                        "unknown-outcome",
                        "no answer from the tab; the request may have run, check in the browser",
                        5,
                    ));
                }
                self.end(Outcome::error(
                    "not-delivered",
                    "the request did not reach the tab and did not run; safe to run again",
                    4,
                ))
            }
            Event::Interrupt => {
                if self.interrupted {
                    // The second Ctrl+C ends the wait.
                    return self.cancelled(None);
                }
                self.interrupted = true;
                Step::SendCancel
            }
            Event::CancelWaitOver => {
                let answer = self.cancel_answer;
                self.cancelled(answer)
            }
        }
    }

    fn on_reply(&mut self, reply: Reply, now: u64) -> Step {
        match reply {
            Reply::Received(received) => {
                let info = received.hidden.then(|| match received.data_age_ms {
                    Some(age) => format!(
                        "the tab is in the background; its data is {} s old",
                        (age / 1000.0).round()
                    ),
                    None => "the tab is in the background".to_owned(),
                });
                self.received = Some(received);
                info.map_or(Step::Wait, Step::Info)
            }
            Reply::AwaitingApproval { expires_at, .. } => {
                self.approving = true;
                self.approval_expires_at = Some(expires_at);
                Step::Approval(format!(
                    "Waiting for approval ({}): approve it in the Gecko tab in the browser.",
                    approx_left(expires_at - now as f64)
                ))
            }
            Reply::Chunk(chunk) => {
                self.chunks.push(chunk);
                Step::Wait
            }
            Reply::CancelRequested { .. } => {
                self.cancel_answer = Some(CancelAnswer::Requested);
                Step::Wait
            }
            Reply::CancelledBeforeStart { .. } => {
                self.cancel_answer = Some(CancelAnswer::BeforeStart);
                Step::Wait
            }
            Reply::Finished(finished) => self.finish(*finished),
            // A help result has no place in a command's run.
            Reply::HelpResult(_) => Step::Wait,
        }
    }

    fn finish(&mut self, finished: Finished) -> Step {
        if self.interrupted
            && finished.status == Status::Error
            && finished.failure.code.as_deref() == Some("cancelled")
        {
            let answer = self.cancel_answer.or(Some(CancelAnswer::Requested));
            return self.cancelled(answer);
        }
        let mut outcome = match finished.status {
            Status::Error => self.tab_error(&finished),
            Status::Ok => {
                let mut outcome = Outcome::ok();
                outcome.summary = finished.summary.clone();
                if finished.chunks > 0 {
                    match self.joined(&finished) {
                        Some(data) => outcome.data = Some(data),
                        None => outcome.complete = false,
                    }
                }
                outcome.preview = finished.preview;
                outcome.next = finished.next.clone();
                outcome
            }
        };
        // How the change ran, as the tab wrote it (attach spec §5.4).
        outcome.approval = finished.approval.clone();
        outcome.suggest = finished.suggest.clone();
        outcome.help = finished.help.clone();
        outcome.view = finished.view.as_ref().map(|view| ViewInfo {
            id: view.id.clone(),
            title: view.title.clone(),
        });
        if let Some(received) = &self.received {
            outcome.hidden = Some(received.hidden);
            outcome.data_age_ms = Some(
                received
                    .data_age_ms
                    .and_then(serde_json::Number::from_f64)
                    .map_or(serde_json::Value::Null, serde_json::Value::Number),
            );
        }
        let from_tab_message = finished.failure.message.is_some();
        mark_untrusted(&mut outcome, from_tab_message);
        self.end(outcome)
    }

    /// A `finished` error: the class decides, but the CLI's own observation wins
    /// (A9, F18): a `not-run` after `received` with `change` is of unknown outcome.
    fn tab_error(&self, finished: &Finished) -> Outcome {
        let code = finished
            .failure
            .code
            .clone()
            .unwrap_or_else(|| "failed".to_owned());
        let message = finished
            .failure
            .message
            .clone()
            .unwrap_or_else(|| code.clone());
        let exit = finished.exit_code();
        if exit == 4 && self.received.as_ref().is_some_and(|r| r.change) {
            return Outcome::error(
                code,
                format!(
                    "{message}; but the request had started changing data, check in the browser"
                ),
                5,
            );
        }
        Outcome::error(code, message, exit)
    }

    /// The result's data from its chunks; `None` when a part is missing, repeated,
    /// of the wrong size, or the text is not JSON.
    fn joined(&self, finished: &Finished) -> Option<serde_json::Value> {
        let total = finished.chunks;
        if self.chunks.len() as u64 != total {
            return None;
        }
        let mut parts: Vec<Option<&Chunk>> = vec![None; usize::try_from(total).ok()?];
        for chunk in &self.chunks {
            let slot = parts.get_mut(usize::try_from(chunk.index).ok()?)?;
            if slot.is_some() || chunk.total != total || chunk.bytes != chunk.data.len() as u64 {
                return None;
            }
            *slot = Some(chunk);
        }
        let text: String = parts
            .iter()
            .map(|p| p.map(|c| c.data.as_str()))
            .collect::<Option<_>>()?;
        if text.len() as u64 != finished.bytes {
            return None;
        }
        serde_json::from_str(&text).ok()
    }
}

/// The outcome of a step that should end the run; any other step is a run that ended
/// without a result.
pub fn end_of(step: Step) -> Outcome {
    match step {
        Step::End(outcome) => *outcome,
        _ => Outcome::error("failed", "the request ended without a result", 1),
    }
}

/// The free-text fields that come from user input or data (§6.5).
fn mark_untrusted(outcome: &mut Outcome, message_from_tab: bool) {
    let mut untrusted = Vec::new();
    if outcome.summary.is_some() {
        untrusted.push("summary");
    }
    if outcome.data.is_some() {
        untrusted.push("data");
    }
    if outcome.message.is_some() && message_from_tab {
        untrusted.push("message");
    }
    if outcome.view.as_ref().is_some_and(|v| v.title.is_some()) {
        untrusted.push("view.title");
    }
    outcome.untrusted = untrusted;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approx() {
        assert_eq!(approx_left(120_000.0), "~2 min");
        assert_eq!(approx_left(44_100.0), "~45 s");
        assert_eq!(approx_left(-5.0), "~0 s");
    }

    #[test]
    fn after_the_end_nothing_counts() {
        let mut run = Run::new(false);
        assert!(matches!(run.on(Event::Lost("x".into()), 0), Step::End(_)));
        assert_eq!(run.on(Event::Interrupt, 0), Step::Wait);
    }

    #[test]
    fn second_interrupt_ends() {
        let mut run = Run::new(false);
        assert_eq!(run.on(Event::Interrupt, 0), Step::SendCancel);
        let Step::End(outcome) = run.on(Event::Interrupt, 0) else {
            panic!()
        };
        assert_eq!((outcome.status, outcome.exit()), ("cancelled", 130));
    }

    #[test]
    fn a_reload_drops_an_open_approval_card() {
        let mut run = Run::new(true);
        run.on(
            Event::Reply(Reply::AwaitingApproval {
                id: "r".into(),
                expires_at: 1.0,
            }),
            0,
        );
        let Step::End(outcome) = run.on(Event::TabLeft, 0) else {
            panic!()
        };
        assert_eq!(
            (outcome.code.as_deref(), outcome.exit()),
            (Some("approval-dropped"), 3)
        );
        // Without a card it is a lost connection: a read is safe to run again.
        let mut read = Run::new(false);
        let Step::End(outcome) = read.on(Event::TabLeft, 0) else {
            panic!()
        };
        assert_eq!(outcome.exit(), 4);
    }

    #[test]
    fn approval_passes_through() {
        let mut run = Run::new(true);
        let finished = Finished {
            id: "r".into(),
            status: Status::Ok,
            summary: Some("Task 8812 stopped".into()),
            failure: gecko_wire::reply::Failure {
                code: None,
                class: None,
                message: None,
            },
            chunks: 0,
            bytes: 0,
            view: None,
            preview: false,
            suggest: None,
            next: None,
            help: None,
            approval: Some("card".into()),
        };
        let Step::End(outcome) = run.on(Event::Reply(Reply::Finished(Box::new(finished))), 0)
        else {
            panic!()
        };
        assert_eq!(outcome.approval.as_deref(), Some("card"));
    }
}
