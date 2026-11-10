# Environment variables and test hooks

This list is part of the contract: the tests drive the CLI and the development hub with
these variables. A new hook is added here first.

Values are milliseconds (`_MS`) or `1` (on). Without the variable, the default applies.

## User settings

The tests use these too, but they are for users; the README describes them.

| Variable           | Meaning                                                                          | Default                                                                                        |
| ------------------ | -------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| `GECKO_CONFIG_DIR` | Directory of `token.json` (the binding's token) and `config.json` (trusted hubs) | Windows `%APPDATA%\gecko-cli`, elsewhere `$XDG_CONFIG_HOME/gecko-cli` or `~/.config/gecko-cli` |

The usual proxy variables are read: `https_proxy`, `http_proxy`,
`all_proxy`, `no_proxy`, and their upper-case names; when both exist, the lower-case one
wins. HTTP and WebSocket follow the same rule. Only `http://` proxies are supported.

## CLI test hooks

| Variable                     | Meaning                                                                                                                 | Default        |
| ---------------------------- | ----------------------------------------------------------------------------------------------------------------------- | -------------- |
| `GECKO_TEST_INTERRUPT_STDIN` | `1`: each `x` written to stdin counts as a Ctrl+C. Windows cannot send SIGINT to a child process, so the tests use this | Off            |
| `GECKO_TEST_TTY`             | `1`: stdin, stdout and stderr count as terminals (colour, progress line, the trust prompt of `attach`)                  | Real TTY state |
| `GECKO_DELIVERY_MARGIN_MS`   | Margin added to the delivery window; if `received` has not come when it ends too, a read exits 4 and a change exits 5   | 2000           |
| `GECKO_CANCEL_WAIT_MS`       | How long the `cancel` answer is awaited after Ctrl+C                                                                    | 3000           |
| `GECKO_TRANSPORT`            | `dev`: the development transport is chosen even when the address is not localhost                                       | By the address |

stdin has no single owner: the Ctrl+C hook, `--line -` and `attach -` all read it. A
test uses one of them per call.

The end-to-end tests (`crates/gecko/tests/e2e.rs`) run the binary through
`gecko-testkit` (`crates/testkit`). It removes every `GECKO_*` variable from the test's
environment and keeps the rest, sets `GECKO_CONFIG_DIR` to a directory of its own and
`NO_COLOR=1`.

## Development hub hooks

| Variable                | Meaning                                                                                                                                                                                                 | Default                           |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------- |
| `GECKO_DEV_HUB_RATE`    | Rate limit: average requests per second per grant                                                                                                                                                       | 5                                 |
| `GECKO_DEV_HUB_BURST`   | Burst allowance of the rate limit                                                                                                                                                                       | 20 or the rate, whichever is more |
| `GECKO_DEV_HUB_USERS`   | User list, a JSON array (`[{ "id", "name", "terminal" }]`; `terminal: false` has no terminal access)                                                                                                    | One test user (`u1`)              |
| `GECKO_DEV_HUB_TIMINGS` | Timings as a JSON object in milliseconds: `attachTtlMs`, `suspendMs`, `unloadMs`, `holdMs`, `requestTtlMs`, `accessTtlMs`, `refreshTtlMs`, `heartbeatMs`, `heartbeatMisses` (tests shorten the windows) | The spec's timings                |
| `GECKO_DEV_HUB_STDIN`   | `1`: the hub stops when stdin closes (the tests close the process tree this way)                                                                                                                        | Off                               |

## SignalR test hub

`tests/signalr-hub/` exists only for the tests of the Rust client; it is not part of the
product.

| Variable                          | Meaning                                                                           | Default |
| --------------------------------- | --------------------------------------------------------------------------------- | ------- |
| `TEST_HUB_KEEPALIVE_MS`           | The hub's `Ping` interval (`KeepAliveInterval`)                                   | 15000   |
| `TEST_HUB_CLIENT_TIMEOUT_MS`      | Silence after which the hub drops the client (`ClientTimeoutInterval`)            | 30000   |
| `GECKO_SIGNALR_TEST_HUB`          | Hub address the client tests connect to; without it the tests are skipped         | None    |
| `GECKO_SIGNALR_TEST_HUB_REQUIRED` | `1`: the tests fail when the hub address is missing (set by the CI `signalr` job) | Off     |
