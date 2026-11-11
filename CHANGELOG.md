# Changelog

## Unreleased

- The TypeScript build and its tests are removed; the repository is Rust only.
- Binding with one line: **Attach CLI** in a Gecko tab gives `gecko attach <code>`,
  which binds the CLI to that tab. `gecko attach -` reads the code from stdin. The code
  works once, for 2 minutes, and carries the hub address.
- A hub address is trusted once: the CLI asks on a terminal, and `--trust-hub` trusts it
  without asking. Only `https://` hubs and `http://` on this computer are accepted.
- The approval level (`ask-server` or `never-ask`) is chosen in the tab; `gecko status`
  shows it. A change that ran without approval says so, and `--json` carries
  `approval` (`card` or `none`).
- One binding at a time; its token ends with it. Reloading the tab keeps the binding,
  and a line sent meanwhile waits for the tab up to 5 seconds.
- Removed: `gecko login`, `gecko logout`, `--env`, `GECKO_ENV_<NAME>`, `GECKO_BROWSER`,
  and the device sign-in and binding pages of the development hub (`--auto-approve`,
  `--interval`). The development hub gains `--public-url`.
- `--dry-run` and `--yes` go to the tab as typed; a preview no longer prints a line to
  confirm with. The CLI reads command-console 1.8.0 (`effect`, `approval`).
- Exit code 6 now means an invalid or expired attach code or a refused token; an ended
  binding is 4.
- On Windows the token file is yours alone also in a config directory made
  beforehand; before, such a directory could leave `SYSTEM` and `Administrators` on it.
- The development hub sends a line held during a reload only after the returning tab
  has its answer to `hello`, and logs suspensions and deliveries (request kind and id,
  never the line).

## 0.3.0

The first release, and the first Rust build. The hosted Gecko hub is not available yet:
this release works with the local development hub only.

- Binaries for Windows (x64, ARM64), Linux (x64, ARM64, static) and macOS (Apple silicon,
  Intel), with `SHA256SUMS`. `install.ps1` and `install.sh` download the binary for your
  machine, check its SHA-256 and put it on your PATH.
- The CLI is rewritten in Rust (`crates/`). It signs in, binds a tab, shows
  `gecko status`, sends lines to the tab, prints the tab's help and formats results as
  tables. It speaks command-console 1.7.
- `gecko dev-hub` runs from the Rust binary: the same hub as the Node one, checked by
  the same contract specs. Release builds leave it out.
- A narrow SignalR client for the hosted hub, tested against a small ASP.NET Core hub
  (`tests/signalr-hub/`). The hosted hub itself is not available yet.
- `--out <file>` writes the result to a file as UTF-8.
- A refused connection to a hub on the same machine is reported at once on Windows (it
  took two seconds).

## 0.2.0 (TypeScript build, not released)

- `gecko status`: the sign-in, the bound tab and the hub with its latency.
- A binding starts by itself only at a terminal; scripts and agents get exit code 4
  `no-binding` at once.
- A one-line wait indicator for calls that take longer than 700 ms.
- Suggestions for a mistyped command name; options placed after the command name are a
  usage error that shows the right order.
- Tables keep identifier columns whole, hide empty columns, and switch to a vertical
  layout when they do not fit.
- A connection that ends before `received` is exit code 4 for a read (safe to retry) and
  exit code 5 for a change.

## 0.1.0 (TypeScript build, not released)

- `gecko login`, `logout`, `attach [--new]`, `detach`, `help` and `dev-hub`; every other
  line goes to the bound tab.
- The development hub: device sign-in, CLI tokens, binding, routing, rate limiting,
  cancellation, heartbeats to browser tabs.
- The browser-side client (`@dolusoft/gecko-cli/tab-client`).
