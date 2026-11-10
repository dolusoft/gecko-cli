//! End to end: the real binary against the real development hub (gecko-testkit `Hub`)
//! and a fake tab that answers with command-console 1.8.0 replies. Black box: argv,
//! stdin and environment in; exit code, stdout and stderr out. Both feature sets run
//! these: the hub comes from the testkit, not from the binary.

use gecko_testkit::envelope::{
    Finished, Received, awaiting_approval, chunk, received, received_change, received_read,
};
use gecko_testkit::{Cli, Hub, Process, Request, Tab, WAIT, connect, disconnect, now_ms};
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
        } else {
            assert!(
                output.stderr.contains(
                    "error (not-allowed): The user rejected the change in the browser; nothing changed.\n"
                ),
                "{output}"
            );
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

#[tokio::test]
async fn an_expired_approval_exits_3() {
    for json in [false, true] {
        let mut card = open_card(json).await;
        // The fixtures have no example of an expired card; the tab refuses the change
        // as `not-allowed` (class `denied`), as for a rejection.
        let expired = Finished::error(
            "not-allowed",
            "denied",
            Some("The approval expired in the browser; nothing changed."),
        );
        card.tab.reply(&expired.envelope(&card.request.id)).await;
        let output = card.run.output().await;
        output.expect_code(3);
        if json {
            assert_eq!(output.json()["code"], "not-allowed", "{output}");
        } else {
            assert!(
                output
                    .stderr
                    .contains("error (not-allowed): The approval expired"),
                "{output}"
            );
        }
        card.hub.stop();
    }
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
