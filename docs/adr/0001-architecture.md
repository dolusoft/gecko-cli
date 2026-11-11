# ADR 0001: Architecture

Status: accepted. Reference: rust-kit `rust-ddd-scaffold` (see
"Enforcement").

## Context

`gecko` is a small CLI: it binds to a browser tab through a hub and sends it
command lines. It is changed mostly by coding agents. Until 0.3 it was one
crate in which binding, line sending, output and configuration were
interwoven; the rules (a core without I/O, a direction of dependencies) held
only by habit. The structure therefore has to fail a build or a test when it
is broken, without anyone reading a document.

## Decisions

1. **One binary, crates by role.** Every workspace crate has exactly one role.
   Crate boundaries make the dependency direction a compiler fact.

   | Role           | Crates                                             | May depend on                                                             |
   | -------------- | -------------------------------------------------- | ------------------------------------------------------------------------- |
   | `kernel`       | `gecko-kernel`                                     | allowed external crates only (`serde`, `serde_json`)                      |
   | `protocol`     | `gecko-wire`, `gecko-hub-protocol`                 | `kernel`                                                                  |
   | `contract`     | `gecko-command-contract`, `gecko-binding-contract` | `kernel`, `protocol`                                                      |
   | `domain`       | `gecko-command-domain`, `gecko-binding-domain`     | its own `contract`, `kernel`, `protocol`                                  |
   | `module`       | `gecko-command-module`, `gecko-binding-module`     | its own `domain`, any `contract`, `kernel`, `protocol`, `adapter-lib`     |
   | `adapter-lib`  | `gecko-hub-client`                                 | `kernel`, `protocol`                                                      |
   | `dev-tool`     | `gecko-dev-hub`                                    | no workspace crate; only `app` (behind a feature) and test support use it |
   | `test-support` | `gecko-testkit`                                    | anything; appears only among dev-dependencies                             |
   | `app`          | `gecko` (the binary)                               | anything; the composition root                                            |

   The inner roles (`kernel`, `protocol`, `contract`, `domain`) use no other
   external crate, so `tokio`, `reqwest` and the WebSocket stack stay out of
   them. A module is not allowed to depend on another module.

2. **Bounded contexts.** `command` (a line for the tab, `gecko help`) and
   `binding` (`attach`, `detach`, `status`, the token and its store). Each is
   three crates: `contract` (ports and the values a call hands in), `domain`
   (rules, no I/O, no clock) and `module` (slices under `features/`, I/O only
   under `src/adapters/`).

3. **Domain and kernel without infrastructure.** Time and identity come in as
   values or ports. A `clippy.toml` in each such crate bans the clock, the
   environment, the standard streams, threads, files, sockets, processes and
   the random hasher.

4. **Ports.** Slices reach the world through traits; the binary implements
   them and passes them in.

   | Port            | Defined in               | Implemented by                                     |
   | --------------- | ------------------------ | -------------------------------------------------- |
   | `HubConnection` | `gecko-hub-protocol`     | `gecko-hub-client` (`DevTransport`)                |
   | `Reporter`      | `gecko-kernel`           | `app` (`Io`: stderr lines, prompt, progress, JSON) |
   | `Interrupts`    | `gecko-kernel`           | `app` (Ctrl+C)                                     |
   | `Clock`         | `gecko-kernel`           | `app` (`SystemClock`)                              |
   | `IdSource`      | `gecko-kernel`           | `app` (`RandomIds`)                                |
   | `TokenStore`    | `gecko-binding-contract` | `gecko-binding-module` `adapters/fs` (`Config`)    |
   | `TrustPrompt`   | `gecko-binding-contract` | `app` (one line from stdin)                        |
   | `HubOpener`     | `gecko-binding-contract` | `app` (opens the connection, refreshes the token)  |

   `HubConnection` is used generically (`H: HubConnection`), not as `dyn`: a
   call opens one connection, and a second transport becomes an `enum`.
   `Settings` (the environment the CLI reads) is a value in `gecko-kernel`; the
   binary reads the environment once, in `main.rs`, and nowhere else.

5. **No `session` context.** Opening a connection holds no rule of its own:
   it is composition (token, transport, one refresh on `401`) and belongs to
   the binary. What a closed connection means as a failure is a mapping of
   the hub contract and lives in `gecko-hub-protocol`; the open connection
   with the hub's status is the value `Session<H>` there.

6. **`gecko-dev-hub` is one crate.** The development hub is a tool, not the
   product: it is not in release builds (`--no-default-features`) and talks
   to the CLI only over the wire. Splitting its rules into pure functions
   would be a rewrite with no user benefit. It may be split later as a
   separate piece of work.

## Enforcement

Interim: `crates/app/tests/architecture.rs` checks, as pure functions over
`cargo metadata`, manifest text and source text, each with a unit test that
feeds it a broken input:

- every member has a role and every manifest under `crates/` is a member;
- the dependency matrix above, dev-dependencies included;
- the external crates of the inner roles;
- `[lints] workspace = true` everywhere and the workspace lint levels;
- the `clippy.toml` copies carry the root's lines and the bans;
- I/O in a module only under `src/adapters/`;
- no `#[path]` or `include!`;
- the legacy list (empty) only shrinks;
- the release graph (`--no-default-features`) contains neither the dev hub,
  nor `sha2`, nor test support.

The architecture test, every `clippy.toml` and the root lint tables are gate
files: a change to them is a pull request of its own, with the reason in its
description.

rust-kit's own checks (`cargo xtask arch`) are not part of this repository:
the maintainers run them on a copy of it, outside. This test stays the
check every contributor runs; it is not deleted when the outside checks
grow. A finding there is fixed here like any other review comment.

`deny.toml` is a gate file too: licences, advisories and sources, checked by
`cargo deny` in CI.

## Limits

- The checks on source text (I/O placement, `#[path]`) can be evaded by
  macros; the hard line is the external-crate list and Clippy.
- The checks run in CI; enforcement at merge time needs branch protection,
  which is a remote setting.

## Consequences

A new command is a slice in a context's `module`, with its rules in that
context's `domain`. A dependency across a boundary, I/O in a domain, or a
crate without a role is a red test with the rule named in the message.
