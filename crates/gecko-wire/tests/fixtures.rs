//! The examples published with command-console (`fixtures/command-console-<version>/`)
//! against the CLI's side of the contract: replies and capabilities it reads, the
//! command name rule and the limits it checks before sending. Every published version
//! in the repository runs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use gecko_wire::capabilities::{Capabilities, Effect, read_parts};
use gecko_wire::class::{Danger, ErrorClass};
use gecko_wire::reply::{Read, Reply, Status, read_reply};
use gecko_wire::request::{Input, check_limits, command_name};
use serde_json::{Map, Value, json};

/// The fixture folders, oldest first.
const VERSIONS: [&str; 2] = ["1.7.2", "1.8.2"];

fn load(version: &str, file: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../fixtures/command-console-{version}/remote"))
        .join(file);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

/// The examples of `file` in every version, each with its label (`1.8.2 name`).
fn examples(file: &str) -> Vec<(String, Value)> {
    VERSIONS
        .iter()
        .flat_map(|version| {
            load(version, file)
                .as_array()
                .unwrap()
                .iter()
                .map(|example| (format!("{version} {}", name(example)), example.clone()))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn name(example: &Value) -> &str {
    example["name"].as_str().unwrap()
}

fn number(n: f64) -> Value {
    if n.fract() == 0.0 && (0.0..9.0e15).contains(&n) {
        json!(n as u64)
    } else {
        json!(n)
    }
}

fn argv_field(map: &mut Map<String, Value>, key: &str, value: Option<&Vec<String>>) {
    if let Some(value) = value {
        map.insert(key.into(), json!(value));
    }
}

/// A reply as the contract's `read` writes it, so nothing the CLI read is lost.
fn as_read(reply: &Reply) -> Value {
    let mut map = Map::new();
    map.insert("v".into(), json!(1));
    let kind = match reply {
        Reply::Received(_) => "received",
        Reply::AwaitingApproval { .. } => "awaiting-approval",
        Reply::Chunk(_) => "chunk",
        Reply::Finished(_) => "finished",
        Reply::HelpResult(_) => "help-result",
        Reply::CancelRequested { .. } => "cancel-requested",
        Reply::CancelledBeforeStart { .. } => "cancelled-before-start",
    };
    map.insert("kind".into(), json!(kind));
    map.insert("id".into(), json!(reply.id()));
    let status = |s: Status| json!(if s == Status::Ok { "ok" } else { "error" });
    let class = |c: ErrorClass| serde_json::to_value(c).unwrap();
    match reply {
        Reply::Received(r) => {
            map.insert("hidden".into(), json!(r.hidden));
            map.insert(
                "dataAgeMs".into(),
                r.data_age_ms.map_or(Value::Null, number),
            );
            map.insert("change".into(), json!(r.change));
            map.insert("viewRelevant".into(), json!(r.view_relevant));
        }
        Reply::AwaitingApproval { expires_at, .. } => {
            map.insert("expiresAt".into(), number(*expires_at));
        }
        Reply::Chunk(c) => {
            map.insert("index".into(), json!(c.index));
            map.insert("total".into(), json!(c.total));
            map.insert("bytes".into(), json!(c.bytes));
            map.insert("data".into(), json!(c.data));
        }
        Reply::Finished(f) => {
            let mut outcome = Map::new();
            outcome.insert("status".into(), status(f.status));
            if let Some(summary) = &f.summary {
                outcome.insert("summary".into(), json!(summary));
            }
            if let Some(code) = &f.failure.code {
                outcome.insert("code".into(), json!(code));
            }
            if let Some(c) = f.failure.class {
                outcome.insert("class".into(), class(c));
            }
            if let Some(message) = &f.failure.message {
                outcome.insert("message".into(), json!(message));
            }
            map.insert("outcome".into(), Value::Object(outcome));
            map.insert("chunks".into(), json!(f.chunks));
            map.insert("bytes".into(), json!(f.bytes));
            if let Some(view) = &f.view {
                let mut v = json!({ "id": view.id });
                if let Some(title) = &view.title {
                    v["title"] = json!(title);
                }
                map.insert("view".into(), v);
            }
            if f.preview {
                map.insert("preview".into(), json!(true));
            }
            argv_field(&mut map, "suggest", f.suggest.as_ref());
            argv_field(&mut map, "next", f.next.as_ref());
            if let Some(help) = &f.help {
                map.insert("help".into(), json!(help));
            }
            if let Some(approval) = &f.approval {
                map.insert("approval".into(), json!(approval));
            }
        }
        Reply::HelpResult(h) => {
            map.insert("status".into(), status(h.status));
            if let Some(code) = &h.failure.code {
                map.insert("code".into(), json!(code));
            }
            if let Some(c) = h.failure.class {
                map.insert("class".into(), class(c));
            }
            if let Some(message) = &h.failure.message {
                map.insert("message".into(), json!(message));
            }
            if let Some(text) = &h.text {
                map.insert("text".into(), json!(text));
            }
            if let Some(result) = &h.result {
                map.insert("result".into(), result.clone());
            }
            argv_field(&mut map, "suggest", h.suggest.as_ref());
            argv_field(&mut map, "next", h.next.as_ref());
            if let Some(help) = &h.help {
                map.insert("help".into(), json!(help));
            }
        }
        Reply::CancelRequested { .. } | Reply::CancelledBeforeStart { .. } => {}
    }
    Value::Object(map)
}

fn reply_of(example: &Value) -> Read {
    read_reply(&example["envelope"].to_string())
}

fn parts_of(example: &Value) -> Vec<String> {
    example["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_owned())
        .collect()
}

fn caps_json(caps: &Capabilities) -> Value {
    serde_json::to_value(caps).unwrap()
}

#[test]
fn valid_replies_are_read_whole() {
    for (label, example) in examples("valid/replies.json") {
        match reply_of(&example) {
            Read::Reply(reply) => assert_eq!(as_read(&reply), example["read"], "{label}"),
            other => panic!("{label}: {other:?}"),
        }
    }
}

#[test]
fn invalid_replies_are_refused() {
    for (label, example) in examples("invalid/replies.json") {
        assert!(matches!(reply_of(&example), Read::Invalid(_)), "{label}");
    }
}

#[test]
fn valid_capabilities_are_read_whole() {
    for (label, example) in examples("valid/capabilities.json") {
        let caps = read_parts(&parts_of(&example)).unwrap();
        assert_eq!(caps_json(&caps), example["read"], "{label}");
        assert!(caps.missing_features().is_empty(), "{label}");
        assert_eq!(caps.envelope(), Some(1));
    }
}

#[test]
fn invalid_capabilities_are_refused() {
    for (label, example) in examples("invalid/capabilities.json") {
        assert!(read_parts(&parts_of(&example)).is_err(), "{label}");
    }
}

#[test]
fn future_messages_from_the_tab() {
    for (label, example) in examples("future/tab-to-cli.json") {
        let expect = &example["expect"];
        if example.get("parts").is_some() {
            let caps = read_parts(&parts_of(&example)).unwrap();
            if let Some(read) = example.get("read") {
                assert_eq!(caps_json(&caps), *read, "{label}");
            }
            if let Some(defaults) = expect.get("default") {
                for (path, value) in defaults.as_object().unwrap() {
                    match path.as_str() {
                        "commands.0.danger" => {
                            assert_eq!(value, "change", "{label}");
                            assert_eq!(caps.commands[0].danger, Danger::Change, "{label}");
                        }
                        "commands.0.effect" => {
                            assert_eq!(value, "server", "{label}");
                            assert_eq!(caps.commands[0].effect(), Effect::Server, "{label}");
                            assert_eq!(caps_json(&caps)["commands"][0]["effect"], *value);
                        }
                        other => panic!("{label}: unknown path {other}"),
                    }
                }
            }
            continue;
        }
        let read = reply_of(&example);
        match expect {
            Value::String(s) if s == "ignore" => assert_eq!(read, Read::Ignored, "{label}"),
            Value::String(s) if s == "accept-ignore" => {
                assert!(matches!(read, Read::Reply(_)), "{label}");
            }
            Value::Object(object) => {
                let Read::Reply(Reply::Finished(finished)) = read else {
                    panic!("{label}: {read:?}");
                };
                for (path, value) in object["default"].as_object().unwrap() {
                    match path.as_str() {
                        "outcome.status" => {
                            assert_eq!(value, "error", "{label}");
                            assert_eq!(finished.status, Status::Error, "{label}");
                        }
                        "exitCode" => assert_eq!(
                            u64::from(finished.exit_code()),
                            value.as_u64().unwrap(),
                            "{label}"
                        ),
                        "approval" => {
                            assert_eq!(finished.approval.as_deref(), value.as_str(), "{label}")
                        }
                        other => panic!("{label}: unknown path {other}"),
                    }
                }
            }
            other => panic!("{label}: {other}"),
        }
    }
}

#[test]
fn every_code_has_a_known_class() {
    let table = [
        ("failed", 1),
        ("usage", 2),
        ("denied", 3),
        ("not-run", 4),
        ("unknown", 5),
        ("cancelled", 1),
    ];
    for version in VERSIONS {
        for row in load(version, "error-class.json").as_array().unwrap() {
            let class: ErrorClass = serde_json::from_value(row["class"].clone()).unwrap();
            assert_ne!(class, ErrorClass::Unrecognized, "{version} {row}");
            let expected = table
                .iter()
                .find(|(c, _)| *c == row["class"])
                .map(|(_, code)| *code)
                .unwrap();
            assert_eq!(class.exit_code(), expected, "{version} {row}");
        }
    }
}

#[test]
fn command_names_on_the_cli_side() {
    for version in VERSIONS {
        let fixture = load(version, "command-name.json");
        let mut sensitive = Vec::new();
        let mut commands = Vec::new();
        for command in fixture["commands"].as_array().unwrap() {
            let mut names = vec![command["name"].clone()];
            names.extend(command["aliases"].as_array().unwrap().iter().cloned());
            if command["sensitive"] == true {
                sensitive.extend(names);
            } else {
                commands.push(command.clone());
            }
        }
        let parts = vec![
            json!({
                "envelopes": [1], "features": [], "revision": "r",
                "sensitive": sensitive, "commands": commands
            })
            .to_string(),
        ];
        let caps = read_parts(&parts).unwrap();
        for row in fixture["rows"].as_array().unwrap() {
            let label = format!("{version} {}", name(row));
            let input = if let Some(argv) = row["input"].get("argv") {
                Input::Argv(serde_json::from_value(argv.clone()).unwrap())
            } else {
                Input::Line(row["input"]["line"].as_str().unwrap().to_owned())
            };
            let found = command_name(&input);
            match (&row["cli"]["name"], &row["cli"]["refuse"]) {
                (Value::String(expected), _) => {
                    assert_eq!(found, Some(expected.as_str()), "{label}");
                    assert!(!caps.is_sensitive(expected), "{label}");
                }
                (_, Value::String(why)) if why == "name" => assert_eq!(found, None, "{label}"),
                (_, Value::String(why)) if why == "sensitive" => {
                    let found = found.unwrap_or_else(|| panic!("{label}: no name"));
                    assert!(caps.is_sensitive(found), "{label}");
                }
                other => panic!("{label}: {other:?}"),
            }
        }
    }
}

#[test]
fn request_limits_match_the_tab() {
    let input_of = |envelope: &Value| {
        if let Some(argv) = envelope.get("argv").and_then(Value::as_array) {
            Some(Input::Argv(
                argv.iter()
                    .map(|a| a.as_str().map(str::to_owned))
                    .collect::<Option<_>>()?,
            ))
        } else {
            envelope
                .get("line")
                .and_then(Value::as_str)
                .map(|l| Input::Line(l.to_owned()))
        }
    };
    for (label, example) in examples("valid/cli-request.json") {
        let input = input_of(&example["envelope"]).unwrap();
        assert!(check_limits(&input).is_ok(), "{label}");
    }
    for version in VERSIONS {
        let invalid = load(version, "invalid/cli-request.json");
        let find = |label: &str| {
            invalid
                .as_array()
                .unwrap()
                .iter()
                .find(|e| name(e) == label)
                .unwrap()
                .clone()
        };
        for label in [
            "argv-empty",
            "argv-257-items",
            "argv-item-4001-code-points",
            "argv-total-4001-code-points",
            "line-4001-code-points",
        ] {
            let input = input_of(&find(label)["envelope"]).unwrap();
            assert!(check_limits(&input).is_err(), "{version} {label}");
        }
        let empty_name = find("argv-empty-name");
        assert_eq!(
            command_name(&input_of(&empty_name["envelope"]).unwrap()),
            None
        );
    }
}
