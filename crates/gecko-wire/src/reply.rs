//! Replies from the tab, as the CLI reads them (command-console 1.7.0,
//! docs/remote-contract.md "From the tab"). A reply is checked field by field; an
//! unknown field is ignored, an unknown `kind` is dropped, and enums added later fall
//! back to their documented defaults.

use serde_json::{Map, Value};

use crate::class::ErrorClass;

/// The largest envelope in UTF-8 bytes (`envelopeMaxBytes`).
pub const ENVELOPE_MAX_BYTES: usize = 16_384;
/// The most chunks of one result (`remoteChunksMax`).
pub const CHUNKS_MAX: u64 = 3;

#[derive(Debug, Clone, PartialEq)]
pub struct Received {
    pub id: String,
    pub hidden: bool,
    pub data_age_ms: Option<f64>,
    pub change: bool,
    pub view_relevant: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub id: String,
    pub index: u64,
    pub total: u64,
    pub bytes: u64,
    pub data: String,
}

/// `outcome.status`; an unknown value reads as `Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Error,
}

/// What a `finished` and a `help-result` share on an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: Option<String>,
    /// `None` when missing: exit code 1, as an unknown class.
    pub class: Option<ErrorClass>,
    pub message: Option<String>,
}

impl Failure {
    pub fn exit_code(&self) -> u8 {
        self.class.map_or(1, ErrorClass::exit_code)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub id: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub id: String,
    pub status: Status,
    pub summary: Option<String>,
    pub failure: Failure,
    pub chunks: u64,
    pub bytes: u64,
    pub view: Option<View>,
    pub preview: bool,
    pub suggest: Option<Vec<String>>,
    pub next: Option<Vec<String>>,
    pub help: Option<String>,
}

impl Finished {
    /// 0 for `ok`; otherwise the class's code (§5.6).
    pub fn exit_code(&self) -> u8 {
        match self.status {
            Status::Ok => 0,
            Status::Error => self.failure.exit_code(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HelpResult {
    pub id: String,
    pub status: Status,
    pub failure: Failure,
    pub text: Option<Vec<String>>,
    pub result: Option<Value>,
    pub suggest: Option<Vec<String>>,
    pub next: Option<Vec<String>>,
    pub help: Option<String>,
}

impl HelpResult {
    pub fn exit_code(&self) -> u8 {
        match self.status {
            Status::Ok => 0,
            Status::Error => self.failure.exit_code(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Received(Received),
    AwaitingApproval { id: String, expires_at: f64 },
    Chunk(Chunk),
    Finished(Box<Finished>),
    HelpResult(Box<HelpResult>),
    CancelRequested { id: String },
    CancelledBeforeStart { id: String },
}

impl Reply {
    pub fn id(&self) -> &str {
        match self {
            Reply::Received(r) => &r.id,
            Reply::Chunk(c) => &c.id,
            Reply::Finished(f) => &f.id,
            Reply::HelpResult(h) => &h.id,
            Reply::AwaitingApproval { id, .. }
            | Reply::CancelRequested { id }
            | Reply::CancelledBeforeStart { id } => id,
        }
    }
}

/// What reading a reply gives.
#[derive(Debug, Clone, PartialEq)]
pub enum Read {
    Reply(Reply),
    /// A `kind` this CLI does not know: dropped (rule 4).
    Ignored,
    /// Not a reply this CLI can read; carries why, for logs.
    Invalid(String),
}

type Fields = Map<String, Value>;

fn invalid(why: impl Into<String>) -> Read {
    Read::Invalid(why.into())
}

fn text(fields: &Fields, key: &str) -> Result<String, String> {
    match fields.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => Err(format!("{key}: expected a string")),
    }
}

fn optional_text(fields: &Fields, key: &str) -> Result<Option<String>, String> {
    match fields.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{key}: expected a string")),
    }
}

fn flag(fields: &Fields, key: &str) -> Result<bool, String> {
    match fields.get(key) {
        Some(Value::Bool(b)) => Ok(*b),
        _ => Err(format!("{key}: expected a boolean")),
    }
}

/// A finite number ≥ 0.
fn amount(value: Option<&Value>, key: &str) -> Result<f64, String> {
    match value.and_then(Value::as_f64) {
        Some(n) if n.is_finite() && n >= 0.0 => Ok(n),
        _ => Err(format!("{key}: expected a number >= 0")),
    }
}

/// An integer ≥ 0.
fn count(fields: &Fields, key: &str) -> Result<u64, String> {
    fields
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{key}: expected an integer >= 0"))
}

/// An argv: one or more strings.
fn argv(fields: &Fields, key: &str) -> Result<Option<Vec<String>>, String> {
    let Some(value) = fields.get(key) else {
        return Ok(None);
    };
    let items = value
        .as_array()
        .filter(|items| !items.is_empty())
        .ok_or_else(|| format!("{key}: expected an argv"))?;
    items
        .iter()
        .map(|item| item.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
        .map(Some)
        .ok_or_else(|| format!("{key}: expected an argv"))
}

fn help_name(fields: &Fields) -> Result<Option<String>, String> {
    match optional_text(fields, "help")? {
        Some(name) if name.is_empty() => Err("help: empty".to_owned()),
        other => Ok(other),
    }
}

fn status(value: Option<&Value>) -> Result<Status, String> {
    match value {
        Some(Value::String(s)) if s == "ok" => Ok(Status::Ok),
        // Unknown: read as an error (rule 4).
        Some(Value::String(_)) => Ok(Status::Error),
        _ => Err("status: expected a string".to_owned()),
    }
}

fn failure(fields: &Fields) -> Result<Failure, String> {
    let class = match fields.get("class") {
        None => None,
        Some(Value::String(s)) => Some(
            serde_json::from_value::<ErrorClass>(Value::String(s.clone()))
                .unwrap_or(ErrorClass::Unrecognized),
        ),
        Some(_) => return Err("class: expected a string".to_owned()),
    };
    Ok(Failure {
        code: optional_text(fields, "code")?,
        class,
        message: optional_text(fields, "message")?,
    })
}

/// Reads one reply from its JSON text.
pub fn read_reply(json: &str) -> Read {
    if json.len() > ENVELOPE_MAX_BYTES {
        return invalid("over 16384 bytes");
    }
    match serde_json::from_str::<Value>(json) {
        Ok(Value::Object(fields)) => read_fields(&fields),
        Ok(_) => invalid("not an object"),
        Err(error) => invalid(format!("not JSON: {error}")),
    }
}

/// Reads one reply already parsed (an event payload).
pub fn read_value(value: &Value) -> Read {
    match value {
        Value::Object(fields) => {
            match serde_json::to_string(value) {
                Ok(text) if text.len() > ENVELOPE_MAX_BYTES => return invalid("over 16384 bytes"),
                _ => {}
            }
            read_fields(fields)
        }
        _ => invalid("not an object"),
    }
}

fn read_fields(fields: &Fields) -> Read {
    // `as_u64` is `None` for a string, a float or a negative number.
    if fields.get("v").and_then(Value::as_u64) != Some(1) {
        return invalid("v: expected 1");
    }
    let Some(kind) = fields.get("kind").and_then(Value::as_str) else {
        return invalid("kind: expected a string");
    };
    let id = match text(fields, "id") {
        Ok(id) if !id.is_empty() => id,
        _ => return invalid("id: expected a non-empty string"),
    };
    let reply = match kind {
        "received" => received(fields, id),
        "awaiting-approval" => amount(fields.get("expiresAt"), "expiresAt")
            .map(|expires_at| Reply::AwaitingApproval { id, expires_at }),
        "chunk" => chunk(fields, id),
        "finished" => finished(fields, id),
        "help-result" => help_result(fields, id),
        "cancel-requested" => Ok(Reply::CancelRequested { id }),
        "cancelled-before-start" => Ok(Reply::CancelledBeforeStart { id }),
        _ => return Read::Ignored,
    };
    match reply {
        Ok(reply) => Read::Reply(reply),
        Err(why) => Read::Invalid(format!("{kind}: {why}")),
    }
}

fn received(fields: &Fields, id: String) -> Result<Reply, String> {
    let data_age_ms = match fields.get("dataAgeMs") {
        Some(Value::Null) => None,
        other => Some(amount(other, "dataAgeMs")?),
    };
    Ok(Reply::Received(Received {
        id,
        hidden: flag(fields, "hidden")?,
        data_age_ms,
        change: flag(fields, "change")?,
        view_relevant: flag(fields, "viewRelevant")?,
    }))
}

fn chunk(fields: &Fields, id: String) -> Result<Reply, String> {
    let total = count(fields, "total")?;
    if total == 0 || total > CHUNKS_MAX {
        return Err("total: expected 1 to 3".to_owned());
    }
    Ok(Reply::Chunk(Chunk {
        id,
        index: count(fields, "index")?,
        total,
        bytes: count(fields, "bytes")?,
        data: text(fields, "data")?,
    }))
}

fn finished(fields: &Fields, id: String) -> Result<Reply, String> {
    let Some(Value::Object(outcome)) = fields.get("outcome") else {
        return Err("outcome: expected an object".to_owned());
    };
    let view = match fields.get("view") {
        None => None,
        Some(Value::Object(view)) => Some(View {
            id: text(view, "id")?,
            title: optional_text(view, "title")?,
        }),
        Some(_) => return Err("view: expected an object".to_owned()),
    };
    let preview = match fields.get("preview") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("preview: expected a boolean".to_owned()),
    };
    Ok(Reply::Finished(Box::new(Finished {
        id,
        status: status(outcome.get("status"))?,
        summary: optional_text(outcome, "summary")?,
        failure: failure(outcome)?,
        chunks: count(fields, "chunks")?,
        bytes: count(fields, "bytes")?,
        view,
        preview,
        suggest: argv(fields, "suggest")?,
        next: argv(fields, "next")?,
        help: help_name(fields)?,
    })))
}

fn help_result(fields: &Fields, id: String) -> Result<Reply, String> {
    let text_lines = match fields.get("text") {
        None => None,
        Some(Value::Array(items)) => Some(
            items
                .iter()
                .map(|item| item.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or("text: expected strings")?,
        ),
        Some(_) => return Err("text: expected strings".to_owned()),
    };
    let result = fields.get("result").cloned();
    if text_lines.is_some() && result.is_some() {
        return Err("text and result together".to_owned());
    }
    Ok(Reply::HelpResult(Box::new(HelpResult {
        id,
        status: status(fields.get("status"))?,
        failure: failure(fields)?,
        text: text_lines,
        result,
        suggest: argv(fields, "suggest")?,
        next: argv(fields, "next")?,
        help: help_name(fields)?,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_must_be_the_integer_one() {
        let base = r#""kind":"cancel-requested","id":"r1""#;
        assert!(matches!(
            read_reply(&format!("{{\"v\":1,{base}}}")),
            Read::Reply(Reply::CancelRequested { .. })
        ));
        for v in ["2", "\"1\"", "1.5"] {
            assert!(
                matches!(
                    read_reply(&format!("{{\"v\":{v},{base}}}")),
                    Read::Invalid(_)
                ),
                "{v}"
            );
        }
    }

    #[test]
    fn chunk_total_is_bounded() {
        let chunk = |total: u64| {
            read_reply(&format!(
                r#"{{"v":1,"kind":"chunk","id":"r","index":0,"total":{total},"bytes":1,"data":"x"}}"#
            ))
        };
        assert!(matches!(chunk(3), Read::Reply(_)));
        assert!(matches!(chunk(4), Read::Invalid(_)));
        assert!(matches!(chunk(0), Read::Invalid(_)));
    }
}
