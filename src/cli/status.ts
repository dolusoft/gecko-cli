// `gecko status` (UX U1): login, binding and hub at a glance, from what is
// already at hand: the local login, the hub's status and the tab's
// `nav.where`. It never starts pairing.

import type { CapabilitiesReply } from '@dolusoft/command-protocol'

import { CliFailure } from './auth.ts'
import { readToken } from './config.ts'
import type { Result } from './output.ts'
import type { Interrupts } from './run.ts'
import { commandOf, runLine } from './run.ts'
import type { Session } from './session.ts'
import { capabilities, envOf, goneMessage, openSession } from './session.ts'

/** How long the tab is asked for where it is (UX U1: 2 s). */
export const whereWaitMs = 2000

export type Where = { title?: string; path?: string } | 'no-answer'

/**
 * Where the bound tab is, from its `nav.where`; undefined when the page has
 * no such command. The call leaves an `actor: cli` line in the tab's
 * history, an accepted cost (UX U1).
 */
export async function whereIs(
  session: Session,
  caps: CapabilitiesReply,
  interrupts: Interrupts
): Promise<Where | undefined> {
  if (!commandOf(caps, 'nav.where')) return undefined
  const result = await runLine({
    transport: session.transport,
    capabilities: caps,
    line: 'nav.where',
    io: { info: () => {} },
    interrupts,
    deadlineMs: Number(process.env.GECKO_STATUS_WAIT_MS ?? whereWaitMs)
  })
  if (result.status !== 'ok') return 'no-answer'
  const data = result.data
  if (typeof data !== 'object' || data === null) return {}
  const record = data as Record<string, unknown>
  return {
    ...(typeof record.title === 'string' ? { title: record.title } : {}),
    ...(typeof record.path === 'string' ? { path: record.path } : {})
  }
}

export interface StatusData {
  user: { id: string; name: string } | null
  env: string
  hub: { reachable: boolean; latencyMs?: number }
  binding: null | {
    state: 'active' | 'suspended' | 'gone' | 'unknown'
    reason?: string
    approvedAt?: number
    pageId?: string
    title?: string
    path?: string
    /** `ok`, `no-answer`, `busy` (another gecko call holds the hub). */
    tab?: 'ok' | 'no-answer' | 'busy'
  }
}

/** "08.11 00:20": day.month hours:minutes on this machine's clock. */
export function shortTime(ms: number): string {
  const date = new Date(ms)
  const two = (n: number) => String(n).padStart(2, '0')
  return `${two(date.getDate())}.${two(date.getMonth() + 1)} ${two(date.getHours())}:${two(date.getMinutes())}`
}

/** The three lines of the plain status. */
export function statusLines(data: StatusData): string {
  const who = data.user ? ` · ${data.user.name}` : ''
  const lines = [`ortam   ${data.env}${who}`]
  const b = data.binding
  if (b === null) {
    lines.push('sekme   yok · bağlamak için: gecko attach')
  } else if (b.tab === 'busy') {
    lines.push('sekme   bilinmiyor · başka bir gecko çağrısı sürüyor')
  } else if (b.state === 'suspended') {
    lines.push('sekme   askıda (bağlantısı düştü; 60 sn içinde dönebilir)')
  } else if (b.state === 'gone') {
    lines.push(`sekme   sona erdi: ${goneMessage(b.reason)}`)
  } else {
    const approved =
      b.approvedAt !== undefined ? ` (onay ${shortTime(b.approvedAt)})` : ''
    const place =
      b.tab === 'no-answer'
        ? ' · sekme: yanıt yok'
        : [b.title, b.path]
            .filter(Boolean)
            .map(part => ` · ${part}`)
            .join('')
    lines.push(`sekme   bağlı${approved}${place}`)
  }
  lines.push(
    data.hub.reachable
      ? `hub     ulaşılıyor${data.hub.latencyMs !== undefined ? ` · ${data.hub.latencyMs} ms` : ''}`
      : 'hub     ulaşılamıyor'
  )
  return `${lines.join('\n')}\n`
}

/**
 * Gathers the status. No login: code 6. Hub unreachable: code 4. `busy`
 * (another gecko call holds the grant's one live connection): the local
 * part and a line that says so, code 0 (UX U1).
 */
export async function gatherStatus(
  given: string | undefined,
  interrupts: Interrupts,
  capabilitiesWaitMs: number
): Promise<StatusData> {
  const env = envOf(given)
  const token = readToken(env)
  if (!token) {
    throw new CliFailure(
      'not-logged-in',
      'giriş yok: önce gecko login --env <ortam>'
    )
  }
  const started = Date.now()
  let session: Session
  try {
    session = await openSession(env)
  } catch (error) {
    if (error instanceof CliFailure && error.code === 'busy') {
      // The 4409 close is the hub's own answer: it is up.
      return {
        user: token.user,
        env,
        hub: { reachable: true },
        binding: { state: 'unknown', tab: 'busy' }
      }
    }
    throw error
  }
  const latencyMs = Date.now() - started
  try {
    const status = session.status
    const base: StatusData = {
      user: status.user,
      env,
      hub: { reachable: true, latencyMs },
      binding: null
    }
    const b = status.binding
    if (!b) return base
    base.binding = {
      state: b.state,
      ...(b.reason ? { reason: b.reason } : {}),
      approvedAt: b.approvedAt,
      pageId: b.pageId
    }
    if (b.state !== 'active') return base
    let caps: CapabilitiesReply
    try {
      caps = await capabilities(session, b, capabilitiesWaitMs)
    } catch {
      base.binding.tab = 'no-answer'
      return base
    }
    const where = await whereIs(session, caps, interrupts)
    if (where === 'no-answer') {
      base.binding.tab = 'no-answer'
    } else {
      base.binding.tab = 'ok'
      if (where?.title) base.binding.title = where.title
      if (where?.path) base.binding.path = where.path
    }
    return base
  } finally {
    await session.transport.close().catch(() => {})
  }
}

/** The status as a result: `--json` gets the fields, plain text the lines. */
export function statusResult(data: StatusData, json: boolean): Result {
  return {
    v: 1,
    status: 'ok',
    summary: json
      ? statusLines(data).trimEnd().split('\n')[1]
      : statusLines(data).trimEnd(),
    ...(json ? { data } : {}),
    preview: false,
    complete: true,
    untrusted:
      json && data.binding?.title !== undefined ? ['data.binding.title'] : []
  }
}
