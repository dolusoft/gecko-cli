//! The command-console fixtures the fake tab follows (`fixtures/command-console-*/`).

use std::path::PathBuf;

use serde_json::{Value, json};

/// The command-console version the fake tab plays.
pub const VERSION: &str = "1.8.2";

/// One fixture file of `remote/`, such as `valid/replies.json`.
pub fn fixture(version: &str, file: &str) -> Value {
    let path = PathBuf::from(env!(
        "GECKO_CLI_FIXTURES_DIR",
        "set in .cargo/config.toml; run cargo inside the repository"
    ))
    .join(format!("command-console-{version}"))
    .join("remote")
    .join(file);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The capability list the fake tab publishes, as `publishCapabilities` takes it
/// (`{ revision, parts }`): the 1.8.2 example `capabilities-effect` of
/// `valid/capabilities.json`. Its commands are `search` (read, alias `s`), `nav.go`
/// (a change of the screen) and `tasks.stop` (a change on the server, with a card at
/// `ask-server`); `login` and `signin` are sensitive.
pub fn capabilities() -> Value {
    let examples = fixture(VERSION, "valid/capabilities.json");
    let example = examples
        .as_array()
        .and_then(|list| {
            list.iter()
                .find(|example| example["name"] == "capabilities-effect")
        })
        .expect("valid/capabilities.json has capabilities-effect");
    let revision = example["read"]["revision"].clone();
    assert!(revision.is_string(), "capabilities-effect has a revision");
    json!({ "revision": revision, "parts": example["parts"] })
}

/// [`capabilities`] with more commands after the fixture's, each `(name, danger)` with
/// no aliases and no effect: commands a test needs that the 1.8.2 example lacks, such
/// as the `tasks.list` of the `sequence/` examples or a Data View's `dataview.open`.
pub fn capabilities_with(extra: &[(&str, &str)]) -> Value {
    let mut published = capabilities();
    let part = published["parts"][0]
        .as_str()
        .expect("capabilities-effect has one part");
    let mut list: Value = serde_json::from_str(part).expect("the part is JSON");
    let commands = list["commands"]
        .as_array_mut()
        .expect("the part lists commands");
    for (name, danger) in extra {
        commands.push(json!({ "name": name, "aliases": [], "danger": danger }));
    }
    published["parts"] = json!([list.to_string()]);
    published
}
