//! The tab's replies, built in one place (command-console 1.8.0, "From the tab").
//!
//! Every envelope is read back through `gecko-wire`, the CLI's own reader, as soon as
//! it is built; it must come out as `Read::Reply` of the kind it was built as. A reply
//! the CLI would ignore or refuse fails the test where it was made, not later as a
//! hang. `tests/envelopes.rs` builds every example of `valid/replies.json` here and
//! compares them field for field.
//!
//! The kinds are those of the contract: `received`, `awaiting-approval`, `chunk`,
//! `finished`, `cancel-requested`, `cancelled-before-start` and `help-result`.

use gecko_wire::{Read, Reply, read_value};
use serde_json::{Map, Value, json};

/// The kinds a tab sends, as the contract names them.
pub const KINDS: [&str; 7] = [
    "received",
    "awaiting-approval",
    "chunk",
    "finished",
    "cancel-requested",
    "cancelled-before-start",
    "help-result",
];

/// A reply the CLI reads as the kind it was built as.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    value: Value,
}

impl Envelope {
    pub fn value(&self) -> &Value {
        &self.value
    }

    pub fn kind(&self) -> &str {
        self.value["kind"].as_str().unwrap_or_default()
    }

    pub fn id(&self) -> &str {
        self.value["id"].as_str().unwrap_or_default()
    }
}

fn kind_of(reply: &Reply) -> &'static str {
    match reply {
        Reply::Received(_) => "received",
        Reply::AwaitingApproval { .. } => "awaiting-approval",
        Reply::Chunk(_) => "chunk",
        Reply::Finished(_) => "finished",
        Reply::HelpResult(_) => "help-result",
        Reply::CancelRequested { .. } => "cancel-requested",
        Reply::CancelledBeforeStart { .. } => "cancelled-before-start",
    }
}

/// The one door out: `gecko-wire` reads `value` as a reply of `kind`, or the test fails.
fn checked(value: Value) -> Envelope {
    let kind = value["kind"].as_str().unwrap_or_default().to_owned();
    match read_value(&value) {
        Read::Reply(reply) if kind_of(&reply) == kind => Envelope { value },
        other => panic!("the testkit built a {kind} reply the CLI reads as {other:?}: {value}"),
    }
}

fn head(kind: &str, id: &str) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("v".into(), json!(1));
    map.insert("kind".into(), json!(kind));
    map.insert("id".into(), json!(id));
    map
}

/// A reply taken as it is from a fixture (a `sequence/` example's `replies`), with the
/// request's real `id` in place of the example's; it passes the same door as every
/// built reply.
pub fn replay(reply: &Value, id: &str) -> Envelope {
    assert!(reply.is_object(), "a fixture reply is an object: {reply}");
    let mut value = reply.clone();
    value["id"] = json!(id);
    checked(value)
}

/// The fields of `received` after `id`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Received {
    /// The page was hidden when it accepted the request.
    pub hidden: bool,
    /// The tab's data age; `None` is `null` (unknown).
    pub data_age_ms: Option<u64>,
    /// The request runs the path that changes data.
    pub change: bool,
    pub view_relevant: bool,
}

/// `received`: the tab accepted the request.
pub fn received(id: &str, fields: Received) -> Envelope {
    let mut map = head("received", id);
    map.insert("hidden".into(), json!(fields.hidden));
    map.insert("dataAgeMs".into(), json!(fields.data_age_ms));
    map.insert("change".into(), json!(fields.change));
    map.insert("viewRelevant".into(), json!(fields.view_relevant));
    checked(Value::Object(map))
}

/// `received` of a read: visible, age unknown, no change.
pub fn received_read(id: &str) -> Envelope {
    received(id, Received::default())
}

/// `received` of a change: visible, age unknown.
pub fn received_change(id: &str) -> Envelope {
    received(
        id,
        Received {
            change: true,
            ..Received::default()
        },
    )
}

/// `awaiting-approval`: the card is open until `expires_at` (the tab's clock, ms).
pub fn awaiting_approval(id: &str, expires_at: u64) -> Envelope {
    let mut map = head("awaiting-approval", id);
    map.insert("expiresAt".into(), json!(expires_at));
    checked(Value::Object(map))
}

/// One `chunk` of the result's JSON text; `bytes` is its UTF-8 length.
pub fn chunk(id: &str, index: u64, total: u64, data: &str) -> Envelope {
    let mut map = head("chunk", id);
    map.insert("index".into(), json!(index));
    map.insert("total".into(), json!(total));
    map.insert("bytes".into(), json!(data.len()));
    map.insert("data".into(), json!(data));
    checked(Value::Object(map))
}

pub fn cancel_requested(id: &str) -> Envelope {
    checked(Value::Object(head("cancel-requested", id)))
}

pub fn cancelled_before_start(id: &str) -> Envelope {
    checked(Value::Object(head("cancelled-before-start", id)))
}

/// The `outcome` of a `finished`, or the top of a `help-result`.
#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    Ok {
        summary: Option<String>,
    },
    Error {
        code: String,
        class: Option<String>,
        message: Option<String>,
    },
}

impl Outcome {
    fn write(&self, map: &mut Map<String, Value>) {
        match self {
            Outcome::Ok { summary } => {
                map.insert("status".into(), json!("ok"));
                if let Some(summary) = summary {
                    map.insert("summary".into(), json!(summary));
                }
            }
            Outcome::Error {
                code,
                class,
                message,
            } => {
                map.insert("status".into(), json!("error"));
                map.insert("code".into(), json!(code));
                if let Some(class) = class {
                    map.insert("class".into(), json!(class));
                }
                if let Some(message) = message {
                    map.insert("message".into(), json!(message));
                }
            }
        }
    }
}

/// A `finished`, field by field; [`Finished::envelope`] gives the reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Finished {
    outcome: Outcome,
    chunks: u64,
    bytes: u64,
    view: Option<(String, Option<String>)>,
    preview: bool,
    suggest: Option<Vec<String>>,
    next: Option<Vec<String>>,
    help: Option<String>,
    approval: Option<String>,
}

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|word| (*word).to_owned()).collect()
}

impl Finished {
    fn with(outcome: Outcome) -> Finished {
        Finished {
            outcome,
            chunks: 0,
            bytes: 0,
            view: None,
            preview: false,
            suggest: None,
            next: None,
            help: None,
            approval: None,
        }
    }

    /// `status: ok` with a summary.
    pub fn ok(summary: &str) -> Finished {
        Finished::with(Outcome::Ok {
            summary: Some(summary.to_owned()),
        })
    }

    /// `status: ok` without a summary.
    pub fn ok_bare() -> Finished {
        Finished::with(Outcome::Ok { summary: None })
    }

    /// `status: error` with its code, class and message.
    pub fn error(code: &str, class: &str, message: Option<&str>) -> Finished {
        Finished::with(Outcome::Error {
            code: code.to_owned(),
            class: Some(class.to_owned()),
            message: message.map(str::to_owned),
        })
    }

    /// The result came in `chunks` chunks of `bytes` bytes in all.
    pub fn chunks(mut self, chunks: u64, bytes: u64) -> Finished {
        self.chunks = chunks;
        self.bytes = bytes;
        self
    }

    /// The result is `data` in one chunk (send [`chunk`]`(id, 0, 1, data)` first).
    pub fn data(self, data: &str) -> Finished {
        self.chunks(1, data.len() as u64)
    }

    pub fn view(mut self, id: &str, title: Option<&str>) -> Finished {
        self.view = Some((id.to_owned(), title.map(str::to_owned)));
        self
    }

    pub fn preview(mut self) -> Finished {
        self.preview = true;
        self
    }

    pub fn suggest(mut self, argv: &[&str]) -> Finished {
        self.suggest = Some(words(argv));
        self
    }

    pub fn next(mut self, argv: &[&str]) -> Finished {
        self.next = Some(words(argv));
        self
    }

    pub fn help(mut self, command: &str) -> Finished {
        self.help = Some(command.to_owned());
        self
    }

    /// How the change ran: `card` or `none`.
    pub fn approval(mut self, approval: &str) -> Finished {
        self.approval = Some(approval.to_owned());
        self
    }

    pub fn envelope(&self, id: &str) -> Envelope {
        let mut map = head("finished", id);
        let mut outcome = Map::new();
        self.outcome.write(&mut outcome);
        map.insert("outcome".into(), Value::Object(outcome));
        map.insert("chunks".into(), json!(self.chunks));
        map.insert("bytes".into(), json!(self.bytes));
        if let Some((view, title)) = &self.view {
            let mut object = Map::new();
            object.insert("id".into(), json!(view));
            if let Some(title) = title {
                object.insert("title".into(), json!(title));
            }
            map.insert("view".into(), Value::Object(object));
        }
        if self.preview {
            map.insert("preview".into(), json!(true));
        }
        if let Some(suggest) = &self.suggest {
            map.insert("suggest".into(), json!(suggest));
        }
        if let Some(next) = &self.next {
            map.insert("next".into(), json!(next));
        }
        if let Some(help) = &self.help {
            map.insert("help".into(), json!(help));
        }
        if let Some(approval) = &self.approval {
            map.insert("approval".into(), json!(approval));
        }
        checked(Value::Object(map))
    }
}

/// A `help-result`, field by field; [`Help::envelope`] gives the reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Help {
    outcome: Outcome,
    text: Option<Vec<String>>,
    result: Option<Value>,
    suggest: Option<Vec<String>>,
    next: Option<Vec<String>>,
    help: Option<String>,
}

impl Help {
    fn with(outcome: Outcome) -> Help {
        Help {
            outcome,
            text: None,
            result: None,
            suggest: None,
            next: None,
            help: None,
        }
    }

    /// `status: ok` with the lines of `want: text`.
    pub fn text(lines: &[&str]) -> Help {
        Help {
            text: Some(words(lines)),
            ..Help::with(Outcome::Ok { summary: None })
        }
    }

    /// `status: ok` with the object of `want: json`.
    pub fn json(result: Value) -> Help {
        Help {
            result: Some(result),
            ..Help::with(Outcome::Ok { summary: None })
        }
    }

    /// `status: error`; a class or a message may be left out.
    pub fn error(code: &str, class: Option<&str>, message: Option<&str>) -> Help {
        Help::with(Outcome::Error {
            code: code.to_owned(),
            class: class.map(str::to_owned),
            message: message.map(str::to_owned),
        })
    }

    pub fn suggest(mut self, argv: &[&str]) -> Help {
        self.suggest = Some(words(argv));
        self
    }

    pub fn next(mut self, argv: &[&str]) -> Help {
        self.next = Some(words(argv));
        self
    }

    pub fn help(mut self, command: &str) -> Help {
        self.help = Some(command.to_owned());
        self
    }

    pub fn envelope(&self, id: &str) -> Envelope {
        let mut map = head("help-result", id);
        self.outcome.write(&mut map);
        if let Some(text) = &self.text {
            map.insert("text".into(), json!(text));
        }
        if let Some(result) = &self.result {
            map.insert("result".into(), result.clone());
        }
        if let Some(suggest) = &self.suggest {
            map.insert("suggest".into(), json!(suggest));
        }
        if let Some(next) = &self.next {
            map.insert("next".into(), json!(next));
        }
        if let Some(help) = &self.help {
            map.insert("help".into(), json!(help));
        }
        checked(Value::Object(map))
    }
}
