// The CLI's files: the chosen environment, one token per environment (the
// only secret on disk, §3) and the capability cache of the bound page. The
// token file is the user's alone: mode 0600 on POSIX, the user's ACL only on
// Windows (§8).

import { execFileSync } from 'node:child_process'
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync
} from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'

export interface StoredToken {
  env: string
  accessToken: string
  /** ms since the epoch. */
  accessExpiresAt: number
  refreshToken: string
  grantId: string
  user: { id: string; name: string }
}

export interface StoredCapabilities {
  pageId: string
  bindingId: string
  capabilities: unknown
}

export function configDir(): string {
  if (process.env.GECKO_CONFIG_DIR) {
    return process.env.GECKO_CONFIG_DIR
  }
  if (process.platform === 'win32') {
    return join(
      process.env.APPDATA ?? join(homedir(), 'AppData', 'Roaming'),
      'gecko-cli'
    )
  }
  return join(
    process.env.XDG_CONFIG_HOME ?? join(homedir(), '.config'),
    'gecko-cli'
  )
}

const fileKey = (env: string) => env.replace(/[^A-Za-z0-9.-]/g, '_')

function ensureDir(): string {
  const dir = configDir()
  if (!existsSync(dir)) {
    mkdirSync(dir, { recursive: true, mode: 0o700 })
    restrictWindows(dir, true)
  }
  return dir
}

/** Removes inherited rights and grants the current user alone. */
function restrictWindows(path: string, directory: boolean) {
  if (process.platform !== 'win32') {
    return
  }
  const user = process.env.USERNAME
  if (!user) {
    return
  }
  const grant = directory ? `${user}:(OI)(CI)F` : `${user}:F`
  try {
    execFileSync('icacls', [path, '/inheritance:r', '/grant:r', grant], {
      stdio: 'ignore',
      windowsHide: true
    })
  } catch {
    // The file stays in the user's profile directory; the ACL is a second
    // fence, not the only one.
  }
}

function readJson<T>(path: string): T | undefined {
  try {
    return JSON.parse(readFileSync(path, 'utf8')) as T
  } catch {
    return undefined
  }
}

function writeSecret(path: string, text: string) {
  const temp = `${path}.${process.pid}.tmp`
  writeFileSync(temp, text, { encoding: 'utf8', mode: 0o600 })
  if (process.platform !== 'win32') {
    chmodSync(temp, 0o600)
  }
  restrictWindows(temp, false)
  renameSync(temp, path)
}

export function readDefaultEnv(): string | undefined {
  return readJson<{ env?: string }>(join(configDir(), 'config.json'))?.env
}

export function writeDefaultEnv(env: string) {
  const dir = ensureDir()
  writeFileSync(join(dir, 'config.json'), JSON.stringify({ env }, null, 2))
}

export function tokenPath(env: string): string {
  return join(configDir(), `token-${fileKey(env)}.json`)
}

export function readToken(env: string): StoredToken | undefined {
  return readJson<StoredToken>(tokenPath(env))
}

export function writeToken(token: StoredToken) {
  ensureDir()
  writeSecret(tokenPath(token.env), JSON.stringify(token, null, 2))
}

export function deleteToken(env: string) {
  rmSync(tokenPath(env), { force: true })
  rmSync(capabilitiesPath(env), { force: true })
}

function capabilitiesPath(env: string): string {
  return join(configDir(), `capabilities-${fileKey(env)}.json`)
}

export function readCapabilities(env: string): StoredCapabilities | undefined {
  return readJson<StoredCapabilities>(capabilitiesPath(env))
}

export function writeCapabilities(env: string, value: StoredCapabilities) {
  ensureDir()
  writeFileSync(capabilitiesPath(env), JSON.stringify(value))
}
