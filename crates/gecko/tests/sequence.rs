//! command-console 1.7.0 `fixtures/remote/sequence/` against the CLI's run: each
//! example feeds replies and events in order and checks the exit code and a subset of
//! the `--json` result. The run is the binary's own module, built here by path.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

#[path = "../src/format.rs"]
mod format;
#[path = "../src/output.rs"]
mod output;
#[path = "../src/run.rs"]
mod run;
mod session {
    pub fn now_ms() -> u64 {
        0
    }
}

use gecko_wire::reply::{Read, read_value};
use run::{Event, Run, Step};
use serde_json::Value;

fn feed(example: &Value) -> output::Outcome {
    let danger_change = example["danger"] != "read";
    let mut run = Run::new(danger_change);
    for item in example["replies"].as_array().unwrap() {
        let event = match item.get("event").and_then(Value::as_str) {
            Some("connection-lost") => Event::Lost("the hub connection was lost".into()),
            Some("timeout") => Event::DeliveryTimeout,
            Some("ctrl-c") => Event::Interrupt,
            Some(other) => panic!("unknown event {other}"),
            None => match read_value(item) {
                Read::Reply(reply) => Event::Reply(reply),
                other => panic!("{}: {other:?}", example["name"]),
            },
        };
        match run.on(event) {
            Step::End(outcome) => return *outcome,
            Step::Wait | Step::Info(_) | Step::Approval(_) | Step::SendCancel => {}
        }
    }
    // Ctrl+C sent a cancel; the wait for its answer ran out.
    match run.on(Event::CancelWaitOver) {
        Step::End(outcome) => *outcome,
        other => panic!("{}: no end ({other:?})", example["name"]),
    }
}

/// Every field of `subset` is in `whole` with the same value.
fn contains(whole: &Value, subset: &Value) -> bool {
    match (whole, subset) {
        (Value::Object(w), Value::Object(s)) => s
            .iter()
            .all(|(k, v)| w.get(k).is_some_and(|wv| contains(wv, v))),
        _ => whole == subset,
    }
}

#[test]
fn sequences() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/command-console-1.7.0/remote/sequence");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let examples: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for example in examples.as_array().unwrap() {
            let label = format!(
                "{}/{}",
                path.file_name().unwrap().to_string_lossy(),
                example["name"].as_str().unwrap()
            );
            let outcome = feed(example);
            let json = serde_json::to_value(&outcome).unwrap();
            let expect = &example["expect"];
            assert_eq!(
                u64::from(outcome.exit()),
                expect["exitCode"].as_u64().unwrap(),
                "{label}: {json}"
            );
            assert!(contains(&json, &expect["json"]), "{label}: {json}");
            for field in expect
                .get("absent")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                assert!(
                    json.get(field.as_str().unwrap()).is_none(),
                    "{label}: {json}"
                );
            }
            count += 1;
        }
    }
    assert_eq!(count, 18);
}
