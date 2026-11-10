//! The architecture of the workspace as a test: the role of every crate, which crate
//! may depend on which, the external crates of the inner roles, lints, Clippy copies,
//! I/O placement inside module crates, no source pulled in by path, the legacy list and
//! the release graph.
//!
//! Every check is a pure function over parsed data (the output of `cargo metadata`,
//! manifest text, source text) and has a unit test that feeds it a broken input and
//! expects a finding. The workspace test at the top runs them all on the real data.
//!
//! This is a gate file: a change to it, to a `clippy.toml` or to the lint tables of the
//! root `Cargo.toml` is a pull request of its own, with the reason in its description.
//!
//! Interim: B0 of the restructuring. The binary's crate is `legacy` until its code has
//! moved to the bounded contexts; the list of legacy crates only shrinks.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

// ---------------------------------------------------------------------------------
// The rules as data
// ---------------------------------------------------------------------------------

/// The role of a crate. Contract, domain and module belong to a bounded context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Kernel,
    Protocol,
    Contract(&'static str),
    Domain(&'static str),
    Module(&'static str),
    AdapterLib,
    DevTool,
    TestSupport,
    App,
    Legacy,
}

/// Every workspace crate by package name, with its role.
const ROLES: &[(&str, Role)] = &[
    ("gecko", Role::Legacy),
    ("gecko-kernel", Role::Kernel),
    ("gecko-wire", Role::Protocol),
    ("gecko-hub-protocol", Role::Protocol),
    ("gecko-hub-client", Role::AdapterLib),
    ("gecko-command-contract", Role::Contract("command")),
    ("gecko-command-domain", Role::Domain("command")),
    ("gecko-command-module", Role::Module("command")),
    ("gecko-dev-hub", Role::DevTool),
    ("gecko-testkit", Role::TestSupport),
];

/// The crates allowed in the legacy role. A crate leaves the list when its code has
/// moved; nothing is added.
const LEGACY: &[&str] = &["gecko"];

/// The legacy list as it stood at B0. Frozen: [`LEGACY`] must stay inside it, so the
/// list cannot grow without editing this line too, in a gate pull request.
const LEGACY_AT_B0: &[&str] = &["gecko"];

/// External crates the inner roles (kernel, protocol, contract, domain) may use.
const EXTERNAL_ALLOWED: &[&str] = &["serde", "serde_json"];

/// The workspace lint table, `(table, lint, level)`.
const WORKSPACE_LINTS: &[(&str, &str, &str)] = &[
    ("rust", "unsafe_code", "deny"),
    ("rust", "missing_debug_implementations", "warn"),
    ("clippy", "dbg_macro", "warn"),
    ("clippy", "print_stderr", "warn"),
    ("clippy", "print_stdout", "warn"),
    ("clippy", "todo", "warn"),
    ("clippy", "unwrap_used", "warn"),
];

/// What the `clippy.toml` of a kernel or domain crate disallows. Clippy takes no
/// wildcards in these lists (checked at B0: `std::env::*` refers to nothing and only
/// warns), so every path is spelled out.
const CLIPPY_DISALLOWED: &[&str] = &[
    "std::time::SystemTime::now",
    "std::time::Instant::now",
    "std::env::var",
    "std::env::var_os",
    "std::io::stdin",
    "std::io::stdout",
    "std::io::stderr",
    "std::thread::spawn",
    "std::fs::File",
    "std::net::TcpStream",
    "std::process::Command",
    "std::collections::hash_map::RandomState",
];

/// I/O a module crate keeps under `src/adapters/`.
const MODULE_IO: &[&str] = &[
    "tokio::net",
    "tokio_tungstenite",
    "reqwest",
    "std::fs",
    "std::net",
    "std::process",
];

/// Files that still pull source in by path, relative to the workspace root. The list
/// only shrinks: a file that no longer does is a finding until it leaves the list.
/// Empty since B3, where `sequence.rs` became the command domain's own test.
const BY_PATH_PENDING: &[&str] = &[];

/// Crates the release binary must not contain, besides every test-support crate.
const NOT_IN_RELEASE: &[&str] = &["gecko-dev-hub", "sha2"];

/// The binary's package.
const BINARY: &str = "gecko";

// ---------------------------------------------------------------------------------
// Parsed data
// ---------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Normal,
    Dev,
    Build,
}

impl Kind {
    fn of(value: &Value) -> Kind {
        match value.as_str() {
            Some("dev") => Kind::Dev,
            Some("build") => Kind::Build,
            _ => Kind::Normal,
        }
    }
}

/// A dependency as a manifest declares it.
#[derive(Debug, Clone)]
struct Dep {
    /// The real package name, not a rename.
    name: String,
    kind: Kind,
    optional: bool,
    /// A path dependency, that is a workspace crate.
    local: bool,
}

/// A workspace member.
#[derive(Debug, Clone)]
struct Member {
    name: String,
    /// The crate's folder relative to the workspace root, with `/`.
    dir: String,
    deps: Vec<Dep>,
}

/// The resolved graph: package name to `(dependency name, kinds of the edge)`.
type Graph = BTreeMap<String, Vec<(String, Vec<Kind>)>>;

#[derive(Debug)]
struct Workspace {
    root: PathBuf,
    members: Vec<Member>,
    graph: Graph,
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn parse_metadata(metadata: &Value) -> Workspace {
    let root = PathBuf::from(metadata["workspace_root"].as_str().unwrap());
    let ids: BTreeSet<&str> = metadata["workspace_members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap())
        .collect();
    let packages = metadata["packages"].as_array().unwrap();
    let name_of: BTreeMap<&str, &str> = packages
        .iter()
        .map(|p| (p["id"].as_str().unwrap(), p["name"].as_str().unwrap()))
        .collect();
    let members = packages
        .iter()
        .filter(|p| ids.contains(p["id"].as_str().unwrap()))
        .map(|p| {
            let manifest = Path::new(p["manifest_path"].as_str().unwrap());
            Member {
                name: p["name"].as_str().unwrap().to_owned(),
                dir: relative(&root, manifest.parent().unwrap()),
                deps: p["dependencies"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|d| Dep {
                        name: d["name"].as_str().unwrap().to_owned(),
                        kind: Kind::of(&d["kind"]),
                        optional: d["optional"].as_bool().unwrap_or(false),
                        local: d["path"].is_string(),
                    })
                    .collect(),
            }
        })
        .collect();
    let mut graph = Graph::new();
    for node in metadata["resolve"]["nodes"].as_array().unwrap() {
        let edges = node["deps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                let kinds = d["dep_kinds"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|k| Kind::of(&k["kind"]))
                    .collect();
                (name_of[d["pkg"].as_str().unwrap()].to_owned(), kinds)
            })
            .collect();
        graph.insert(name_of[node["id"].as_str().unwrap()].to_owned(), edges);
    }
    Workspace {
        root,
        members,
        graph,
    }
}

/// `cargo metadata` of this workspace; `release` resolves it with
/// `--no-default-features`, as the release workflow builds.
fn metadata(release: bool) -> Value {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let mut command = Command::new(cargo);
    command
        .args(["metadata", "--format-version", "1", "--locked"])
        .arg("--manifest-path")
        .arg(manifest);
    if release {
        command.arg("--no-default-features");
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "cargo metadata: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// Every `Cargo.toml` under `crates/`, relative to the root; `target` folders skipped.
fn manifests_on_disk(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n != "target") {
                    walk(root, &path, found);
                }
            } else if path.file_name().is_some_and(|n| n == "Cargo.toml") {
                found.push(relative(root, &path));
            }
        }
    }
    let mut found = Vec::new();
    walk(root, &root.join("crates"), &mut found);
    found.sort();
    found
}

/// Every `.rs` file under `dir`, as `(path relative to the root, text)`.
fn sources(root: &Path, dir: &Path) -> Vec<(String, String)> {
    fn walk(root: &Path, dir: &Path, found: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n != "target") {
                    walk(root, &path, found);
                }
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                found.push((relative(root, &path), text));
            }
        }
    }
    let mut found = Vec::new();
    if dir.is_dir() {
        walk(root, dir, &mut found);
    }
    found.sort();
    found
}

/// `(section, key, value)` of every `key = value` line of a TOML text. Enough for the
/// manifests here: comments are dropped, a line without `=` (inside an array) is
/// skipped, values stay as written.
fn toml_entries(text: &str) -> Vec<(String, String, String)> {
    let mut section = String::new();
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = strip_comment(line).trim();
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = header.trim().to_owned();
        } else if let Some((key, value)) = line.split_once('=') {
            entries.push((
                section.clone(),
                key.trim().to_owned(),
                value.trim().to_owned(),
            ));
        }
    }
    entries
}

fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..i],
            _ => {}
        }
    }
    line
}

/// The meaningful lines of a TOML text: trimmed, without comments and empty lines.
fn toml_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| strip_comment(l).trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect()
}

fn role_of(roles: &[(&str, Role)], name: &str) -> Option<Role> {
    roles.iter().find(|(n, _)| *n == name).map(|(_, r)| *r)
}

// ---------------------------------------------------------------------------------
// The checks: each returns its findings; none means green
// ---------------------------------------------------------------------------------

/// R1: every member has a role, every role names a member, and every `Cargo.toml`
/// under `crates/` is a member.
fn check_roles(members: &[Member], roles: &[(&str, Role)], on_disk: &[String]) -> Vec<String> {
    let mut findings = Vec::new();
    for member in members {
        if role_of(roles, &member.name).is_none() {
            findings.push(format!("{}: member without a role", member.name));
        }
    }
    for (name, _) in roles {
        if !members.iter().any(|m| m.name == *name) {
            findings.push(format!("{name}: has a role but is not a member"));
        }
    }
    for manifest in on_disk {
        let dir = manifest.trim_end_matches("/Cargo.toml");
        if !members.iter().any(|m| m.dir == dir) {
            findings.push(format!(
                "{manifest}: a crate that is not a workspace member"
            ));
        }
    }
    findings
}

/// Whether a crate of role `from` may depend on one of role `to` (spec §4.1).
fn edge_allowed(from: Role, to: Role, kind: Kind, optional: bool) -> bool {
    use Role::*;
    // Test support appears only among dev-dependencies, of the binary.
    if to == TestSupport {
        return kind == Kind::Dev && matches!(from, App | Legacy);
    }
    // The dev hub: the binary behind its feature, and test support.
    if to == DevTool {
        return (matches!(from, App | Legacy) && optional && kind == Kind::Normal)
            || from == TestSupport;
    }
    match from {
        Kernel | DevTool => false,
        Protocol => to == Kernel,
        AdapterLib => matches!(to, Kernel | Protocol),
        Contract(_) => matches!(to, Kernel | Protocol),
        Domain(context) => to == Contract(context) || matches!(to, Kernel | Protocol),
        Module(context) => {
            to == Domain(context) || matches!(to, Contract(_) | Kernel | Protocol | AdapterLib)
        }
        TestSupport | App | Legacy => true,
    }
}

/// R2: the dependency matrix over workspace crates, dev-dependencies included.
fn check_matrix(members: &[Member], roles: &[(&str, Role)]) -> Vec<String> {
    let mut findings = Vec::new();
    for member in members {
        let Some(from) = role_of(roles, &member.name) else {
            continue;
        };
        for dep in member.deps.iter().filter(|d| d.local) {
            match role_of(roles, &dep.name) {
                None => findings.push(format!(
                    "{} -> {}: the dependency has no role",
                    member.name, dep.name
                )),
                Some(to) if !edge_allowed(from, to, dep.kind, dep.optional) => {
                    findings.push(format!(
                        "{} ({from:?}) -> {} ({to:?}, {:?}{}): not allowed",
                        member.name,
                        dep.name,
                        dep.kind,
                        if dep.optional { ", optional" } else { "" }
                    ));
                }
                Some(_) => {}
            }
        }
    }
    findings
}

/// R3: kernel, protocol, contract and domain crates use only allowed external crates.
fn check_external(members: &[Member], roles: &[(&str, Role)], allowed: &[&str]) -> Vec<String> {
    let mut findings = Vec::new();
    for member in members {
        let inner = matches!(
            role_of(roles, &member.name),
            Some(Role::Kernel | Role::Protocol | Role::Contract(_) | Role::Domain(_))
        );
        if !inner {
            continue;
        }
        for dep in member.deps.iter().filter(|d| !d.local) {
            if !allowed.contains(&dep.name.as_str()) {
                findings.push(format!(
                    "{} -> {} ({:?}): external crate not allowed here",
                    member.name, dep.name, dep.kind
                ));
            }
        }
    }
    findings
}

/// R4: every member manifest takes the workspace lints, and the workspace lint table is
/// exactly `expected`.
fn check_lints(
    manifests: &[(String, String)],
    root_manifest: &str,
    expected: &[(&str, &str, &str)],
) -> Vec<String> {
    let mut findings = Vec::new();
    for (name, text) in manifests {
        let entries = toml_entries(text);
        let inherits = entries
            .iter()
            .any(|(s, k, v)| s == "lints" && k == "workspace" && v == "true");
        let own = entries
            .iter()
            .any(|(s, k, _)| s.starts_with("lints") && !(s == "lints" && k == "workspace"));
        if !inherits {
            findings.push(format!("{name}: no `[lints] workspace = true`"));
        }
        if own {
            findings.push(format!("{name}: lint levels of its own"));
        }
    }
    let actual: BTreeSet<(String, String, String)> = toml_entries(root_manifest)
        .into_iter()
        .filter_map(|(s, k, v)| {
            let table = s.strip_prefix("workspace.lints.")?.to_owned();
            Some((table, k, v.trim_matches('"').to_owned()))
        })
        .collect();
    let wanted: BTreeSet<(String, String, String)> = expected
        .iter()
        .map(|(t, k, v)| ((*t).to_owned(), (*k).to_owned(), (*v).to_owned()))
        .collect();
    for (t, k, v) in wanted.difference(&actual) {
        findings.push(format!("[workspace.lints.{t}] lacks {k} = \"{v}\""));
    }
    for (t, k, v) in actual.difference(&wanted) {
        findings.push(format!(
            "[workspace.lints.{t}] has {k} = \"{v}\", not in the test's list"
        ));
    }
    findings
}

/// R5: a kernel or domain crate has a `clippy.toml` with every line of the root one
/// (Clippy does not merge configuration files) and every disallowed path.
/// `copies` is `(crate, role, its clippy.toml if any)`.
fn check_clippy(
    copies: &[(String, Role, Option<String>)],
    root_clippy: &str,
    disallowed: &[&str],
) -> Vec<String> {
    let mut findings = Vec::new();
    let root_lines = toml_lines(root_clippy);
    for (name, role, copy) in copies {
        if !matches!(role, Role::Kernel | Role::Domain(_)) {
            continue;
        }
        let Some(copy) = copy else {
            findings.push(format!("{name}: no clippy.toml"));
            continue;
        };
        let lines = toml_lines(copy);
        for line in &root_lines {
            if !lines.contains(line) {
                findings.push(format!("{name}/clippy.toml lacks the root line `{line}`"));
            }
        }
        for path in disallowed {
            if !copy.contains(&format!("\"{path}\"")) {
                findings.push(format!("{name}/clippy.toml does not disallow {path}"));
            }
        }
    }
    findings
}

/// R6: in a module crate, I/O appears only under `src/adapters/`. `files` are the
/// crate's sources as `(path relative to the crate, text)`. A text scan.
fn check_module_io(crate_name: &str, files: &[(String, String)], io: &[&str]) -> Vec<String> {
    let mut findings = Vec::new();
    for (path, text) in files {
        if !path.starts_with("src/") || path.starts_with("src/adapters/") {
            continue;
        }
        for needle in io {
            if text.contains(needle) {
                findings.push(format!(
                    "{crate_name}/{path}: {needle} outside src/adapters/"
                ));
            }
        }
    }
    findings
}

/// Whether `text` has a path attribute or the `include` macro.
fn pulls_in_by_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let after_spaces = |mut i: usize| {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        i
    };
    for (i, _) in text.match_indices('#') {
        let mut j = after_spaces(i + 1);
        if bytes.get(j) == Some(&b'!') {
            j = after_spaces(j + 1);
        }
        if bytes.get(j) != Some(&b'[') {
            continue;
        }
        let j = after_spaces(j + 1);
        if text[j..].starts_with("path") && !bytes.get(j + 4).is_some_and(|b| ident(*b)) {
            return true;
        }
    }
    for (i, word) in text.match_indices("include") {
        let before = i.checked_sub(1).map(|p| bytes[p]);
        if before.is_some_and(ident) {
            continue;
        }
        let j = after_spaces(i + word.len());
        if bytes.get(j) == Some(&b'!') {
            return true;
        }
    }
    false
}

/// R7: no crate pulls source in by path, except the pending files; a pending file that
/// no longer does (or is gone) must leave the list.
fn check_by_path(files: &[(String, String)], pending: &[&str]) -> Vec<String> {
    let mut findings = Vec::new();
    for (path, text) in files {
        if pulls_in_by_path(text) && !pending.contains(&path.as_str()) {
            findings.push(format!("{path}: source pulled in by path"));
        }
    }
    for path in pending {
        let still = files
            .iter()
            .any(|(p, text)| p == path && pulls_in_by_path(text));
        if !still {
            findings.push(format!(
                "{path}: no longer pending; remove it from the list"
            ));
        }
    }
    findings
}

/// R14: legacy crates are on the legacy list, and the list is inside the B0 one.
fn check_legacy(roles: &[(&str, Role)], legacy: &[&str], at_b0: &[&str]) -> Vec<String> {
    let mut findings = Vec::new();
    for (name, role) in roles {
        if *role == Role::Legacy && !legacy.contains(name) {
            findings.push(format!("{name}: legacy but not on the legacy list"));
        }
    }
    for name in legacy {
        if !at_b0.contains(name) {
            findings.push(format!("{name}: the legacy list only shrinks"));
        }
    }
    findings
}

/// The crates `root` links: its graph through normal and build edges only.
fn linked(graph: &Graph, root: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![root.to_owned()];
    while let Some(name) = stack.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        for (dep, kinds) in graph.get(&name).into_iter().flatten() {
            if kinds.iter().any(|k| *k != Kind::Dev) {
                stack.push(dep.clone());
            }
        }
    }
    seen
}

/// Release graph: the binary's crates, resolved without default features, contain no
/// forbidden crate.
fn check_release(graph: &Graph, binary: &str, forbidden: &[String]) -> Vec<String> {
    if !graph.contains_key(binary) {
        return vec![format!("{binary}: not in the resolved graph")];
    }
    let crates = linked(graph, binary);
    forbidden
        .iter()
        .filter(|name| crates.contains(*name))
        .map(|name| format!("{name}: in the release graph of {binary}"))
        .collect()
}

// ---------------------------------------------------------------------------------
// The workspace
// ---------------------------------------------------------------------------------

fn forbidden_in_release() -> Vec<String> {
    NOT_IN_RELEASE
        .iter()
        .map(|n| (*n).to_owned())
        .chain(
            ROLES
                .iter()
                .filter(|(_, r)| *r == Role::TestSupport)
                .map(|(n, _)| (*n).to_owned()),
        )
        .collect()
}

fn assert_green(check: &str, findings: &[String]) {
    assert!(findings.is_empty(), "{check}:\n  {}", findings.join("\n  "));
}

#[test]
fn the_workspace_keeps_its_architecture() {
    let workspace = parse_metadata(&metadata(true));
    let root = &workspace.root;
    let members = &workspace.members;
    let read = |path: PathBuf| std::fs::read_to_string(path).unwrap();
    let crate_dir = |m: &Member| root.join(&m.dir);

    assert_green(
        "R1 roles",
        &check_roles(members, ROLES, &manifests_on_disk(root)),
    );
    assert_green("R2 matrix", &check_matrix(members, ROLES));
    assert_green(
        "R3 external crates",
        &check_external(members, ROLES, EXTERNAL_ALLOWED),
    );

    let manifests: Vec<(String, String)> = members
        .iter()
        .map(|m| (m.name.clone(), read(crate_dir(m).join("Cargo.toml"))))
        .collect();
    assert_green(
        "R4 lints",
        &check_lints(&manifests, &read(root.join("Cargo.toml")), WORKSPACE_LINTS),
    );

    let copies: Vec<(String, Role, Option<String>)> = members
        .iter()
        .filter_map(|m| {
            let role = role_of(ROLES, &m.name)?;
            let copy = std::fs::read_to_string(crate_dir(m).join("clippy.toml")).ok();
            Some((m.name.clone(), role, copy))
        })
        .collect();
    assert_green(
        "R5 clippy",
        &check_clippy(&copies, &read(root.join("clippy.toml")), CLIPPY_DISALLOWED),
    );

    for member in members {
        if let Some(Role::Module(_)) = role_of(ROLES, &member.name) {
            let files: Vec<(String, String)> = sources(&crate_dir(member), &crate_dir(member));
            assert_green(
                "R6 module I/O",
                &check_module_io(&member.name, &files, MODULE_IO),
            );
        }
    }

    let all: Vec<(String, String)> = members
        .iter()
        .flat_map(|m| sources(root, &crate_dir(m)))
        .collect();
    assert_green("R7 by path", &check_by_path(&all, BY_PATH_PENDING));
    assert_green("R14 legacy", &check_legacy(ROLES, LEGACY, LEGACY_AT_B0));
    assert_green(
        "release graph",
        &check_release(&workspace.graph, BINARY, &forbidden_in_release()),
    );
}

/// The release check on the default build, where the dev hub is in: it must see it.
/// Proves the check reads the real graph, not an empty one.
#[test]
fn the_release_check_sees_the_dev_hub_of_the_default_build() {
    let workspace = parse_metadata(&metadata(false));
    let findings = check_release(&workspace.graph, BINARY, &forbidden_in_release());
    assert!(
        findings.iter().any(|f| f.starts_with("gecko-dev-hub:")),
        "{findings:?}"
    );
    assert!(
        findings.iter().any(|f| f.starts_with("sha2:")),
        "{findings:?}"
    );
    // The testkit is a dev-dependency only: not linked even here.
    assert!(
        !findings.iter().any(|f| f.starts_with("gecko-testkit:")),
        "{findings:?}"
    );
}

// ---------------------------------------------------------------------------------
// Each check goes red on a broken input
// ---------------------------------------------------------------------------------

fn dep(name: &str, kind: Kind, local: bool) -> Dep {
    Dep {
        name: name.to_owned(),
        kind,
        optional: false,
        local,
    }
}

fn member(name: &str, dir: &str, deps: Vec<Dep>) -> Member {
    Member {
        name: name.to_owned(),
        dir: dir.to_owned(),
        deps,
    }
}

#[test]
fn r1_finds_a_crate_without_a_role_and_an_orphan_manifest() {
    let roles = &[("a", Role::Kernel), ("gone", Role::Kernel)];
    let members = [
        member("a", "crates/a", vec![]),
        member("b", "crates/b", vec![]),
    ];
    let disk = [
        "crates/a/Cargo.toml".to_owned(),
        "crates/x/y/Cargo.toml".to_owned(),
    ];
    let findings = check_roles(&members, roles, &disk);
    assert_eq!(findings.len(), 3, "{findings:?}");
    assert!(findings[0].starts_with("b:"));
    assert!(findings[1].starts_with("gone:"));
    assert!(findings[2].starts_with("crates/x/y/Cargo.toml:"));
    let ok = check_roles(&members[..1], &roles[..1], &disk[..1]);
    assert!(ok.is_empty(), "{ok:?}");
}

#[test]
fn r2_finds_edges_against_the_matrix() {
    let roles = &[
        ("k", Role::Kernel),
        ("p", Role::Protocol),
        ("d", Role::Domain("binding")),
        ("c", Role::Contract("command")),
        ("m", Role::Module("binding")),
        ("m2", Role::Module("command")),
        ("a", Role::AdapterLib),
        ("h", Role::DevTool),
        ("t", Role::TestSupport),
        ("g", Role::Legacy),
        ("x", Role::App),
    ];
    let mut optional_hub = dep("h", Kind::Normal, true);
    optional_hub.optional = true;
    let good = [
        member("p", "", vec![dep("k", Kind::Normal, true)]),
        member(
            "m",
            "",
            vec![dep("d", Kind::Normal, true), dep("c", Kind::Normal, true)],
        ),
        member(
            "g",
            "",
            vec![optional_hub.clone(), dep("t", Kind::Dev, true)],
        ),
        member("t", "", vec![dep("h", Kind::Normal, true)]),
        member(
            "x",
            "",
            vec![dep("m", Kind::Normal, true), dep("t", Kind::Dev, true)],
        ),
    ];
    let ok = check_matrix(&good, roles);
    assert!(ok.is_empty(), "{ok:?}");

    let bad = [
        // Kernel depends on nothing in the workspace.
        member("k", "", vec![dep("p", Kind::Normal, true)]),
        // Protocol may not use test support, even as a dev-dependency.
        member("p", "", vec![dep("t", Kind::Dev, true)]),
        // A domain uses only its own contract.
        member("d", "", vec![dep("c", Kind::Normal, true)]),
        // A module uses no other module.
        member("m", "", vec![dep("m2", Kind::Normal, true)]),
        // The adapter library is below the domain.
        member("a", "", vec![dep("d", Kind::Normal, true)]),
        // The binary takes test support only as a dev-dependency, the hub only
        // behind a feature; a dependency without a role is a finding too.
        member(
            "g",
            "",
            vec![
                dep("t", Kind::Normal, true),
                dep("h", Kind::Normal, true),
                dep("nobody", Kind::Normal, true),
            ],
        ),
        // The dev tool uses no workspace crate.
        member("h", "", vec![dep("p", Kind::Normal, true)]),
    ];
    let findings = check_matrix(&bad, roles);
    assert_eq!(findings.len(), 9, "{findings:?}");
}

#[test]
fn r3_finds_tokio_in_an_inner_crate() {
    let roles = &[("p", Role::Protocol), ("a", Role::AdapterLib)];
    let members = [
        member(
            "p",
            "",
            vec![
                dep("serde_json", Kind::Normal, false),
                dep("tokio", Kind::Dev, false),
            ],
        ),
        member("a", "", vec![dep("tokio", Kind::Normal, false)]),
    ];
    let findings = check_external(&members, roles, EXTERNAL_ALLOWED);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].starts_with("p -> tokio"));
}

#[test]
fn r4_finds_a_member_without_workspace_lints_and_a_changed_level() {
    let root = "[workspace.lints.rust]\nunsafe_code = \"allow\" # was deny\n\
                [workspace.lints.clippy]\ntodo = \"warn\"\n";
    let manifests = [
        (
            "a".to_owned(),
            "[package]\nname = \"a\"\n\n[lints]\nworkspace = true\n".to_owned(),
        ),
        ("b".to_owned(), "[package]\nname = \"b\"\n".to_owned()),
        (
            "c".to_owned(),
            "[lints]\nworkspace = true\n[lints.clippy]\ntodo = \"allow\"\n".to_owned(),
        ),
    ];
    let expected = &[("rust", "unsafe_code", "deny"), ("clippy", "todo", "warn")];
    let findings = check_lints(&manifests, root, expected);
    assert_eq!(findings.len(), 4, "{findings:?}");
    assert!(findings[0].starts_with("b:"));
    assert!(findings[1].starts_with("c:"));
    assert!(findings[2].contains("lacks unsafe_code = \"deny\""));
    assert!(findings[3].contains("unsafe_code = \"allow\""));
    let fixed = root.replace("\"allow\" # was deny", "\"deny\"");
    let ok = check_lints(&manifests[..1], &fixed, expected);
    assert!(ok.is_empty(), "{ok:?}");
}

#[test]
fn r5_finds_a_missing_or_short_clippy_copy() {
    let root = "allow-unwrap-in-tests = true\nallow-expect-in-tests = true\n";
    let full = format!(
        "# copy of the root\nallow-unwrap-in-tests = true\nallow-expect-in-tests = true\n\
         disallowed-methods = [{}]\n",
        CLIPPY_DISALLOWED
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let short = full
        .replace("allow-expect-in-tests = true\n", "")
        .replace("\"std::env::var_os\", ", "");
    let copies = [
        ("kernel".to_owned(), Role::Kernel, Some(full.clone())),
        ("domain".to_owned(), Role::Domain("binding"), None),
        ("domain2".to_owned(), Role::Domain("command"), Some(short)),
        ("wire".to_owned(), Role::Protocol, None),
    ];
    let findings = check_clippy(&copies, root, CLIPPY_DISALLOWED);
    assert_eq!(findings.len(), 3, "{findings:?}");
    assert!(findings[0].starts_with("domain: no clippy.toml"));
    assert!(findings[1].contains("allow-expect-in-tests"));
    assert!(findings[2].contains("std::env::var_os"));
}

#[test]
fn r6_finds_io_outside_the_adapters() {
    let files = [
        ("src/adapters/fs.rs".to_owned(), "use std::fs;".to_owned()),
        (
            "src/features/attach.rs".to_owned(),
            "let s = std::net::TcpStream::connect(a);".to_owned(),
        ),
        (
            "tests/attach.rs".to_owned(),
            "use std::process::Command;".to_owned(),
        ),
    ];
    let findings = check_module_io("binding-module", &files, MODULE_IO);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].contains("src/features/attach.rs: std::net"));
}

#[test]
fn r7_finds_source_pulled_in_by_path() {
    // Built from pieces, so this file itself pulls nothing in.
    let attribute = concat!("#", "[path = \"../src/run.rs\"]\nmod run;");
    let inner = concat!("#", "![ path=\"x\" ]");
    let include = concat!("include", " !(\"generated.rs\");");
    assert!(pulls_in_by_path(attribute));
    assert!(pulls_in_by_path(inner));
    assert!(pulls_in_by_path(include));
    assert!(!pulls_in_by_path(
        "#[pathological] include_str!(\"a\") my_include!()"
    ));

    let files = [
        ("crates/a/src/lib.rs".to_owned(), attribute.to_owned()),
        ("crates/a/src/gen.rs".to_owned(), include.to_owned()),
        (
            "crates/b/tests/old.rs".to_owned(),
            "fn main() {}".to_owned(),
        ),
    ];
    let findings = check_by_path(&files, &["crates/b/tests/old.rs"]);
    assert_eq!(findings.len(), 3, "{findings:?}");
    assert!(findings[2].contains("no longer pending"));
}

#[test]
fn r14_finds_a_new_legacy_crate_and_a_longer_list() {
    let roles = &[("gecko", Role::Legacy), ("other", Role::Legacy)];
    let findings = check_legacy(roles, &["gecko"], &["gecko"]);
    assert_eq!(findings, ["other: legacy but not on the legacy list"]);
    let findings = check_legacy(roles, &["gecko", "other"], &["gecko"]);
    assert_eq!(findings, ["other: the legacy list only shrinks"]);
    assert_eq!(check_legacy(&roles[..1], &[], &["gecko"]).len(), 1);
}

#[test]
fn the_release_check_finds_a_linked_crate_and_skips_dev_edges() {
    let mut graph = Graph::new();
    graph.insert(
        "gecko".to_owned(),
        vec![
            ("lib".to_owned(), vec![Kind::Normal]),
            ("gecko-testkit".to_owned(), vec![Kind::Dev]),
        ],
    );
    graph.insert(
        "lib".to_owned(),
        vec![("sha2".to_owned(), vec![Kind::Build])],
    );
    graph.insert(
        "gecko-testkit".to_owned(),
        vec![("gecko-dev-hub".to_owned(), vec![Kind::Normal])],
    );
    let forbidden = forbidden_in_release();
    let findings = check_release(&graph, "gecko", &forbidden);
    assert_eq!(findings, ["sha2: in the release graph of gecko"]);
    let findings = check_release(&graph, "missing", &forbidden);
    assert_eq!(findings.len(), 1, "{findings:?}");
}
