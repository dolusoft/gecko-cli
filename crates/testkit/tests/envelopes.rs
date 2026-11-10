//! The testkit's reply constructor against command-console 1.8.0 `valid/replies.json`:
//! every example is built with the constructor and must equal the published envelope,
//! field for field. A field the contract adds or renames shows up here.

use gecko_testkit::envelope::{
    self, Envelope, Finished, Help, KINDS, Received, awaiting_approval, cancel_requested,
    cancelled_before_start, chunk, received,
};
use gecko_testkit::fixture;
use serde_json::{Value, json};

const TASK_TEXT: [&str; 8] = [
    "tasks.stop · change",
    "Stops a task.",
    "Usage:",
    "  tasks.stop --id <integer> [options]",
    "",
    "Fields:",
    "  --id <integer> · required",
    "  --yes <boolean> · optional",
];

fn task_text(mode: &'static str) -> Vec<&'static str> {
    let mut lines = TASK_TEXT.to_vec();
    lines[4] = mode;
    lines
}

/// The constructor's envelope for each example of `valid/replies.json`.
fn built(name: &str) -> Envelope {
    let rejected = "The user rejected the change in the browser; nothing changed.";
    match name {
        "received" => envelope::received_read("r1"),
        "received-change-age" => received(
            "r1",
            Received {
                hidden: true,
                data_age_ms: Some(42_000),
                change: true,
                view_relevant: true,
            },
        ),
        "awaiting-approval" => awaiting_approval("r1", 121_000),
        "chunk" => chunk("r1", 0, 1, "{\"a\":1}"),
        "finished-ok" => Finished::ok("3 rows")
            .chunks(1, 7)
            .view("v-1", Some("Tasks"))
            .envelope("r1"),
        "finished-preview-suggest" => Finished::ok("1 task stops")
            .preview()
            .suggest(&["tasks.stop", "--id", "8812", "--yes"])
            .envelope("r1"),
        "finished-next" => Finished::ok_bare()
            .data("{\"a\":1}")
            .next(&["tasks.list", "--cursor", "40"])
            .envelope("r1"),
        "finished-unknown-command-suggest" => Finished::error(
            "unknown-command",
            "usage",
            Some("unknown command: tasks.lst; did you mean: tasks.list"),
        )
        .suggest(&["tasks.list"])
        .envelope("r1"),
        "finished-invalid-args-help" => {
            Finished::error("invalid-args", "usage", Some("missing --id"))
                .help("tasks.stop")
                .envelope("r1")
        }
        "finished-cancelled" => Finished::error("cancelled", "cancelled", None).envelope("r1"),
        "help-result-text" => {
            Help::text(&["tasks.list  Lists the tasks", "tasks.stop  Stops a task"]).envelope("h1")
        }
        "help-result-json-next" => Help::json(json!({ "commands": [] }))
            .next(&["help", "tasks", "--cursor", "20"])
            .envelope("h1"),
        "help-result-error" => Help::error(
            "unknown-command",
            Some("usage"),
            Some("unknown command: tsks"),
        )
        .suggest(&["tasks"])
        .envelope("h1"),
        "cancel-requested" => cancel_requested("r1"),
        "cancelled-before-start" => cancelled_before_start("r1"),
        "help-result-command-text" => Help::text(&task_text(
            "Runs as a preview without --yes; with --yes it makes the change.",
        ))
        .envelope("h2"),
        "help-result-invalid-args-help" => Help::error(
            "invalid-args",
            Some("usage"),
            Some("limit: expected integer from 1 to 100"),
        )
        .help("tasks.list")
        .envelope("h3"),
        "help-result-sensitive-name" => {
            Help::error("unknown-command", Some("usage"), None).envelope("h4")
        }
        "finished-unknown-command-name-only" => Finished::error(
            "unknown-command",
            "usage",
            Some("unknown command: tasks.lst"),
        )
        .suggest(&["tasks.list"])
        .envelope("r1"),
        "finished-not-allowed-user-rejected" => {
            Finished::error("not-allowed", "denied", Some(rejected)).envelope("r1")
        }
        "finished-approval-card" => Finished::ok("Task 8812 stopped")
            .approval("card")
            .envelope("r1"),
        "finished-approval-none" => Finished::ok("Dashboard opened")
            .approval("none")
            .envelope("r1"),
        "help-result-command-text-dry-run" => Help::text(&task_text(
            "Makes the change; with --dry-run it shows the preview.",
        ))
        .envelope("h2"),
        other => panic!("valid/replies.json has {other}; the testkit cannot build it yet"),
    }
}

#[test]
fn every_published_reply_is_built_the_same() {
    let examples = fixture("1.8.0", "valid/replies.json");
    let examples = examples.as_array().expect("a list");
    let mut kinds: Vec<String> = Vec::new();
    for example in examples {
        let name = example["name"].as_str().expect("a name");
        let published = &example["envelope"];
        let ours = built(name);
        assert_eq!(ours.value(), published, "{name}");
        kinds.push(ours.kind().to_owned());
    }
    // Every kind the constructor offers is published, and every published one is built.
    let mut kinds_seen: Vec<&str> = kinds.iter().map(String::as_str).collect();
    kinds_seen.sort_unstable();
    kinds_seen.dedup();
    let mut offered = KINDS.to_vec();
    offered.sort_unstable();
    assert_eq!(kinds_seen, offered);
}

#[test]
fn no_progress_kind() {
    assert!(!KINDS.contains(&"progress"));
}

#[test]
#[should_panic(expected = "the CLI reads as")]
fn a_reply_the_cli_would_drop_fails_where_it_is_made() {
    // A chunk `total` over the contract's limit (3): gecko-wire refuses the chunk.
    let _ = chunk("r1", 0, 4, "x");
}

#[test]
fn the_capability_list_is_the_published_one() {
    let published = gecko_testkit::capabilities();
    let parts = published["parts"].as_array().expect("parts");
    let list: Value =
        serde_json::from_str(parts[0].as_str().expect("a part")).expect("a part is JSON");
    let names: Vec<&str> = list["commands"]
        .as_array()
        .expect("commands")
        .iter()
        .map(|command| command["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(names, ["search", "nav.go", "tasks.stop"]);
    assert_eq!(published["revision"], list["revision"]);
}
