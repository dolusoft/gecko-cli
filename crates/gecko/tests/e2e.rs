//! End to end: the real binary against the real development hub (gecko-testkit `Hub`)
//! and a fake tab that answers with command-console 1.8.0 replies. Black box: argv,
//! stdin and environment in; exit code, stdout and stderr out. Both feature sets run
//! these: the hub comes from the testkit, not from the binary.

use gecko_testkit::envelope::{
    Finished, Received, awaiting_approval, cancel_requested, cancelled_before_start, chunk,
    received, received_change, received_read, replay,
};
use std::time::{Duration, Instant};

use gecko_testkit::{
    Cli, Hub, Output, Process, Request, Tab, WAIT, capabilities_with, connect, disconnect, fixture,
    now_ms,
};
use serde_json::{Value, json};

const GECKO: &str = env!("CARGO_BIN_EXE_gecko");

/// The wait line without a terminal, for a card that is open ten minutes more.
const WAIT_LINE: &str =
    "Waiting for approval (~10 min): approve it in the Gecko tab in the browser.";

/// The card's end, ten minutes from now on the tab's clock: "~10 min" holds for any
/// delay under 30 s.
fn ten_minutes() -> u64 {
    now_ms() + 600_000
}

fn cli(json: bool) -> Cli {
    let cli = Cli::new(GECKO);
    if json { cli.json() } else { cli }
}

// --- Row 10: the whole flow, each step in plain text and in JSON ---------------------

async fn whole_flow(json: bool) {
    let hub = Hub::start();
    let mut tab = Tab::open(&hub, "p1").await;
    let cli = cli(json);

    // connect
    let attached = connect(&hub, &cli, &mut tab, "ask-server").await;
    if json {
        let result = attached.json();
        assert_eq!(result["status"], "ok", "{attached}");
        assert_eq!(result["data"]["level"], "ask-server", "{attached}");
        assert_eq!(result["data"]["hub"], hub.url.as_str(), "{attached}");
    } else {
        assert_eq!(
            attached.stdout,
            "Attached: zahid · level ask-server (changes on the server ask for approval in the tab)\n",
            "{attached}"
        );
    }
    let binding = cli.token().expect("a token")["bindingId"].clone();

    // A read: the request reaches the tab stamped, the rows come back in a chunk.
    let run = cli.call(["search", "--ip", "1.1.1.1"]).spawn();
    let request = tab.request().await;
    assert_eq!(request.kind, "cli-request");
    assert_eq!(
        request.argv.as_deref(),
        Some(&["search".to_owned(), "--ip".to_owned(), "1.1.1.1".to_owned()][..])
    );
    assert_eq!(request.message["origin"]["kind"], "cli");
    assert_eq!(request.message["origin"]["bindingId"], binding);
    let rows = r#"[{"ip":"1.1.1.1","hits":3}]"#;
    tab.reply(&received_read(&request.id)).await;
    tab.reply(&chunk(&request.id, 0, 1, rows)).await;
    tab.reply(&Finished::ok("1 row").data(rows).envelope(&request.id))
        .await;
    let read = run.output().await;
    read.expect_code(0);
    if json {
        let result = read.json();
        assert_eq!(result["status"], "ok", "{read}");
        assert_eq!(result["summary"], "1 row", "{read}");
        assert_eq!(
            result["data"],
            json!([{ "ip": "1.1.1.1", "hits": 3 }]),
            "{read}"
        );
        assert_eq!(result["hidden"], false, "{read}");
        assert_eq!(result["dataAgeMs"], Value::Null, "{read}");
        assert_eq!(result["complete"], true, "{read}");
        assert!(result.get("approval").is_none(), "{read}");
        assert_eq!(result["untrusted"], json!(["summary", "data"]), "{read}");
    } else {
        assert_eq!(
            read.stdout, "1 row\nip       hits\n-------  ----\n1.1.1.1  3\n",
            "{read}"
        );
        assert_eq!(read.stderr, "", "{read}");
    }

    // A change on the server: the card opens, the user approves it.
    let mut run = cli.call(["tasks.stop", "--id", "8812"]).spawn();
    let request = tab.request().await;
    tab.reply(&received_change(&request.id)).await;
    tab.reply(&awaiting_approval(&request.id, ten_minutes()))
        .await;
    run.stderr_until(WAIT_LINE).await;
    tab.reply(
        &Finished::ok("Task 8812 stopped")
            .approval("card")
            .envelope(&request.id),
    )
    .await;
    let changed = run.output().await;
    changed.expect_code(0);
    assert_eq!(changed.stderr, format!("{WAIT_LINE}\n"), "{changed}");
    if json {
        let result = changed.json();
        assert_eq!(result["status"], "ok", "{changed}");
        assert_eq!(result["summary"], "Task 8812 stopped", "{changed}");
        assert_eq!(result["approval"], "card", "{changed}");
    } else {
        assert_eq!(changed.stdout, "Task 8812 stopped\n", "{changed}");
    }

    // The status names the user, the tab's place, the level and the hub.
    let status = cli.run(["status"]).await;
    status.expect_code(0);
    if json {
        let data = &status.json()["data"];
        assert_eq!(data["user"]["name"], "zahid", "{status}");
        assert_eq!(data["binding"]["state"], "active", "{status}");
        assert_eq!(data["binding"]["level"], "ask-server", "{status}");
        assert_eq!(data["binding"]["tab"], "ok", "{status}");
        assert_eq!(data["binding"]["title"], "Tasks", "{status}");
        assert_eq!(data["binding"]["path"], "/tasks", "{status}");
        assert_eq!(data["hub"]["url"], hub.url.as_str(), "{status}");
        assert_eq!(data["hub"]["reachable"], true, "{status}");
    } else {
        for line in [
            "user    zahid\n".to_owned(),
            "tab     bound (attached just now) · Tasks · /tasks\n".to_owned(),
            "level   ask-server".to_owned(),
            format!("hub     {} · reachable", hub.url),
        ] {
            assert!(status.stdout.contains(&line), "{line:?}\n{status}");
        }
    }

    // disconnect
    let detached = disconnect(&hub, &cli, &mut tab).await;
    if json {
        let result = detached.json();
        assert_eq!(
            (result["status"].as_str(), result["summary"].as_str()),
            (Some("ok"), Some("Detached")),
            "{detached}"
        );
    } else {
        assert_eq!(detached.stdout, "Detached\n", "{detached}");
    }

    tab.close().await;
    hub.stop();
}

#[tokio::test]
async fn the_whole_flow_in_plain_text() {
    whole_flow(false).await;
}

#[tokio::test]
async fn the_whole_flow_in_json() {
    whole_flow(true).await;
}

// --- Row 12: the approval card, without a terminal -----------------------------------

/// A bound CLI whose `tasks.stop` waits on an open card.
struct Card {
    hub: Hub,
    tab: Tab,
    cli: Cli,
    run: Process,
    request: Request,
}

async fn open_card(json: bool) -> Card {
    let hub = Hub::start();
    let mut tab = Tab::open(&hub, "p1").await;
    let cli = cli(json);
    connect(&hub, &cli, &mut tab, "ask-server").await;
    let mut run = cli.call(["tasks.stop", "--id", "8812"]).spawn();
    let request = tab.request().await;
    tab.reply(&received_change(&request.id)).await;
    tab.reply(&awaiting_approval(&request.id, ten_minutes()))
        .await;
    run.stderr_until(WAIT_LINE).await;
    Card {
        hub,
        tab,
        cli,
        run,
        request,
    }
}

#[tokio::test]
async fn the_wait_line_comes_once_after_awaiting_approval() {
    let hub = Hub::start();
    let mut tab = Tab::open(&hub, "p1").await;
    let cli = Cli::new(GECKO);
    connect(&hub, &cli, &mut tab, "ask-server").await;

    // A hidden page says so at `received`: that line marks the point where the CLI
    // has read `received`, and no wait line came with it.
    let mut run = cli.call(["tasks.stop", "--id", "8812"]).spawn();
    let request = tab.request().await;
    let hidden = Received {
        hidden: true,
        change: true,
        ..Received::default()
    };
    tab.reply(&received(&request.id, hidden)).await;
    let before = run.stderr_until("the tab is in the background\n").await;
    assert!(!before.contains("Waiting for approval"), "{before:?}");
    tab.reply(&awaiting_approval(&request.id, ten_minutes()))
        .await;
    run.stderr_until(WAIT_LINE).await;
    tab.reply(
        &Finished::ok("Task 8812 stopped")
            .approval("card")
            .envelope(&request.id),
    )
    .await;
    let output = run.output().await;
    output.expect_code(0);
    // Once, after the background line, with the time left.
    assert_eq!(
        output.stderr,
        format!("the tab is in the background\n{WAIT_LINE}\n"),
        "{output}"
    );

    // A read and a change of the screen open no card: no wait line.
    for (args, change, approval) in [
        (&["search", "--ip", "1"][..], false, None),
        (&["nav.go", "--to", "/dashboard"][..], true, Some("none")),
    ] {
        let run = cli.call(args).spawn();
        let request = tab.request().await;
        let accepted = if change {
            received_change(&request.id)
        } else {
            received_read(&request.id)
        };
        tab.reply(&accepted).await;
        let mut finished = Finished::ok("done");
        if let Some(approval) = approval {
            finished = finished.approval(approval);
        }
        tab.reply(&finished.envelope(&request.id)).await;
        let output = run.output().await;
        output.expect_code(0);
        assert_eq!(output.stderr, "", "{output}");
    }

    tab.close().await;
    hub.stop();
}

#[tokio::test]
async fn an_approved_change_exits_0_with_approval_card() {
    for json in [false, true] {
        let mut card = open_card(json).await;
        let finished = Finished::ok("Task 8812 stopped").approval("card");
        card.tab.reply(&finished.envelope(&card.request.id)).await;
        let output = card.run.output().await;
        output.expect_code(0);
        if json {
            let result = output.json();
            assert_eq!(result["approval"], "card", "{output}");
            assert_eq!(result["status"], "ok", "{output}");
        } else {
            assert_eq!(output.stdout, "Task 8812 stopped\n", "{output}");
            // A card was asked: no note about a change without approval.
            assert!(!output.stderr.contains("without approval"), "{output}");
        }
        drop(card.cli);
        card.hub.stop();
    }
}

#[tokio::test]
async fn a_rejected_change_exits_3() {
    for json in [false, true] {
        let mut card = open_card(json).await;
        let rejected = Finished::error(
            "not-allowed",
            "denied",
            Some("The user rejected the change in the browser; nothing changed."),
        );
        card.tab.reply(&rejected.envelope(&card.request.id)).await;
        let output = card.run.output().await;
        output.expect_code(3);
        if json {
            let result = output.json();
            assert_eq!(result["status"], "error", "{output}");
            assert_eq!(result["code"], "not-allowed", "{output}");
            assert_eq!(result["exitCode"], 3, "{output}");
            assert!(result.get("approval").is_none(), "{output}");
            // A rejected card suggests nothing; an expired one does.
            assert!(result.get("suggest").is_none(), "{output}");
        } else {
            assert!(
                output.stderr.contains(
                    "error (not-allowed): The user rejected the change in the browser; nothing changed.\n"
                ),
                "{output}"
            );
            assert!(!output.stderr.contains("Did you mean"), "{output}");
        }
        card.hub.stop();
    }
}

#[tokio::test]
async fn a_lost_hub_during_the_approval_exits_5() {
    for json in [false, true] {
        let card = open_card(json).await;
        // The hub goes: the CLI's connection ends while the card is open, and the
        // change may still run.
        card.hub.stop();
        let output = card.run.output().await;
        output.expect_code(5);
        if json {
            let result = output.json();
            assert_eq!(result["status"], "unknown", "{output}");
            assert_eq!(result["code"], "unknown-outcome", "{output}");
            assert_eq!(result["complete"], false, "{output}");
        } else {
            assert!(
                output.stderr.contains("error (unknown-outcome): "),
                "{output}"
            );
            assert!(
                output
                    .stderr
                    .contains("the request may have run; check in the browser"),
                "{output}"
            );
        }
    }
}

/// The last reply (`finished`) of a 1.8.2 `sequence/approval.json` example.
fn approval_finished(name: &str) -> Value {
    let examples = fixture("1.8.2", "sequence/approval.json");
    let example = examples
        .as_array()
        .and_then(|list| list.iter().find(|example| example["name"] == name))
        .unwrap_or_else(|| panic!("sequence/approval.json has {name}"));
    let last = example["replies"]
        .as_array()
        .and_then(|replies| replies.last())
        .expect("replies");
    assert_eq!(last["kind"], "finished", "{name}");
    last.clone()
}

#[tokio::test]
async fn an_expired_approval_exits_3() {
    // command-console 1.8.2: a card that runs out ends as `not-allowed` (class
    // `denied`) and suggests the command again, without the approval field; an
    // approval pressed after the end changes nothing.
    for name in [
        "change-server-card-expired",
        "change-server-card-approved-late",
    ] {
        for json in [false, true] {
            expired_card(name, json).await;
        }
    }
}

async fn expired_card(name: &str, json: bool) {
    let mut card = open_card(json).await;
    let expired = replay(&approval_finished(name), &card.request.id);
    card.tab.reply(&expired).await;
    let output = card.run.output().await;
    output.expect_code(3);
    if json {
        let result = output.json();
        assert_eq!(result["status"], "error", "{output}");
        assert_eq!(result["code"], "not-allowed", "{output}");
        assert_eq!(result["exitCode"], 3, "{output}");
        assert_eq!(
            result["suggest"],
            json!(["tasks.stop", "--id", "8812"]),
            "{output}"
        );
        assert!(result.get("approval").is_none(), "{output}");
    } else {
        assert_eq!(output.stdout, "", "{output}");
        assert_eq!(
            output.stderr,
            format!(
                "{WAIT_LINE}\nerror (not-allowed): The approval expired; nothing changed.\nDid you mean: gecko tasks.stop --id 8812\n"
            ),
            "{output}"
        );
    }
    card.hub.stop();
}

#[tokio::test]
async fn a_reload_drops_the_card_and_exits_3() {
    for json in [false, true] {
        let mut card = open_card(json).await;
        // The page reloads: its binding is suspended, the card is gone with the page.
        card.tab.unload().await;
        let output = card.run.output().await;
        output.expect_code(3);
        if json {
            let result = output.json();
            assert_eq!(result["code"], "approval-dropped", "{output}");
            assert_eq!(result["exitCode"], 3, "{output}");
        } else {
            assert!(
                output.stderr.contains(
                    "error (approval-dropped): the Gecko tab reloaded or closed while the approval card was open; the command did not run\n"
                ),
                "{output}"
            );
        }
        card.hub.stop();
    }
}

// --- Found while writing these: waits for a decision (spec §2) ----------------------

/// A line over the limits is refused before connecting, as `too-large`. command-console
/// puts `too-large` in the class `usage` (exit 2), and so did the TypeScript CLI; this
/// CLI exits 1. The fix changes an exit code users see.
#[tokio::test]
#[ignore = "too-large exits 1, the contract says 2 (usage); a user-visible change waits for a decision"]
async fn a_line_over_the_limit_is_a_usage_error() {
    let cli = Cli::new(GECKO).json();
    let line = format!("search --q {}", "a".repeat(4_000));
    let output = cli.run(["--line", &line]).await;
    assert_eq!(output.json()["code"], "too-large", "{output}");
    output.expect_code(2);
}

// --- The testkit's own claim: a tab that does not read answers no heartbeat ---------

/// The binding's state as `gecko status --json` reports it.
async fn state(cli: &Cli) -> String {
    let output = cli.run(["--json", "status"]).await;
    output.expect_code(0);
    output.json()["data"]["binding"]["state"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// Asks the status until the binding is in `wanted`, within [`WAIT`].
async fn until_state(cli: &Cli, wanted: &str) {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let now = state(cli).await;
        if now == wanted {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the binding stayed {now}, not {wanted}, for {WAIT:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn a_silent_tab_is_found_by_the_heartbeat() {
    // Pings every second; two unanswered ones and the tab counts as lost.
    let hub = Hub::with(|options| {
        options.timings.heartbeat_ms = 1000.0;
        options.timings.heartbeat_misses = 2.0;
    });
    let mut tab = Tab::open(&hub, "p1").await;
    let cli = Cli::new(GECKO);
    connect(&hub, &cli, &mut tab, "ask-server").await;
    // The tab reads nothing now: its pings stay unanswered.
    until_state(&cli, "suspended").await;
    // One round trip reads the pings and answers them: the binding is back.
    tab.heartbeat().await;
    until_state(&cli, "active").await;
    tab.close().await;
    hub.stop();
}

// --- Row 11: Ctrl+C ------------------------------------------------------------------

/// A bound CLI whose `search` the tab has accepted (`received`), with the Ctrl+C hook
/// on; `wait_ms` is `GECKO_CANCEL_WAIT_MS` when given.
async fn running_read(wait_ms: Option<u64>) -> (Hub, Tab, Cli, Process, Request) {
    let hub = Hub::start();
    let mut tab = Tab::open(&hub, "p1").await;
    let cli = Cli::new(GECKO).json();
    connect(&hub, &cli, &mut tab, "ask-server").await;
    let mut call = cli.call(["search", "--ip", "1"]).interrupts();
    if let Some(wait) = wait_ms {
        call = call.env("GECKO_CANCEL_WAIT_MS", &wait.to_string());
    }
    let run = call.spawn();
    let request = tab.request().await;
    tab.reply(&received_read(&request.id)).await;
    (hub, tab, cli, run, request)
}

/// Waits for the CLI's `cancel` of `request`.
async fn saw_cancel(tab: &mut Tab, request: &Request) {
    let cancel = tab.request().await;
    assert_eq!(cancel.kind, "cancel", "{:?}", cancel.message);
    assert_eq!(cancel.id, request.id, "{:?}", cancel.message);
}

fn expect_cancelled(output: &Output, code: &str) {
    output.expect_code(130);
    let result = output.json();
    assert_eq!(result["status"], "cancelled", "{output}");
    assert_eq!(result["code"], code, "{output}");
    assert_eq!(result["complete"], false, "{output}");
}

#[tokio::test]
async fn ctrl_c_sends_cancel_and_the_tabs_answer_ends_it() {
    let (hub, mut tab, _cli, mut run, request) = running_read(None).await;
    run.interrupt();
    saw_cancel(&mut tab, &request).await;
    tab.reply(&cancel_requested(&request.id)).await;
    let cancelled = Finished::error("cancelled", "cancelled", None);
    tab.reply(&cancelled.envelope(&request.id)).await;
    expect_cancelled(&run.output().await, "cancel-requested");
    tab.close().await;
    hub.stop();
}

#[tokio::test]
async fn cancelled_before_start_exits_130() {
    // The answer comes at once; the CLI ends when its cancel wait (3 s) is over.
    let (hub, mut tab, _cli, mut run, request) = running_read(None).await;
    run.interrupt();
    saw_cancel(&mut tab, &request).await;
    tab.reply(&cancelled_before_start(&request.id)).await;
    expect_cancelled(&run.output().await, "cancelled-before-start");
    tab.close().await;
    hub.stop();
}

#[tokio::test]
async fn an_unanswered_cancel_ends_after_the_cancel_wait() {
    let (hub, mut tab, _cli, mut run, request) = running_read(Some(100)).await;
    let pressed = Instant::now();
    run.interrupt();
    saw_cancel(&mut tab, &request).await;
    // The tab says nothing more.
    let output = run.output().await;
    let took = pressed.elapsed();
    expect_cancelled(&output, "cancelled");
    // `GECKO_CANCEL_WAIT_MS` counts, not the default 3 s.
    assert!(took < Duration::from_millis(2500), "{took:?}\n{output}");
    tab.close().await;
    hub.stop();
}

#[tokio::test]
async fn a_second_ctrl_c_ends_the_wait() {
    // A cancel wait longer than any test: only the second Ctrl+C can end it in time.
    let (hub, mut tab, _cli, mut run, request) = running_read(Some(600_000)).await;
    run.interrupt();
    saw_cancel(&mut tab, &request).await;
    run.interrupt();
    expect_cancelled(&run.output().await, "cancelled");
    tab.close().await;
    hub.stop();
}

/// A real SIGINT, sent with `kill -INT` as a terminal does: the same cancel as the
/// test hook. Windows cannot send it to a child process.
#[cfg(unix)]
#[tokio::test]
async fn a_real_sigint_sends_cancel() {
    let hub = Hub::start();
    let mut tab = Tab::open(&hub, "p1").await;
    let cli = Cli::new(GECKO).json();
    connect(&hub, &cli, &mut tab, "ask-server").await;
    let run = cli.call(["search", "--ip", "1"]).spawn();
    // The request is out: the CLI listens for Ctrl+C by now.
    let request = tab.request().await;
    tab.reply(&received_read(&request.id)).await;
    let killed = std::process::Command::new("kill")
        .arg("-INT")
        .arg(run.pid().to_string())
        .status()
        .expect("kill runs");
    assert!(killed.success(), "kill -INT {}: {killed}", run.pid());
    saw_cancel(&mut tab, &request).await;
    tab.reply(&cancel_requested(&request.id)).await;
    let cancelled = Finished::error("cancelled", "cancelled", None);
    tab.reply(&cancelled.envelope(&request.id)).await;
    expect_cancelled(&run.output().await, "cancel-requested");
    tab.close().await;
    hub.stop();
}

// --- Row 13: the lines about the tab -------------------------------------------------

/// Runs `args` on a bound CLI; the tab accepts with `fields` and ends with `finished`.
async fn read_with(
    cli: &Cli,
    tab: &mut Tab,
    args: &[&str],
    fields: Received,
    finished: Finished,
) -> Output {
    let run = cli.call(args).spawn();
    let request = tab.request().await;
    tab.reply(&received(&request.id, fields)).await;
    tab.reply(&finished.envelope(&request.id)).await;
    run.output().await
}

#[tokio::test]
async fn a_hidden_tab_says_how_old_its_data_is() {
    let hub = Hub::start();
    let mut tab = Tab::open(&hub, "p1").await;
    let cli = Cli::new(GECKO);
    connect(&hub, &cli, &mut tab, "ask-server").await;

    let hidden = Received {
        hidden: true,
        data_age_ms: Some(42_400),
        ..Received::default()
    };
    let output = read_with(&cli, &mut tab, &["search"], hidden, Finished::ok("done")).await;
    output.expect_code(0);
    assert_eq!(
        output.stderr, "the tab is in the background; its data is 42 s old\n",
        "{output}"
    );
    assert_eq!(output.stdout, "done\n", "{output}");

    // A visible tab says nothing, whatever the age.
    let visible = Received {
        data_age_ms: Some(42_400),
        ..Received::default()
    };
    let output = read_with(&cli, &mut tab, &["search"], visible, Finished::ok("done")).await;
    output.expect_code(0);
    assert_eq!(output.stderr, "", "{output}");

    tab.close().await;
    hub.stop();
}

#[tokio::test]
async fn the_view_line_comes_only_in_a_data_view() {
    let hub = Hub::start();
    let mut tab = Tab::open(&hub, "p1").await;
    let cli = Cli::new(GECKO);
    connect(&hub, &cli, &mut tab, "ask-server").await;
    tab.publish_capabilities(capabilities_with(&[("dataview.open", "read")]))
        .await
        .expect("the list is taken");
    let with_view = || Finished::ok("done").view("v7", Some("Open tasks"));

    for (args, line) in [
        // A Data View's command, and any command run in a view.
        (&["dataview.open"][..], "View: Open tasks\n"),
        (&["--view", "v7", "search"][..], "View: Open tasks\n"),
        // Elsewhere the view the tab names is not a line.
        (&["search"][..], ""),
    ] {
        let output = read_with(&cli, &mut tab, args, Received::default(), with_view()).await;
        output.expect_code(0);
        assert_eq!(output.stderr, line, "{output}");
        assert_eq!(output.stdout, "done\n", "{output}");
    }

    // Without a title, the view's id.
    let output = read_with(
        &cli,
        &mut tab,
        &["dataview.open"],
        Received::default(),
        Finished::ok("done").view("v7", None),
    )
    .await;
    output.expect_code(0);
    assert_eq!(output.stderr, "View: v7\n", "{output}");

    tab.close().await;
    hub.stop();
}

// --- Row 14: no binding --------------------------------------------------------------

#[tokio::test]
async fn a_line_without_a_token_is_no_binding() {
    for json in [false, true] {
        let cli = cli(json);
        let output = cli.run(["--line", "search --ip 1"]).await;
        output.expect_code(4);
        if json {
            let result = output.json();
            assert_eq!(result["code"], "no-binding", "{output}");
            assert_eq!(result["exitCode"], 4, "{output}");
        } else {
            assert!(
                output.stderr.starts_with("error (no-binding): "),
                "{output}"
            );
            assert!(output.stderr.contains("gecko attach <code>"), "{output}");
        }
    }
}

// --- Row 15: the small ones ----------------------------------------------------------

#[tokio::test]
async fn line_dash_reads_the_line_from_stdin() {
    let hub = Hub::start();
    let mut tab = Tab::open(&hub, "p1").await;
    let cli = Cli::new(GECKO);
    connect(&hub, &cli, &mut tab, "ask-server").await;
    let run = cli
        .call(["--line", "-"])
        .stdin("search --ip \"a b\"\r\n")
        .spawn();
    let request = tab.request().await;
    assert_eq!(request.kind, "cli-request");
    // The line goes as it was written, less its line end; the tab parses it.
    assert_eq!(
        request.message["line"], "search --ip \"a b\"",
        "{:?}",
        request.message
    );
    assert!(request.argv.is_none(), "{:?}", request.message);
    tab.reply(&received_read(&request.id)).await;
    tab.reply(&Finished::ok("done").envelope(&request.id)).await;
    run.output().await.expect_code(0);
    tab.close().await;
    hub.stop();
}

#[tokio::test]
async fn a_rate_limit_over_5_s_is_busy_at_once() {
    // One request in ten seconds: the second send would wait ~10 s, over the CLI's
    // 5 s in all, so it ends at once as `busy`.
    let hub = Hub::with(|options| {
        options.rate_per_second = 0.1;
        options.rate_burst = Some(1.0);
    });
    let mut tab = Tab::open(&hub, "p1").await;
    let cli = Cli::new(GECKO).json();
    connect(&hub, &cli, &mut tab, "ask-server").await;
    let output = read_with(
        &cli,
        &mut tab,
        &["search"],
        Received::default(),
        Finished::ok("done"),
    )
    .await;
    output.expect_code(0);
    let started = Instant::now();
    let output = cli.run(["search"]).await;
    let took = started.elapsed();
    output.expect_code(4);
    assert_eq!(output.json()["code"], "busy", "{output}");
    assert!(took < Duration::from_secs(5), "{took:?}\n{output}");
    tab.close().await;
    hub.stop();
}

#[tokio::test]
async fn the_token_is_renewed_30_s_before_it_ends() {
    // A 30 s token is due at once; a 2 min one is not.
    for (ttl, renewed) in [(30_000.0, true), (120_000.0, false)] {
        let hub = Hub::with(|options| options.timings.access_ttl_ms = ttl);
        let mut tab = Tab::open(&hub, "p1").await;
        let cli = Cli::new(GECKO);
        connect(&hub, &cli, &mut tab, "ask-server").await;
        let before = cli.token().expect("a token");
        cli.run(["status"]).await.expect_code(0);
        let after = cli.token().expect("a token");
        assert_eq!(
            before["accessToken"] != after["accessToken"],
            renewed,
            "ttl {ttl}: {before} -> {after}"
        );
        if renewed {
            assert!(
                after["accessExpiresAt"].as_u64() > before["accessExpiresAt"].as_u64(),
                "{before} -> {after}"
            );
        }
        tab.close().await;
        hub.stop();
    }
}

/// `icacls` of `token.json`: inheritance removed, one entry, the user's, full control.
///
/// As for a user, the CLI makes its config directory itself (`%APPDATA%\gecko-cli`
/// does not exist before the first attach) and gives it to the user alone. A
/// directory made beforehand, under a parent with no inheritable entries, keeps the
/// explicit entries a new file gets there (`SYSTEM`, `Administrators`); that case is
/// not this test's (seen on the CI's Windows runner).
#[cfg(windows)]
#[tokio::test]
async fn the_token_file_is_the_users_alone() {
    let hub = Hub::start();
    let mut tab = Tab::open(&hub, "p1").await;
    let cli = Cli::new(GECKO);
    let dir = cli.dir().join("gecko-cli");
    let dir_text = dir.display().to_string();
    let code = tab.attach_code("ask-server").await;
    cli.call(["attach", "-", "--trust-hub"])
        .env("GECKO_CONFIG_DIR", &dir_text)
        .stdin(&format!("{code}\n"))
        .output()
        .await
        .expect_code(0);
    tab.bound().await;
    let path = dir.join("token.json");
    let listed = std::process::Command::new("icacls")
        .arg(&path)
        .output()
        .expect("icacls runs");
    assert!(listed.status.success(), "{listed:?}");
    let text = String::from_utf8_lossy(&listed.stdout).into_owned();
    // The path stands before the first entry; each entry is `<who>:<rights>`, and the
    // last line counts the files.
    let path_text = path.display().to_string();
    let entries: Vec<String> = text
        .lines()
        .map(|line| {
            line.strip_prefix(path_text.as_str())
                .unwrap_or(line)
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|line| line.contains(":("))
        .collect();
    let user = std::env::var("USERNAME")
        .expect("USERNAME")
        .to_ascii_lowercase();
    assert_eq!(entries.len(), 1, "{text}");
    let entry = &entries[0];
    let who = entry.split(":(").next().unwrap_or_default();
    assert!(who == user || who.ends_with(&format!("\\{user}")), "{text}");
    // Full control, not inherited (`(I)`).
    assert_eq!(&entry[who.len()..], ":(f)", "{text}");
    tab.close().await;
    hub.stop();
}

// --- command-console 1.8.2 `sequence/`, end to end -----------------------------------

/// Plays every example of one 1.8.2 `sequence/` file through the real binary and the
/// real hub: the fake tab sends each example's `replies` as they are; `connection-lost`
/// stops the hub, `timeout` is silence past a short delivery window, `ctrl-c` is the
/// test hook and waits for the CLI's `cancel`; `tab.expire` lets the card run out
/// before the tab's last word. Returns how many examples ran.
async fn sequences(file: &str) -> usize {
    let examples = fixture("1.8.2", &format!("sequence/{file}"));
    let examples = examples.as_array().expect("a list of examples");
    for example in examples {
        play(file, example).await;
    }
    examples.len()
}

async fn play(file: &str, example: &Value) {
    let name = format!("{file}/{}", example["name"].as_str().unwrap_or("?"));
    let replies = example["replies"].as_array().expect("replies");
    let event = |wanted: &str| replies.iter().any(|item| item["event"] == wanted);
    let timeout = event("timeout");
    let expire = example["tab"]["expire"] == true;
    let hub = Hub::with(|options| {
        if timeout {
            options.timings.request_ttl_ms = 300.0;
        }
    });
    let mut tab = Tab::open(&hub, "p1").await;
    let cli = Cli::new(GECKO).json();
    let level = example["tab"]["level"].as_str().unwrap_or("ask-server");
    connect(&hub, &cli, &mut tab, level).await;
    // The examples' read command is `tasks.list`.
    tab.publish_capabilities(capabilities_with(&[("tasks.list", "read")]))
        .await
        .expect("the list is taken");
    let argv: Vec<&str> = example["request"]["argv"]
        .as_array()
        .expect("argv")
        .iter()
        .map(|word| word.as_str().expect("a word"))
        .collect();
    let mut call = cli.call(&argv);
    if event("ctrl-c") {
        call = call.interrupts();
    }
    if timeout {
        call = call.env("GECKO_DELIVERY_MARGIN_MS", "0");
    }
    let mut run = call.spawn();
    let request = tab.request().await;
    let sent: Vec<&str> = request.argv.iter().flatten().map(String::as_str).collect();
    assert_eq!(sent, argv, "{name}");
    let mut hub = Some(hub);
    for item in replies {
        match item["event"].as_str() {
            Some("connection-lost") => {
                if let Some(hub) = hub.take() {
                    hub.stop();
                }
            }
            // Silence: the window runs out.
            Some("timeout") => {}
            Some("ctrl-c") => {
                run.interrupt();
                saw_cancel(&mut tab, &request).await;
            }
            Some(other) => panic!("{name}: unknown event {other}"),
            None => {
                tab.reply(&replay(item, &request.id)).await;
                // `tab.expire`: the card's time runs out before the tab's last word.
                // The example's `expiresAt` is on its own clock, long past on ours:
                // once the CLI shows the card with no time left, it must still wait
                // for the tab (each later reply checks it is there to take it).
                if expire && item["kind"] == "awaiting-approval" {
                    run.stderr_until("Waiting for approval (~0 s)").await;
                }
            }
        }
    }
    let output = run.output().await;
    let expect = &example["expect"];
    let code = expect["exitCode"].as_i64().expect("exitCode");
    output.expect_code(i32::try_from(code).expect("an exit code"));
    let result = output.json();
    for (key, value) in expect["json"].as_object().expect("expect.json") {
        assert_eq!(&result[key], value, "{name}: {key}\n{output}");
    }
    for field in expect["absent"].as_array().into_iter().flatten() {
        let field = field.as_str().expect("a field name");
        assert!(result.get(field).is_none(), "{name}: {field}\n{output}");
    }
    if let Some(hub) = hub {
        tab.close().await;
        hub.stop();
    }
}

#[tokio::test]
async fn sequence_approval() {
    // 1.8.2: with the expired card (`change-server-card-expired`,
    // `change-server-card-approved-late`).
    assert_eq!(sequences("approval.json").await, 9);
}

#[tokio::test]
async fn sequence_cancel() {
    assert_eq!(sequences("cancel.json").await, 2);
}

#[tokio::test]
async fn sequence_chunks() {
    assert_eq!(sequences("chunks.json").await, 3);
}

#[tokio::test]
async fn sequence_class() {
    assert_eq!(sequences("class.json").await, 4);
}

#[tokio::test]
async fn sequence_delivery() {
    assert_eq!(sequences("delivery.json").await, 10);
}
