# Remote contract

The messages between a desktop CLI and the one browser tab it is bound to (ADR 0014), as of command-console 1.8.0. This document is normative: the types in `@dolusoft/command-protocol` (`src/remote.ts`, `src/outcome.ts`) implement it, and the examples in [`fixtures/remote/`](../fixtures/remote/README.md) illustrate it without replacing it. The hub between the two is opaque: it stamps the CLI's messages and routes them, and it stores the tab's capabilities without reading them; its own protocol lives in gecko-cli (`docs/hub-protocol.md`).

Every envelope is one JSON object of at most 16384 UTF-8 bytes (`envelopeMaxBytes`). Text limits count Unicode code points (a character outside the BMP, such as an emoji, is one). "Reader" below means the side that receives the message: the tab (`parseCliMessage`) for messages from the CLI, the CLI for replies from the tab.

## Common rules

| Rule                                      | Reader's behavior                                                 |
| ----------------------------------------- | ----------------------------------------------------------------- |
| An unknown field                          | Ignored. The tab leaves it out of the message it reads.           |
| `v` other than 1                          | The tab refuses `unsupported-version`. The CLI ignores the reply. |
| An unknown `kind` from the CLI            | The tab refuses it as `invalid` and stays silent.                 |
| An unknown `kind` from the tab            | The CLI ignores it.                                               |
| An unknown `danger` in the capabilities   | Read as `change`.                                                 |
| An unknown `effect` in the capabilities   | Read as `server`.                                                 |
| An unknown `finished.approval`            | Read as `none`.                                                   |
| An unknown `outcome.status`               | Read as `error`, exit code 1.                                     |
| An unknown or missing `class` on an error | Exit code 1. The CLI does not fall back on its own code table.    |

## The stamp

The hub writes these fields on every message from the CLI; the tab refuses (`invalid`) a message without them.

| Field              | Type    | Required | Limit                     | Notes                                                                                              |
| ------------------ | ------- | -------- | ------------------------- | -------------------------------------------------------------------------------------------------- |
| `origin.kind`      | `"cli"` | yes      |                           |                                                                                                    |
| `origin.grantId`   | string  | yes      | not empty                 | The CLI install.                                                                                   |
| `origin.bindingId` | string  | yes      | not empty                 | The binding of that install to one tab. The tab ignores a message whose ids are not its binding's. |
| `stampedAt`        | number  | yes      | finite, ≥ 0               | ms since the epoch, the server's clock. More than 30 s ahead of the tab: `clock-skew`.             |
| `expiresAt`        | number  | yes      | finite, after `stampedAt` | After it the tab does not run the request: `expired`.                                              |

## From the CLI

### `cli-request`

| Field  | Type             | Required              | Default                   | Limit                                                                                                                        | Unknown or wrong value                |
| ------ | ---------------- | --------------------- | ------------------------- | ---------------------------------------------------------------------------------------------------------------------------- | ------------------------------------- |
| `v`    | `1`              | yes                   |                           |                                                                                                                              | another number: `unsupported-version` |
| `kind` | `"cli-request"`  | yes                   |                           |                                                                                                                              |                                       |
| `id`   | string           | yes                   |                           | not empty                                                                                                                    | `invalid`                             |
| `argv` | string[]         | one of `argv`, `line` |                           | 1 to 256 items (`remoteArgvMaxItems`); each and all together at most 4000 code points (`remoteLineMax`); `argv[0]` not empty | `invalid`                             |
| `line` | string           | one of `argv`, `line` |                           | at most 4000 code points                                                                                                     | `invalid`                             |
| `view` | string or `null` | no                    | the view active on accept | not empty                                                                                                                    | `null` is no view; `""` is `invalid`  |

Both `argv` and `line`, or neither, is `invalid`. The CLI sends `argv` (the command name, then its arguments, as the shell split them) unless the user gave `--line`. The tab answers a refused request whose `id` and `origin` it can read with one `finished` (`invalid-args`, `class: usage`; or `unsupported-version`, `too-large`), and nothing of the request.

### `help-request`

| Field             | Type                              | Required | Default | Limit                                   | Unknown or wrong value   |
| ----------------- | --------------------------------- | -------- | ------- | --------------------------------------- | ------------------------ |
| `v`, `kind`, `id` | as above, `kind` `"help-request"` | yes      |         |                                         |                          |
| `argv`            | string[]                          | yes      |         | as `cli-request.argv`, but may be empty | `invalid`                |
| `want`            | `"text"` or `"json"`              | yes      |         |                                         | `invalid`                |
| `width`           | integer                           | no       | 80      | 20 to 400                               | outside: left out, so 80 |

`argv` is everything after `help`, raw; the tab reads it. The first item that does not start with `-` is the target; `--cursor <n>` and `--limit <n>` (also `--cursor=<n>`) page. Any other option, a second target or an option without a value is `invalid-args`. A target that names a command the CLI may run, or its alias (case-insensitive), gives that command; one that starts a namespace gives that page; no target gives the whole list. Any other target is `unknown-command` with `suggest: [nearest name]`; a sensitive name is `unknown-command` with nothing else. The CLI sends an unknown command name alone this way to get the suggestion. Answered by one `help-result`, also while a command runs, with no `received`; the stamp is checked as for `cli-request`, and an unreadable `help-request` whose `id` and `origin` can be read gets a `help-result` error.

### `cancel`

| Field                          | Type     | Required |
| ------------------------------ | -------- | -------- |
| `v`, `kind` (`"cancel"`), `id` | as above | yes      |

Cancels the request `id` if it is the tab's current run; otherwise ignored.

## From the tab

### `received`

The tab accepted the request and is about to run it.

| Field                            | Type             | Required | Notes                                                                                                                                                                                                                                                                                          |
| -------------------------------- | ---------------- | -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `v`, `kind` (`"received"`), `id` |                  | yes      |                                                                                                                                                                                                                                                                                                |
| `hidden`                         | boolean          | yes      | The page was hidden on accept.                                                                                                                                                                                                                                                                 |
| `dataAgeMs`                      | number or `null` | yes      | ≥ 0; `null` when unknown. The CLI prints a staleness line only for a number.                                                                                                                                                                                                                   |
| `change`                         | boolean          | yes      | The request runs the path that changes data: not a `read` command, not a preview. Decided from the normalized, validated input.                                                                                                                                                                |
| `viewRelevant`                   | boolean          | yes      | The CLI prints the view line. `true` when the request carries a `view`, or the command declares `usesView: true` (when the command does not set it, its namespace's `usesView`). The tab's active view alone does not make it `true`; `finished.view` still names the view the command ran in. |

### `awaiting-approval`

| Field             | Type   | Required | Notes                                               |
| ----------------- | ------ | -------- | --------------------------------------------------- |
| `v`, `kind`, `id` |        | yes      |                                                     |
| `expiresAt`       | number | yes      | ≥ 0, the tab's clock; after it the plan is dropped. |

### `chunk`

| Field             | Type    | Required | Notes                                                                     |
| ----------------- | ------- | -------- | ------------------------------------------------------------------------- |
| `v`, `kind`, `id` |         | yes      |                                                                           |
| `index`           | integer | yes      | 0-based.                                                                  |
| `total`           | integer | yes      | At most 3 (`remoteChunksMax`).                                            |
| `bytes`           | integer | yes      | The UTF-8 length of `data`.                                               |
| `data`            | string  | yes      | A slice of the JSON text of the result's `data`, cut between code points. |

Chunks come before their `finished`. A missing or repeated part, or byte counts that do not hold, make the result incomplete: the CLI prints no data and says so (`complete: false`).

### `finished`

| Field             | Type                 | Required   | Notes                                                                                                                                                                                                                                                        |
| ----------------- | -------------------- | ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `v`, `kind`, `id` |                      | yes        |                                                                                                                                                                                                                                                              |
| `outcome.status`  | `"ok"` or `"error"`  | yes        | Unknown: read as `error`.                                                                                                                                                                                                                                    |
| `outcome.summary` | string               | no         | On `ok`.                                                                                                                                                                                                                                                     |
| `outcome.code`    | `ErrorCode`          | on `error` |                                                                                                                                                                                                                                                              |
| `outcome.class`   | `ErrorClass`         | on `error` | See the table below.                                                                                                                                                                                                                                         |
| `outcome.message` | string               | no         | English; never on a sensitive line's reply.                                                                                                                                                                                                                  |
| `chunks`, `bytes` | integer              | yes        | The parts and UTF-8 bytes of `data` that came before.                                                                                                                                                                                                        |
| `view`            | `{ id, title? }`     | no         | The view the command ran in.                                                                                                                                                                                                                                 |
| `preview`         | `true`               | no         | The command ran as its preview (`--dry-run`).                                                                                                                                                                                                                |
| `approval`        | `"card"` or `"none"` | no         | How a change was let run, once its command was called: `card` after the user approved it in the tab, `none` with no card. Absent for a read, a preview and a request that did not reach its command. Unknown: read as `none`.                                |
| `suggest`         | string[]             | no         | Argv without the CLI's name: the nearest name of an `unknown-command` (never its arguments); a positional fix of an `invalid-args`; the canonical argv of a `not-allowed`, without the confirm field, except a change the user rejected. A preview has none. |
| `next`            | string[]             | no         | Argv of the next page of a paged result.                                                                                                                                                                                                                     |
| `help`            | string               | no         | On an `invalid-args`: the command whose help the CLI points to.                                                                                                                                                                                              |

`suggest` and `next` are printed by the CLI with its own quoting and its own prefix; they never run by themselves. A sensitive line's reply has no `suggest`, `next`, `help` or `message`.

### `help-result`

| Field                               | Type                | Required   | Notes                                                                                                   |
| ----------------------------------- | ------------------- | ---------- | ------------------------------------------------------------------------------------------------------- |
| `v`, `kind` (`"help-result"`), `id` |                     | yes        |                                                                                                         |
| `status`                            | `"ok"` or `"error"` | yes        | Unknown: read as `error`.                                                                               |
| `code`, `class`, `message`          |                     | on `error` | As in `finished.outcome`.                                                                               |
| `text`                              | string[]            | no         | For `want: "text"`: English lines wrapped to `width`, no colors, no CLI prefix; its JSON at most 12 KB. |
| `result`                            | any JSON            | no         | For `want: "json"`: the agent's `help` result.                                                          |
| `suggest`, `next`, `help`           |                     | no         | As in `finished`; `next` is `["help", …target, "--cursor", value]`.                                     |

At most one of `text` and `result`. `next` carries `--limit <n>` when the request gave one. A page of `text` that would pass 12 KB lists fewer commands and gives `next`; a single command that would pass it is cut, with the last line `(cut to fit; --json has the rest)`. On an error, `help` names the target command when it is one (an invalid `--cursor` or `--limit`). The layout of `text` (each line as the CLI prints it):

```
Namespaces:
  tasks (2 commands)
      Background tasks.
Commands:
  tasks.stop · change
      Stops a task.
```

```
tasks.stop · change
Stops a task.
Usage:
  tasks.stop --id <integer> [options]
Makes the change; with --dry-run it shows the preview.
Fields:
  --id <integer> · required
  --yes <boolean> · optional
```

### `cancelled-before-start`, `cancel-requested`

| Field             | Type | Required |
| ----------------- | ---- | -------- |
| `v`, `kind`, `id` |      | yes      |

`cancelled-before-start`: the command did not run. `cancel-requested`: it was running; its local work stopped, backend work it started may go on. A `finished` with `cancelled` follows.

## Capabilities

What a bound tab runs for the CLI is not an envelope. The tab calls the hub method `publishCapabilities { revision, parts }` when a binding is attached and whenever its commands change; the hub replaces its stored list with it, whole and atomically, and gives it in its `status` reply as `binding.capabilities = { revision, parts } | null`. `parts` is the JSON text of one object cut between code points (`capabilitiesParts`): one to 3 strings of at most 16384 UTF-8 bytes each, always an array. The CLI joins them and reads the object (`readCapabilitiesParts`).

| Field       | Type                                   | Required | Notes                                                                                                                                                                                                                                                                |
| ----------- | -------------------------------------- | -------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `envelopes` | integer[]                              | yes      | The envelope versions the tab speaks: `[1]` (`remoteEnvelopes`). The CLI picks the highest one both speak.                                                                                                                                                           |
| `features`  | string[]                               | yes      | See below.                                                                                                                                                                                                                                                           |
| `minCli`    | string                                 | no       | The oldest CLI version the tab supports, from the shell. Below it the CLI prints how to update and exits 2.                                                                                                                                                          |
| `revision`  | string                                 | yes      | Not empty. Changes whenever the commands or the sensitive names change.                                                                                                                                                                                              |
| `sensitive` | string[]                               | yes      | The names and aliases of sensitive commands: their lines never leave the CLI.                                                                                                                                                                                        |
| `commands`  | `{ name, aliases, danger, effect? }[]` | yes      | The commands a CLI may send: not `hidden`, not `irreversible`, with `input` and `danger`, not sensitive. `danger` is `read` or `change`; unknown: `change`. A `change` command carries `effect`: `screen` or `server`; unknown: `server`. A `read` command has none. |

`effect` is what a change touches. `screen` changes only what the user sees (a filter, a column, a view) and never opens an approval card. `server` changes data on the server and opens one when the binding's approval level is `ask-server`, the default; at `never-ask`, which the user picks in the tab when binding, it runs with no card. A command that does not declare `effect` is `server`.

The fields are written in that order, so the head is in the first part. A list that does not fit in 3 parts is an error of `attachRemote`.

### Features

| Name              | Meaning                                                                                           |
| ----------------- | ------------------------------------------------------------------------------------------------- |
| `argv`            | `cli-request` takes `argv`.                                                                       |
| `received-v2`     | `received` carries `change` and `viewRelevant`, and `dataAgeMs` only for the command it concerns. |
| `suggest-argv`    | `finished` and `help-result` carry `suggest`, `next` and `help` as argv; `outcome.line` is gone.  |
| `help-v2`         | `help-request` and `help-result`.                                                                 |
| `error-class`     | Every error carries `class`.                                                                      |
| `capabilities-v2` | The capabilities above, published in parts.                                                       |
| `effect`          | Each `change` command of the capabilities carries its `effect`.                                   |
| `dry-run`         | A change runs without its confirm field; `--dry-run` asks for its preview (see below).            |
| `approval`        | `finished` carries `approval` for a change that ran.                                              |

A tab that lacks one of the features a CLI needs makes that CLI exit 2 `unsupported-version`.

## Dry run

`--dry-run` is reserved in a request's arguments: the tab takes it out before it reads them, wherever it stands. On a `change` command with `confirm`, the command runs as its preview (the confirm field `false`) and `finished` carries `preview: true`; without `--dry-run` it runs with the confirm field `true`, so a confirm field the CLI sends (`--yes`) changes nothing. On a `change` command without `confirm` it is `invalid-args` with `help`, because there is no preview to show. On a `read` command it is dropped. A preview never opens an approval card and carries no `approval`.

## Error classes

The CLI turns an error into its exit code by `class`, never by `code`. 0 comes only from `status: "ok"`; the tab never produces 0 or 6.

| Class       | Exit code | Codes                                                                                          |
| ----------- | --------- | ---------------------------------------------------------------------------------------------- |
| `failed`    | 1         | `failed`, `timeout`, `target-not-found`, `id-conflict`                                         |
| `usage`     | 2         | `invalid-args`, `unknown-command`, `unsupported-version`, `too-large`                          |
| `denied`    | 3         | `not-allowed`                                                                                  |
| `not-run`   | 4         | `busy`, `connection-lost`, `expired`, `clock-skew`, `not-delivered`, `tab-unreachable`, `gone` |
| `unknown`   | 5         | `unknown-outcome`                                                                              |
| `cancelled` | 1         | `cancelled` (cancelled in the browser; 130 is only for Ctrl+C in the CLI)                      |

The CLI's own observation wins: `not-run` counts only for a refusal before `received`; after a `received` with `change: true` a `not-run` is 5, and a missing or contradictory end of a request that changes data (by `received.change` or by its `danger`) is 5. After `received`, the tab's `change` flag decides between exit 4 and 5; the command's `danger` is used only when no `received` arrived. `fixtures/remote/error-class.json` lists the same table.

## Command name

The same rule on both sides, for both forms:

- `argv`: the name is `argv[0]`.
- `line`: leading whitespace is skipped; the name runs to the first whitespace. Whitespace is the ECMAScript `WhiteSpace` and `LineTerminator` set: U+FEFF is in it, U+0085 is not.
- The name matches `^[A-Za-z][A-Za-z0-9._-]*$`; otherwise the CLI sends nothing and exits 2, and the tab answers `invalid-args`. A quoted name is refused.
- Names and aliases compare case-insensitively (ASCII folding), in the sensitive list and in the membership check alike.

The tab keeps three invariants; changing one is a contract change:

1. The command that runs is the first part of the request, found by its name or an alias, case-insensitively, by exact match.
2. One request runs one command: no chaining, no prefix command, no name resolved by context.
3. The name ends at the whitespace set above.

The CLI filters sensitive commands by name only. A command marked sensitive by its line content (`isSensitive`) cannot be filtered by the CLI: its line reaches the hub and the tab refuses to run it.

## Changing this contract

1. Adding is free: an optional field, a `features` name, an error code with its class and its row in the table above. A new enum value only with a defined default for readers (see "Common rules").
2. A new envelope `kind` comes with a `features` name.
3. A breaking change is a new version in `envelopes`. The old version goes on a schedule announced with `minCli`; both are spoken for at least 90 days.
4. A published fixture example is never changed or removed; examples are only added. Each release attaches `fixtures/remote/` and this document as `remote-fixtures-<version>.tar.gz`, and CI compares the folder with the latest release's archive on every pull request (`scripts/check-fixtures-frozen.mjs`).
