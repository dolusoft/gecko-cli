// The development hub: §10.3 of the desktop CLI specification in one
// process, with fake users. Device login (RFC 8628), CLI tokens, the
// binding of a CLI grant to one tab page, routing with the hub's stamp and
// the browser broadcast that CLI connections never join. Development and
// tests only; it listens on localhost only.

import { randomBytes, randomUUID } from 'node:crypto'
import type { IncomingMessage, ServerResponse } from 'node:http'
import { createServer } from 'node:http'
import type { AddressInfo } from 'node:net'

import type { WebSocket } from 'ws'
import { WebSocketServer } from 'ws'

import type {
  BindingState,
  GoneReason,
  HubErrorBody,
  InvokeFrame
} from '../shared/wire.ts'
import { closeCodes, parseFrame } from '../shared/wire.ts'
import { loginPage } from './page.ts'

export interface DevUser {
  id: string
  name: string
  /** Whether the user may use the terminal. Default `true`. */
  terminal?: boolean
}

export interface DevHubTimings {
  /** Device codes live this long (§8: 10 min). */
  deviceTtlMs: number
  /** The device poll interval (RFC 8628: 5 s). */
  intervalMs: number
  /** Pairing codes live this long (§5.2: 2 min). */
  pairTtlMs: number
  /** A suspended binding waits this long for its page (§5.3: 60 s). */
  suspendMs: number
  /** `expiresAt - stampedAt` of a CLI message (§4: 30 s). */
  requestTtlMs: number
  /** Access token life (§8: 1 h). */
  accessTtlMs: number
  /** Refresh token life (§8: 7 days). */
  refreshTtlMs: number
}

export interface DevHubOptions {
  /** Default 4400; 0 picks a free port. */
  port?: number
  /** Default `127.0.0.1`. Only loopback addresses are accepted. */
  host?: string
  /** Default one user, `u1` named `zahid`. */
  users?: DevUser[]
  timings?: Partial<DevHubTimings>
  /** CLI requests per grant per second (§5.1: 5). */
  ratePerSecond?: number
  /** Wrong codes per user per minute before `too-many-attempts`. */
  maxCodeAttempts?: number
  /** Approve every device code for the first user on its first poll. */
  autoApprove?: boolean
  /** The origin of the web app for the login and pairing addresses. */
  webOrigin?: string
  /** The hub's clock. Default `Date.now`. */
  now?: () => number
  /** Lines the hub logs (never a command line). */
  log?: (line: string) => void
}

export interface DevHub {
  /** `http://127.0.0.1:<port>` */
  url: string
  port: number
  /** The environment name a CLI logs in with: `127.0.0.1:<port>`. */
  env: string
  close(): Promise<void>
  /** A copy of the hub's records, for tests. */
  snapshot(): DevHubSnapshot
}

export interface DevHubSnapshot {
  grants: { grantId: string; userId: string; revoked: boolean }[]
  bindings: {
    bindingId: string
    grantId: string
    pageId: string
    state: BindingState
    reason?: GoneReason
  }[]
  cliConnections: string[]
  browserConnections: { userId: string; pageId: string | null }[]
}

const defaults: DevHubTimings = {
  deviceTtlMs: 10 * 60_000,
  intervalMs: 5_000,
  pairTtlMs: 2 * 60_000,
  suspendMs: 60_000,
  requestTtlMs: 30_000,
  accessTtlMs: 60 * 60_000,
  refreshTtlMs: 7 * 24 * 60 * 60_000
}

const loopback = new Set(['127.0.0.1', '::1', 'localhost'])

// No 0/O, 1/I/L, 5/S, 2/Z: a code is read off a screen and typed.
const codeAlphabet = 'ABCDEFGHJKMNPQRTUVWXY346789'

function code(): string {
  const bytes = randomBytes(8)
  let text = ''
  for (let i = 0; i < 8; i++) {
    text += codeAlphabet[bytes[i] % codeAlphabet.length]
    if (i === 3) {
      text += '-'
    }
  }
  return text
}

function secret(): string {
  return randomBytes(24).toString('base64url')
}

/** A code as the user may type it: case and the dash do not matter. */
function normalizeCode(value: unknown): string {
  if (typeof value !== 'string') {
    return ''
  }
  const plain = value.toUpperCase().replace(/[^A-Z0-9]/g, '')
  return plain.length === 8 ? `${plain.slice(0, 4)}-${plain.slice(4)}` : plain
}

interface Device {
  deviceCode: string
  userCode: string
  expiresAt: number
  intervalMs: number
  lastPollAt: number | null
  status: 'pending' | 'approved' | 'denied' | 'used'
  userId?: string
}

interface Grant {
  grantId: string
  userId: string
  env: string
  createdAt: number
  revoked: boolean
  refreshToken: string
  refreshExpiresAt: number
}

interface PairCode {
  code: string
  grantId: string
  userId: string
  expiresAt: number
  used: boolean
}

interface Binding {
  bindingId: string
  grantId: string
  userId: string
  pageId: string
  browser: Conn | null
  state: BindingState
  reason?: GoneReason
  approvedAt: number
  suspendTimer: NodeJS.Timeout | null
  /** The tab's last `capabilities` reply, kept whole and unread. */
  capabilities: unknown
}

interface Conn {
  id: string
  socket: WebSocket
  kind: 'cli' | 'browser'
  userId: string
  grantId?: string
  pageId: string | null
}

class Refusal extends Error {
  readonly body: HubErrorBody
  constructor(body: HubErrorBody) {
    super(body.message ?? body.code)
    this.body = body
  }
}

const refuse = (code: string, message?: string, reason?: GoneReason): never => {
  throw new Refusal({ code, message, reason })
}

/** Starts the hub. Resolves once it listens. */
export async function startDevHub(
  options: DevHubOptions = {}
): Promise<DevHub> {
  const host = options.host ?? '127.0.0.1'
  if (!loopback.has(host)) {
    throw new Error(
      `The development hub listens on localhost only, not ${host}.`
    )
  }
  const timings: DevHubTimings = { ...defaults, ...options.timings }
  const now = options.now ?? Date.now
  const log = options.log ?? (() => {})
  const ratePerSecond = options.ratePerSecond ?? 5
  const maxCodeAttempts = options.maxCodeAttempts ?? 5
  const users = new Map(
    (options.users ?? [{ id: 'u1', name: 'zahid' }]).map(user => [
      user.id,
      user
    ])
  )
  const firstUser = [...users.keys()][0]

  const devices = new Map<string, Device>()
  const grants = new Map<string, Grant>()
  const accessTokens = new Map<string, { grantId: string; expiresAt: number }>()
  const pairCodes = new Map<string, PairCode>()
  const bindings = new Map<string, Binding>()
  const conns = new Set<Conn>()
  const cliByGrant = new Map<string, Conn>()
  const rate = new Map<string, number[]>()
  const attempts = new Map<string, number[]>()
  /** The last binding of a grant, kept after it is gone for its reason. */
  const lastBinding = new Map<string, string>()

  let env = ''
  let webOrigin = ''

  // --- helpers ---------------------------------------------------------

  const send = (conn: Conn, name: string, payload: unknown) => {
    if (conn.socket.readyState === conn.socket.OPEN) {
      conn.socket.send(JSON.stringify({ t: 'event', name, payload }))
    }
  }

  const tooManyAttempts = (userId: string): boolean => {
    const window = (attempts.get(userId) ?? []).filter(t => now() - t < 60_000)
    attempts.set(userId, window)
    return window.length >= maxCodeAttempts
  }

  const failedAttempt = (userId: string) => {
    attempts.set(userId, [...(attempts.get(userId) ?? []), now()])
  }

  const bindingOfGrant = (grantId: string): Binding | undefined => {
    const id = lastBinding.get(grantId)
    return id ? bindings.get(id) : undefined
  }

  const bindingView = (binding: Binding) => ({
    bindingId: binding.bindingId,
    state: binding.state,
    ...(binding.reason ? { reason: binding.reason } : {}),
    pageId: binding.pageId,
    approvedAt: binding.approvedAt
  })

  const announce = (binding: Binding) => {
    const cli = cliByGrant.get(binding.grantId)
    if (cli) {
      send(cli, 'binding', bindingView(binding))
    }
  }

  const end = (binding: Binding, reason: GoneReason) => {
    if (binding.state === 'gone') {
      return
    }
    if (binding.suspendTimer) {
      clearTimeout(binding.suspendTimer)
      binding.suspendTimer = null
    }
    binding.state = 'gone'
    binding.reason = reason
    if (binding.browser) {
      send(binding.browser, 'binding', {
        bindingId: binding.bindingId,
        state: 'gone',
        reason
      })
    }
    binding.browser = null
    announce(binding)
    log(`binding ${binding.bindingId} gone: ${reason}`)
  }

  const suspend = (binding: Binding) => {
    if (binding.state !== 'active') {
      return
    }
    binding.state = 'suspended'
    binding.browser = null
    binding.suspendTimer = setTimeout(() => {
      binding.suspendTimer = null
      end(binding, 'suspended-timeout')
    }, timings.suspendMs)
    announce(binding)
  }

  const revoke = (grant: Grant, reason: GoneReason) => {
    grant.revoked = true
    for (const [token, entry] of accessTokens) {
      if (entry.grantId === grant.grantId) {
        accessTokens.delete(token)
      }
    }
    for (const binding of bindings.values()) {
      if (binding.grantId === grant.grantId) {
        end(binding, reason)
      }
    }
    const cli = cliByGrant.get(grant.grantId)
    if (cli) {
      cli.socket.close(closeCodes.revoked, reason)
    }
  }

  const issueTokens = (grant: Grant) => {
    const accessToken = `cli_${secret()}`
    accessTokens.set(accessToken, {
      grantId: grant.grantId,
      expiresAt: now() + timings.accessTtlMs
    })
    return {
      access_token: accessToken,
      token_type: 'Bearer',
      expires_in: Math.round(timings.accessTtlMs / 1000),
      refresh_token: grant.refreshToken,
      refresh_expires_in: Math.round((grant.refreshExpiresAt - now()) / 1000),
      grant_id: grant.grantId,
      user: { id: grant.userId, name: users.get(grant.userId)?.name ?? '' }
    }
  }

  /** The user of a browser token: `dev:<userId>`. */
  const browserUser = (
    token: string | null | undefined
  ): DevUser | undefined => {
    if (!token?.startsWith('dev:')) {
      return undefined
    }
    return users.get(token.slice(4))
  }

  const bearer = (request: IncomingMessage): string | null => {
    const header = request.headers.authorization
    return header?.startsWith('Bearer ') ? header.slice(7) : null
  }

  // --- HTTP: device login, refresh, revoke, grants -----------------------

  const json = (response: ServerResponse, status: number, body: unknown) => {
    response.writeHead(status, {
      'content-type': 'application/json; charset=utf-8',
      'cache-control': 'no-store'
    })
    response.end(JSON.stringify(body))
  }

  const readBody = async (
    request: IncomingMessage
  ): Promise<Record<string, unknown>> => {
    const chunks: Buffer[] = []
    for await (const chunk of request) {
      chunks.push(chunk as Buffer)
    }
    const text = Buffer.concat(chunks).toString('utf8')
    if (text === '') {
      return {}
    }
    if ((request.headers['content-type'] ?? '').includes('json')) {
      try {
        const value = JSON.parse(text) as unknown
        return typeof value === 'object' && value !== null
          ? (value as Record<string, unknown>)
          : {}
      } catch {
        return {}
      }
    }
    return Object.fromEntries(new URLSearchParams(text))
  }

  const http = async (request: IncomingMessage, response: ServerResponse) => {
    const url = new URL(request.url ?? '/', 'http://localhost')
    const path = url.pathname
    if (request.method === 'GET' && path === '/health') {
      return json(response, 200, { ok: true, env })
    }
    if (request.method === 'GET' && path === '/cli-login') {
      response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' })
      return response.end(loginPage(firstUser ?? 'u1'))
    }
    if (
      request.method !== 'POST' &&
      !(request.method === 'GET' && path === '/cli/grants')
    ) {
      return json(response, 404, { error: 'not_found' })
    }
    const body = request.method === 'POST' ? await readBody(request) : {}

    if (path === '/oauth/device_authorization') {
      const device: Device = {
        deviceCode: secret(),
        userCode: code(),
        expiresAt: now() + timings.deviceTtlMs,
        intervalMs: timings.intervalMs,
        lastPollAt: null,
        status: 'pending'
      }
      devices.set(device.deviceCode, device)
      return json(response, 200, {
        device_code: device.deviceCode,
        user_code: device.userCode,
        verification_uri: `${webOrigin}/cli-login`,
        verification_uri_complete: `${webOrigin}/cli-login#${device.userCode}`,
        expires_in: Math.round(timings.deviceTtlMs / 1000),
        interval: timings.intervalMs / 1000
      })
    }

    if (path === '/oauth/token') {
      const grantType = body.grant_type
      if (grantType === 'urn:ietf:params:oauth:grant-type:device_code') {
        const device = devices.get(String(body.device_code ?? ''))
        if (!device || device.status === 'used') {
          return json(response, 400, { error: 'invalid_grant' })
        }
        if (now() >= device.expiresAt) {
          devices.delete(device.deviceCode)
          return json(response, 400, { error: 'expired_token' })
        }
        if (
          device.lastPollAt !== null &&
          now() - device.lastPollAt < device.intervalMs
        ) {
          // RFC 8628 §3.5: the interval grows by 5 s on every slow_down.
          device.intervalMs += Math.max(timings.intervalMs, 1)
          device.lastPollAt = now()
          return json(response, 400, {
            error: 'slow_down',
            interval: device.intervalMs / 1000
          })
        }
        device.lastPollAt = now()
        if (device.status === 'pending' && options.autoApprove && firstUser) {
          device.status = 'approved'
          device.userId = firstUser
        }
        if (device.status === 'pending') {
          return json(response, 400, { error: 'authorization_pending' })
        }
        if (device.status === 'denied') {
          devices.delete(device.deviceCode)
          return json(response, 400, { error: 'access_denied' })
        }
        device.status = 'used'
        const grant: Grant = {
          grantId: `g_${randomUUID()}`,
          userId: device.userId!,
          env,
          createdAt: now(),
          revoked: false,
          refreshToken: `cr_${secret()}`,
          refreshExpiresAt: now() + timings.refreshTtlMs
        }
        grants.set(grant.grantId, grant)
        log(`grant ${grant.grantId} for ${grant.userId}`)
        return json(response, 200, issueTokens(grant))
      }
      if (grantType === 'refresh_token') {
        const grant = [...grants.values()].find(
          g => g.refreshToken === body.refresh_token
        )
        if (!grant || grant.revoked || now() >= grant.refreshExpiresAt) {
          return json(response, 400, { error: 'invalid_grant' })
        }
        // The grant id stays; the binding does not drop (§3).
        return json(response, 200, issueTokens(grant))
      }
      return json(response, 400, { error: 'unsupported_grant_type' })
    }

    if (path === '/oauth/revoke') {
      const grant = [...grants.values()].find(
        g => g.refreshToken === body.token
      )
      if (grant && !grant.revoked) {
        revoke(grant, 'logged-out')
      }
      // RFC 7009: 200 whether or not the token was known.
      return json(response, 200, {})
    }

    // The browser side: the fake user's session is `Bearer dev:<userId>`.
    const user = browserUser(bearer(request))
    if (!user) {
      return json(response, 401, { error: 'unauthorized' })
    }

    if (path === '/oauth/device/verify') {
      if (tooManyAttempts(user.id)) {
        return json(response, 429, { error: 'too-many-attempts' })
      }
      const userCode = normalizeCode(body.user_code)
      const device = [...devices.values()].find(
        d => d.userCode === userCode && d.status === 'pending'
      )
      if (!device || now() >= device.expiresAt) {
        failedAttempt(user.id)
        return json(response, 400, { error: 'invalid-code' })
      }
      const approve = body.approve !== false && body.approve !== 'false'
      device.status = approve ? 'approved' : 'denied'
      device.userId = user.id
      return json(response, 200, { ok: true })
    }

    if (path === '/cli/grants' && request.method === 'GET') {
      return json(
        response,
        200,
        [...grants.values()]
          .filter(g => g.userId === user.id && !g.revoked)
          .map(g => ({
            grantId: g.grantId,
            createdAt: g.createdAt,
            env: g.env
          }))
      )
    }

    const revokeMatch = /^\/cli\/grants\/([^/]+)\/revoke$/.exec(path)
    if (revokeMatch) {
      const grant = grants.get(decodeURIComponent(revokeMatch[1]))
      if (!grant || grant.userId !== user.id) {
        return json(response, 404, { error: 'not_found' })
      }
      revoke(grant, 'revoked')
      return json(response, 200, { ok: true })
    }

    return json(response, 404, { error: 'not_found' })
  }

  // --- WebSocket: methods --------------------------------------------------

  const cliMethods: Record<
    string,
    (conn: Conn, args: Record<string, unknown>) => unknown
  > = {
    status(conn) {
      const grant = grants.get(conn.grantId!)!
      const binding = bindingOfGrant(conn.grantId!)
      return {
        user: { id: grant.userId, name: users.get(grant.userId)?.name ?? '' },
        env,
        grantId: grant.grantId,
        binding: binding
          ? {
              ...bindingView(binding),
              ...(binding.capabilities !== undefined && binding.state !== 'gone'
                ? { capabilities: binding.capabilities }
                : {})
            }
          : null
      }
    },
    pair(conn) {
      // An older code of the grant stops working.
      for (const entry of pairCodes.values()) {
        if (entry.grantId === conn.grantId) {
          entry.used = true
        }
      }
      const entry: PairCode = {
        code: code(),
        grantId: conn.grantId!,
        userId: conn.userId,
        expiresAt: now() + timings.pairTtlMs,
        used: false
      }
      pairCodes.set(entry.code, entry)
      return {
        code: entry.code,
        expiresAt: entry.expiresAt,
        url: `${webOrigin}/cli-pair#${entry.code}`
      }
    },
    send(conn, args) {
      const envelope = args.envelope
      if (
        typeof envelope !== 'object' ||
        envelope === null ||
        Array.isArray(envelope)
      ) {
        return refuse('invalid')
      }
      // The hub reads `kind` and the request id only (§2).
      const { kind, id } = envelope as Record<string, unknown>
      if (
        (kind !== 'cli-request' &&
          kind !== 'cancel' &&
          kind !== 'help-request') ||
        typeof id !== 'string'
      ) {
        return refuse('invalid')
      }
      const binding = bindingOfGrant(conn.grantId!)
      if (!binding) {
        return refuse('no-binding', 'no binding for this grant')
      }
      if (binding.state === 'gone') {
        return refuse('gone', 'the binding ended', binding.reason)
      }
      if (binding.state === 'suspended' || !binding.browser) {
        return refuse('tab-unreachable', 'the bound tab is not connected')
      }
      if (kind === 'cli-request' || kind === 'help-request') {
        const window = (rate.get(conn.grantId!) ?? []).filter(
          t => now() - t < 1000
        )
        if (window.length >= ratePerSecond) {
          rate.set(conn.grantId!, window)
          return refuse('busy', 'rate limit: at most 5 requests a second')
        }
        window.push(now())
        rate.set(conn.grantId!, window)
      }
      const stampedAt = now()
      const expiresAt = stampedAt + timings.requestTtlMs
      // The client's own copy of the stamp is dropped, then written anew.
      const stamped = { ...(envelope as Record<string, unknown>) }
      delete stamped.origin
      delete stamped.stampedAt
      delete stamped.expiresAt
      send(binding.browser, 'cli-message', {
        ...stamped,
        origin: {
          kind: 'cli',
          grantId: conn.grantId,
          bindingId: binding.bindingId
        },
        stampedAt,
        expiresAt
      })
      return { stampedAt, expiresAt }
    },
    detach(conn) {
      const binding = bindingOfGrant(conn.grantId!)
      if (binding) {
        end(binding, 'detached')
      }
      return { ok: true }
    }
  }

  const bindingOfBrowser = (conn: Conn): Binding | undefined =>
    [...bindings.values()].find(b => b.browser === conn && b.state === 'active')

  const pairCheck = (conn: Conn, args: Record<string, unknown>): PairCode => {
    if (tooManyAttempts(conn.userId)) {
      return refuse('too-many-attempts', 'too many wrong codes; wait a minute')
    }
    const entry = pairCodes.get(normalizeCode(args.code))
    if (!entry || entry.used || now() >= entry.expiresAt) {
      failedAttempt(conn.userId)
      return refuse('invalid-code', 'the code is wrong, used or expired')
    }
    if (entry.userId !== conn.userId) {
      failedAttempt(conn.userId)
      // The CLI keeps waiting and says another account is signed in (§5.5).
      const cli = cliByGrant.get(entry.grantId)
      if (cli) {
        send(cli, 'pair-refused', { code: 'other-account' })
      }
      return refuse('other-account', 'the code belongs to another account')
    }
    if (users.get(conn.userId)?.terminal === false) {
      // Not consumed: the CLI hears it and ends with code 3 (§5.2).
      const cli = cliByGrant.get(entry.grantId)
      if (cli) {
        send(cli, 'pair-refused', { code: 'no-terminal-access' })
      }
      return refuse('no-terminal-access', 'this account has no terminal access')
    }
    const grant = grants.get(entry.grantId)
    if (!grant || grant.revoked) {
      return refuse('invalid-code', 'the grant was revoked')
    }
    return entry
  }

  const browserMethods: Record<
    string,
    (conn: Conn, args: Record<string, unknown>) => unknown
  > = {
    hello(conn, args) {
      const pageId = args.pageId
      if (typeof pageId !== 'string' || pageId === '') {
        return refuse('invalid', 'pageId')
      }
      if (conn.pageId !== null && conn.pageId !== pageId) {
        // A new page on the same connection: the old page is gone.
        const old = bindingOfBrowser(conn)
        if (old) {
          end(old, 'closed')
        }
      }
      conn.pageId = pageId
      const binding = [...bindings.values()].find(
        b =>
          b.pageId === pageId &&
          b.userId === conn.userId &&
          (b.state === 'suspended' ||
            (b.state === 'active' && b.browser !== conn))
      )
      if (binding) {
        // The page came back within the suspension time (§5.3).
        if (binding.suspendTimer) {
          clearTimeout(binding.suspendTimer)
          binding.suspendTimer = null
        }
        binding.browser = conn
        binding.state = 'active'
        announce(binding)
      }
      const live = bindingOfBrowser(conn)
      return {
        binding: live
          ? {
              bindingId: live.bindingId,
              grantId: live.grantId,
              approvedAt: live.approvedAt
            }
          : null
      }
    },
    pairInfo(conn, args) {
      const entry = pairCheck(conn, args)
      const grant = grants.get(entry.grantId)!
      // Server facts only; never a name the CLI declares (§5.2).
      return { grantCreatedAt: grant.createdAt, env: grant.env }
    },
    pair(conn, args) {
      if (conn.pageId === null) {
        return refuse('invalid', 'say hello with the pageId first')
      }
      const entry = pairCheck(conn, args)
      // Atomic: the check and the use run in one turn of the event loop.
      entry.used = true
      const old = bindingOfBrowser(conn)
      if (old) {
        end(old, 'replaced')
      }
      const previous = bindingOfGrant(entry.grantId)
      if (previous) {
        end(previous, 'replaced')
      }
      const binding: Binding = {
        bindingId: `b_${randomUUID()}`,
        grantId: entry.grantId,
        userId: conn.userId,
        pageId: conn.pageId,
        browser: conn,
        state: 'active',
        approvedAt: now(),
        suspendTimer: null,
        capabilities: undefined
      }
      bindings.set(binding.bindingId, binding)
      lastBinding.set(entry.grantId, binding.bindingId)
      const cli = cliByGrant.get(entry.grantId)
      if (cli) {
        send(cli, 'paired', bindingView(binding))
      }
      log(`binding ${binding.bindingId} active`)
      return {
        bindingId: binding.bindingId,
        grantId: binding.grantId,
        approvedAt: binding.approvedAt
      }
    },
    sendToCli(conn, args) {
      const binding = bindingOfBrowser(conn)
      const reply = args.reply
      if (!binding) {
        return refuse('not-bound', 'this connection has no binding')
      }
      if (typeof reply !== 'object' || reply === null) {
        return refuse('invalid')
      }
      // The hub reads `kind` only; the capabilities are kept whole for the
      // next CLI connection, as each `gecko` call is a new connection.
      if ((reply as Record<string, unknown>).kind === 'capabilities') {
        binding.capabilities = reply
      }
      const cli = cliByGrant.get(binding.grantId)
      if (cli) {
        send(cli, 'reply', reply)
        return { delivered: true }
      }
      return { delivered: false }
    },
    detach(conn) {
      const binding = bindingOfBrowser(conn)
      if (binding) {
        end(binding, 'detached')
      }
      return { ok: true }
    },
    unload(conn) {
      // The page is closing or reloading.
      const binding = bindingOfBrowser(conn)
      if (binding) {
        end(binding, 'closed')
      }
      conn.pageId = null
      return { ok: true }
    },
    logout(conn) {
      for (const binding of bindings.values()) {
        if (binding.userId === conn.userId) {
          end(binding, 'logged-out')
        }
      }
      return { ok: true }
    },
    broadcast(conn, args) {
      // Today's `Send`: every other browser connection of the user. A CLI
      // connection never joins (§5.1).
      let count = 0
      for (const other of conns) {
        if (
          other !== conn &&
          other.kind === 'browser' &&
          other.userId === conn.userId
        ) {
          send(other, 'broadcast', args.envelope)
          count++
        }
      }
      return { delivered: count }
    }
  }

  const onInvoke = (conn: Conn, frame: InvokeFrame) => {
    const table = conn.kind === 'cli' ? cliMethods : browserMethods
    const method = Object.hasOwn(table, frame.method)
      ? table[frame.method]
      : undefined
    const answer = (body: unknown) => {
      if (conn.socket.readyState === conn.socket.OPEN) {
        conn.socket.send(JSON.stringify(body))
      }
    }
    if (!method) {
      return answer({
        t: 'result',
        id: frame.id,
        ok: false,
        error: { code: 'unknown-method', message: frame.method }
      })
    }
    try {
      const args =
        typeof frame.args === 'object' && frame.args !== null
          ? (frame.args as Record<string, unknown>)
          : {}
      const value = method(conn, args)
      answer({ t: 'result', id: frame.id, ok: true, value })
    } catch (error) {
      const body =
        error instanceof Refusal
          ? error.body
          : { code: 'failed', message: String(error) }
      answer({ t: 'result', id: frame.id, ok: false, error: body })
    }
  }

  // --- connections -----------------------------------------------------------

  const server = createServer((request, response) => {
    http(request, response).catch(error => {
      log(`http error: ${String(error)}`)
      if (!response.headersSent) {
        json(response, 500, { error: 'server_error' })
      }
    })
  })
  const wss = new WebSocketServer({ noServer: true, maxPayload: 64 * 1024 })

  server.on('upgrade', (request, socket, head) => {
    const url = new URL(request.url ?? '/', 'http://localhost')
    if (url.pathname !== '/hub') {
      socket.destroy()
      return
    }
    wss.handleUpgrade(request, socket, head, ws => accept(ws, request, url))
  })

  const accept = (socket: WebSocket, request: IncomingMessage, url: URL) => {
    const header = bearer(request)
    const query = url.searchParams.get('access_token')
    let conn: Conn
    if (query?.startsWith('cli_')) {
      // §8: the query string path is closed for a CLI token.
      socket.close(closeCodes.forbidden, 'cli token in the query string')
      return
    }
    if (header?.startsWith('cli_')) {
      const entry = accessTokens.get(header)
      const grant = entry && grants.get(entry.grantId)
      if (!entry || !grant || grant.revoked || now() >= entry.expiresAt) {
        socket.close(closeCodes.unauthorized, 'token')
        return
      }
      if (cliByGrant.has(grant.grantId)) {
        // One live CLI connection per grant (§3); the tab sees the attempt.
        const binding = bindingOfGrant(grant.grantId)
        if (binding?.browser) {
          send(binding.browser, 'cli-busy', {
            grantId: grant.grantId,
            at: now()
          })
        }
        socket.close(closeCodes.busy, 'another CLI call is running')
        return
      }
      conn = {
        id: randomUUID(),
        socket,
        kind: 'cli',
        userId: grant.userId,
        grantId: grant.grantId,
        pageId: null
      }
      cliByGrant.set(grant.grantId, conn)
    } else {
      const user = browserUser(query ?? header)
      if (!user) {
        socket.close(closeCodes.unauthorized, 'session')
        return
      }
      conn = {
        id: randomUUID(),
        socket,
        kind: 'browser',
        userId: user.id,
        pageId: null
      }
    }
    conns.add(conn)
    socket.on('message', data => {
      const frame = parseFrame(String(data))
      if (frame?.t === 'invoke') {
        onInvoke(conn, frame)
      }
    })
    socket.on('close', () => {
      conns.delete(conn)
      if (conn.kind === 'cli') {
        if (cliByGrant.get(conn.grantId!) === conn) {
          cliByGrant.delete(conn.grantId!)
        }
        return
      }
      for (const binding of bindings.values()) {
        if (binding.browser === conn) {
          suspend(binding)
        }
      }
    })
    socket.send(
      JSON.stringify({
        t: 'event',
        name: 'ready',
        payload: { kind: conn.kind }
      })
    )
  }

  await new Promise<void>((resolve, reject) => {
    server.once('error', reject)
    server.listen(options.port ?? 4400, host, () => resolve())
  })
  const port = (server.address() as AddressInfo).port
  const hostPart = host === '::1' ? '[::1]' : host
  env = `${hostPart}:${port}`
  const url = `http://${env}`
  webOrigin = options.webOrigin ?? url
  log(`dev hub listening on ${url}`)

  return {
    url,
    port,
    env,
    snapshot() {
      return {
        grants: [...grants.values()].map(g => ({
          grantId: g.grantId,
          userId: g.userId,
          revoked: g.revoked
        })),
        bindings: [...bindings.values()].map(b => ({
          bindingId: b.bindingId,
          grantId: b.grantId,
          pageId: b.pageId,
          state: b.state,
          ...(b.reason ? { reason: b.reason } : {})
        })),
        cliConnections: [...cliByGrant.keys()],
        browserConnections: [...conns]
          .filter(c => c.kind === 'browser')
          .map(c => ({ userId: c.userId, pageId: c.pageId }))
      }
    },
    close() {
      for (const binding of bindings.values()) {
        if (binding.suspendTimer) {
          clearTimeout(binding.suspendTimer)
        }
      }
      for (const conn of conns) {
        conn.socket.terminate()
      }
      return new Promise<void>(resolve => {
        wss.close()
        server.close(() => resolve())
        server.closeAllConnections()
      })
    }
  }
}
