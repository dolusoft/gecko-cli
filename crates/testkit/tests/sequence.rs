//! command-console `fixtures/remote/sequence/` (1.7.2 and 1.8.2) against the CLI's run: each
//! example feeds replies and events in order and checks the exit code and a subset of
//! the `--json` result. The run is the command domain's state machine; its clock is a
//! parameter (`Run::on(event, now)`). The test lives in the testkit, not in the domain:
//! reading the files is I/O, and the domain has none.

use gecko_command_domain::run::{Event, Run, Step};
use gecko_wire::{Read, read_value};
use serde_json::Value;

/// The fields of an example's `tab` this runner knows (1.8.2 `remote/README.md`); a
/// new one fails here instead of being passed over.
const TAB_FIELDS: [&str; 5] = ["level", "command", "summary", "approve", "expire"];

fn feed(example: &Value) -> gecko_kernel::Outcome {
    // The run's clock, which an example with `tab.expire` moves past the card.
    let mut now = 0;
    if let Some(tab) = example.get("tab").and_then(Value::as_object) {
        for key in tab.keys() {
            assert!(
                TAB_FIELDS.contains(&key.as_str()),
                "{}: tab.{key} is new",
                example["name"]
            );
        }
    }
    // `tab.expire`: the card's time runs out before the tab's last word.
    let expire = example["tab"]["expire"] == true;
    let danger_change = example["danger"] != "read";
    let mut run = Run::new(danger_change);
    for item in example["replies"].as_array().expect("feed") {
        if expire && item["kind"] == "finished" {
            // The card is over before the tab's word; the run still waits for that
            // word, it does not end on its own clock.
            let end = run
                .approval_expires_at()
                .unwrap_or_else(|| panic!("{}: expire without a card", example["name"]));
            now = end as u64 + 1;
        }
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
        match run.on(event, now) {
            Step::End(outcome) => return *outcome,
            Step::Wait | Step::Info(_) | Step::Approval(_) | Step::SendCancel => {}
        }
    }
    // Ctrl+C sent a cancel; the wait for its answer ran out.
    match run.on(Event::CancelWaitOver, now) {
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

/// Runs every example of one fixture folder; returns how many ran.
fn sequences_of(version: &str) -> usize {
    let mut count = 0;
    for (file, examples) in gecko_testkit::sequences(version) {
        for example in examples.as_array().expect("sequences_of") {
            let label = format!(
                "{version} {file}/{}",
                example["name"].as_str().expect("sequences_of")
            );
            let outcome = feed(example);
            let json = serde_json::to_value(&outcome).expect("sequences_of");
            let expect = &example["expect"];
            assert_eq!(
                u64::from(outcome.exit()),
                expect["exitCode"].as_u64().expect("sequences_of"),
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
                    json.get(field.as_str().expect("sequences_of")).is_none(),
                    "{label}: {json}"
                );
            }
            // 1.7.2: an error thrown while planning a change comes with its text, and
            // `--json` carries it.
            if example["name"] == "change-plan-error" {
                assert_eq!(
                    json["message"], "/system/usersettings is not a page of your menu.",
                    "{label}: {json}"
                );
            }
            count += 1;
        }
    }
    count
}

#[test]
fn sequences() {
    assert_eq!(sequences_of("1.7.2"), 19);
    // 1.8.0 adds `approval.json`: the `approval` field, `--dry-run` and `--yes`.
    // 1.8.2 adds the expired card: `change-server-card-expired` and
    // `change-server-card-approved-late` (`tab.expire`).
    assert_eq!(sequences_of("1.8.2"), 28);
}
