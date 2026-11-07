// The frames of the development transport: plain JSON over one WebSocket.
// It mirrors the shape of a SignalR hub (invocations with a completion, and
// server events), so the CLI and the tab speak to the hub through methods
// and events only; the real hub swaps the framing, not the calls.

/** A call from a client: `method` with `args`; answered by a `result`. */
export interface InvokeFrame {
  t: 'invoke'
  id: number
  method: string
  args?: unknown
}

/** The answer to an `invoke`. */
export type ResultFrame =
  | { t: 'result'; id: number; ok: true; value: unknown }
  | { t: 'result'; id: number; ok: false; error: HubErrorBody }

/** A message the hub pushes to a client. */
export interface EventFrame {
  t: 'event'
  name: string
  payload: unknown
}

export type Frame = InvokeFrame | ResultFrame | EventFrame

/** Why the hub refused a call. */
export interface HubErrorBody {
  code: string
  message?: string
  /** `gone` only: why the binding ended. */
  reason?: GoneReason
  /** `rate-limited` only: when the grant's bucket holds a request again. */
  retryAfterMs?: number
}

/** Why a binding ended (§5.3). */
export type GoneReason =
  | 'replaced'
  | 'closed'
  | 'detached'
  | 'logged-out'
  | 'revoked'
  | 'suspended-timeout'

/** The state of a binding (§5.3). */
export type BindingState = 'active' | 'suspended' | 'gone'

/**
 * WebSocket close codes the hub uses when it ends or refuses a connection.
 * The client reads them on `close`.
 */
export const closeCodes = {
  /** No token, a wrong or expired token, or a revoked grant. */
  unauthorized: 4401,
  /** A CLI token in the query string: the CLI sends it as a header. */
  forbidden: 4403,
  /** Another live CLI connection holds the grant (§3). */
  busy: 4409,
  /** The grant was revoked while connected. */
  revoked: 4410
} as const

export function parseFrame(text: string): Frame | undefined {
  let value: unknown
  try {
    value = JSON.parse(text)
  } catch {
    return undefined
  }
  if (typeof value !== 'object' || value === null) {
    return undefined
  }
  const frame = value as Record<string, unknown>
  if (frame.t === 'invoke') {
    return typeof frame.id === 'number' && typeof frame.method === 'string'
      ? (frame as unknown as InvokeFrame)
      : undefined
  }
  if (frame.t === 'result') {
    return typeof frame.id === 'number'
      ? (frame as unknown as ResultFrame)
      : undefined
  }
  if (frame.t === 'event') {
    return typeof frame.name === 'string'
      ? (frame as unknown as EventFrame)
      : undefined
  }
  return undefined
}

/** The input of command-console-core's agent `help` tool. */
export interface HelpInput {
  command?: string
  prefix?: string
  cursor?: string
  limit?: number
}

/**
 * The CLI asks the bound tab for its help (`gecko help [name]`). The tab's
 * own `help` command is not open to the CLI; the answer is the output of
 * command-console-core's agent `help` tool, permission filtered and paged.
 * The hub stamps and routes it as it does a `cli-request`.
 */
export interface HelpRequest {
  v: 1
  kind: 'help-request'
  id: string
  input: HelpInput
}

/** The tab's answer to a `help-request`, sent with `sendToCli`. */
export interface HelpResult {
  v: 1
  kind: 'help-result'
  id: string
  /**
   * The `help` tool's output: a page `{ namespaces, commands, nextCursor? }`,
   * one command `{ name, summary, usage, danger, confirm?, input }`, or
   * `{ status: 'error', code, message? }`.
   */
  result: unknown
}

export function isHelpResult(value: unknown): value is HelpResult {
  if (typeof value !== 'object' || value === null) return false
  const record = value as Record<string, unknown>
  return (
    record.v === 1 &&
    record.kind === 'help-result' &&
    typeof record.id === 'string' &&
    'result' in record
  )
}

/** A stamped `help-request` as the tab hears it, or `undefined`. */
export function readHelpRequest(raw: unknown):
  | (HelpRequest & {
      origin: { kind: 'cli'; grantId: string; bindingId: string }
      stampedAt: number
      expiresAt: number
    })
  | undefined {
  if (typeof raw !== 'object' || raw === null) return undefined
  const record = raw as Record<string, unknown>
  const origin = record.origin as Record<string, unknown> | undefined
  if (
    record.v !== 1 ||
    record.kind !== 'help-request' ||
    typeof record.id !== 'string' ||
    typeof record.expiresAt !== 'number' ||
    typeof record.stampedAt !== 'number' ||
    typeof origin !== 'object' ||
    origin === null ||
    origin.kind !== 'cli' ||
    typeof origin.bindingId !== 'string'
  ) {
    return undefined
  }
  const input =
    typeof record.input === 'object' &&
    record.input !== null &&
    !Array.isArray(record.input)
      ? (record.input as HelpInput)
      : {}
  return { ...record, input } as never
}
