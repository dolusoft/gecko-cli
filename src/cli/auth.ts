// Device login (RFC 8628, §8), token refresh and revocation, over HTTP.

import { isLocalHost } from '../transport/dev.ts'
import type { StoredToken } from './config.ts'
import { readToken, writeToken } from './config.ts'

export class CliFailure extends Error {
  readonly code: string
  constructor(code: string, message: string) {
    super(message)
    this.name = 'CliFailure'
    this.code = code
  }
}

/** The host of an environment: a host name as given, or a named alias. */
export function resolveEnv(name: string): string {
  if (/[.:]/.test(name) || name === 'localhost') {
    return name
  }
  const alias = process.env[`GECKO_ENV_${name.toUpperCase()}`]
  if (alias) {
    return alias
  }
  throw new CliFailure(
    'usage',
    `bilinmeyen ortam: ${name}. Adresi verin (--env develop.ornek.com) ya da GECKO_ENV_${name.toUpperCase()} ile tanımlayın.`
  )
}

export function baseUrl(host: string): string {
  return `${isLocalHost(host) ? 'http' : 'https'}://${host}`
}

interface TokenResponse {
  access_token: string
  expires_in: number
  refresh_token: string
  grant_id: string
  user: { id: string; name: string }
}

async function post(
  url: string,
  body: Record<string, string>
): Promise<{ status: number; json: Record<string, unknown> }> {
  let response: Response
  try {
    response = await fetch(url, {
      method: 'POST',
      headers: { 'content-type': 'application/x-www-form-urlencoded' },
      body: new URLSearchParams(body)
    })
  } catch {
    throw new CliFailure('unreachable', `sunucuya ulaşılamadı: ${url}`)
  }
  const json = (await response.json().catch(() => ({}))) as Record<
    string,
    unknown
  >
  return { status: response.status, json }
}

function stored(env: string, response: TokenResponse): StoredToken {
  return {
    env,
    accessToken: response.access_token,
    accessExpiresAt: Date.now() + response.expires_in * 1000,
    refreshToken: response.refresh_token,
    grantId: response.grant_id,
    user: response.user
  }
}

export interface DeviceStart {
  userCode: string
  verificationUri: string
  verificationUriComplete: string
}

/**
 * The device flow: asks for codes, lets the caller show them, then polls
 * at the server's interval, slowing down when told (RFC 8628 §3.5).
 */
export async function deviceLogin(
  env: string,
  show: (start: DeviceStart) => void,
  signal: AbortSignal
): Promise<StoredToken> {
  const host = resolveEnv(env)
  const base = baseUrl(host)
  const start = await post(`${base}/oauth/device_authorization`, {
    client_id: 'gecko-cli'
  })
  if (start.status !== 200) {
    throw new CliFailure('failed', 'giriş başlatılamadı')
  }
  const deviceCode = String(start.json.device_code)
  let intervalMs = Number(start.json.interval ?? 5) * 1000
  const deadline = Date.now() + Number(start.json.expires_in ?? 600) * 1000
  show({
    userCode: String(start.json.user_code),
    verificationUri: String(start.json.verification_uri),
    verificationUriComplete: String(
      start.json.verification_uri_complete ?? start.json.verification_uri
    )
  })
  while (Date.now() < deadline) {
    await sleep(intervalMs, signal)
    const poll = await post(`${base}/oauth/token`, {
      grant_type: 'urn:ietf:params:oauth:grant-type:device_code',
      device_code: deviceCode,
      client_id: 'gecko-cli'
    })
    if (poll.status === 200) {
      const token = stored(env, poll.json as unknown as TokenResponse)
      writeToken(token)
      return token
    }
    switch (poll.json.error) {
      case 'authorization_pending':
        continue
      case 'slow_down':
        intervalMs =
          typeof poll.json.interval === 'number'
            ? poll.json.interval * 1000
            : intervalMs + 5000
        continue
      case 'access_denied':
        throw new CliFailure('not-allowed', 'giriş tarayıcıda reddedildi')
      case 'expired_token':
        throw new CliFailure('token-expired', 'giriş kodunun süresi doldu')
      default:
        throw new CliFailure(
          'failed',
          `giriş başarısız: ${String(poll.json.error)}`
        )
    }
  }
  throw new CliFailure('token-expired', 'giriş kodunun süresi doldu')
}

/** The token of `env`, refreshed when it is about to expire. */
export async function currentToken(
  env: string,
  force = false
): Promise<StoredToken> {
  const token = readToken(env)
  if (!token) {
    throw new CliFailure(
      'not-logged-in',
      `giriş yok: önce gecko login --env ${env}`
    )
  }
  if (!force && token.accessExpiresAt - Date.now() > 30_000) {
    return token
  }
  const response = await post(`${baseUrl(resolveEnv(env))}/oauth/token`, {
    grant_type: 'refresh_token',
    refresh_token: token.refreshToken,
    client_id: 'gecko-cli'
  })
  if (response.status !== 200) {
    throw new CliFailure(
      'token-expired',
      `oturumun süresi doldu ya da yetki iptal edildi: gecko login --env ${env}`
    )
  }
  const next = stored(env, response.json as unknown as TokenResponse)
  writeToken(next)
  return next
}

/** Revokes the grant on the server (RFC 7009); ignores a network error. */
export async function revokeToken(token: StoredToken) {
  try {
    await post(`${baseUrl(resolveEnv(token.env))}/oauth/revoke`, {
      token: token.refreshToken,
      token_type_hint: 'refresh_token'
    })
  } catch {
    // The file is deleted anyway; the grant can be revoked from the profile.
  }
}

export function sleep(ms: number, signal?: AbortSignal) {
  return new Promise<void>((resolve, reject) => {
    if (signal?.aborted) {
      reject(new CliFailure('cancelled', 'iptal edildi'))
      return
    }
    const timer = setTimeout(() => {
      signal?.removeEventListener('abort', abort)
      resolve()
    }, ms)
    const abort = () => {
      clearTimeout(timer)
      reject(new CliFailure('cancelled', 'iptal edildi'))
    }
    signal?.addEventListener('abort', abort, { once: true })
  })
}
