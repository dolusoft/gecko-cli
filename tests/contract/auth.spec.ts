// §8: device login (RFC 8628), the token file, refresh, logout and
// revocation, against the development hub as a real process.

import { execFileSync } from 'node:child_process'
import { existsSync, statSync } from 'node:fs'
import { join } from 'node:path'

import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest'

import { commands, newProbe } from '../support/commands.ts'
import type { HubProcess } from '../support/harness.ts'
import {
  approveDevice,
  attach,
  cli,
  fakeTab,
  jsonRun,
  login,
  pause,
  startHub,
  storedToken,
  tempConfig
} from '../support/harness.ts'

let hub: HubProcess
const cleanups: (() => Promise<void> | void)[] = []

beforeAll(async () => {
  hub = await startHub({
    GECKO_DEV_HUB_TIMINGS: JSON.stringify({
      intervalMs: 50,
      accessTtlMs: 31_000
    })
  })
})

afterEach(async () => {
  for (const cleanup of cleanups.splice(0).reverse()) await cleanup()
})

afterAll(async () => {
  await hub?.stop()
})

function config() {
  const value = tempConfig()
  cleanups.push(() => value.remove())
  return value.dir
}

describe('§8 device login', () => {
  it('prints the code, waits for the approval, stores the token', async () => {
    const dir = config()
    const run = await login(hub, dir)
    expect(run.stderr).toMatch(/Kod: [A-Z0-9]{4}-[A-Z0-9]{4}/)
    expect(run.stdout).toBe(`Onaylandı: zahid (${hub.env})\n`)
    expect(storedToken(dir, hub.env).grantId).toMatch(/^g_/)
  })

  it('the token file is the user’s alone', async () => {
    const dir = config()
    await login(hub, dir)
    const file = join(
      dir,
      `token-${hub.env.replace(/[^A-Za-z0-9.-]/g, '_')}.json`
    )
    if (process.platform === 'win32') {
      const acl = execFileSync('icacls', [file], { encoding: 'utf8' })
      expect(acl).toContain(process.env.USERNAME!)
      expect(acl).not.toMatch(
        /Everyone|BUILTIN\\Users|Authenticated Users|Herkes/
      )
      expect(acl).not.toContain('(I)')
    } else {
      expect(statSync(file).mode & 0o777).toBe(0o600)
    }
  })

  it('a denial in the browser: code 3', async () => {
    const dir = config()
    const run = cli(['--json', 'login', '--env', hub.env], { configDir: dir })
    const [, code] = await run.waitStderr(/Kod: (\S+)/)
    await approveDevice(hub.url, code, { approve: false })
    const done = await run.done
    expect(done.code).toBe(3)
    expect(existsSync(join(dir, 'config.json'))).toBe(false)
  })

  it('polling faster than the interval gets slow_down and a longer interval', async () => {
    const start = (await (
      await fetch(`${hub.url}/oauth/device_authorization`, { method: 'POST' })
    ).json()) as { device_code: string; interval: number }
    const poll = () =>
      fetch(`${hub.url}/oauth/token`, {
        method: 'POST',
        headers: { 'content-type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams({
          grant_type: 'urn:ietf:params:oauth:grant-type:device_code',
          device_code: start.device_code
        })
      }).then(r => r.json() as Promise<{ error: string; interval?: number }>)
    expect((await poll()).error).toBe('authorization_pending')
    const second = await poll()
    expect(second.error).toBe('slow_down')
    expect(second.interval).toBeGreaterThan(start.interval)
  })

  it('an expired device code: code 6', async () => {
    const own = await startHub({
      GECKO_DEV_HUB_TIMINGS: JSON.stringify({
        intervalMs: 50,
        deviceTtlMs: 400
      })
    })
    cleanups.push(() => own.stop())
    const done = await cli(['--json', 'login', '--env', own.env], {
      configDir: config()
    }).done
    expect(done.code).toBe(6)
    expect(JSON.parse(done.stdout).code).toBe('token-expired')
  })

  it('no login: code 6 with the hint', async () => {
    const run = await jsonRun(['search', '--ip', '1'], config())
    expect(run.code).toBe(6)
    expect(run.result).toMatchObject({ code: 'not-logged-in' })
  })

  it('a refresh keeps the grant and the binding', async () => {
    const dir = config()
    await login(hub, dir)
    const tab = await fakeTab({
      hubUrl: hub.url,
      commands: commands(newProbe())
    })
    cleanups.push(() => tab.close())
    await attach(hub, dir, tab)
    const before = storedToken(dir, hub.env)
    // The access token lives 31 s; the CLI refreshes 30 s before the end.
    await pause(1200)
    const run = await jsonRun(['search', '--ip', '1'], dir)
    expect(run.code).toBe(0)
    const after = storedToken(dir, hub.env)
    expect(after.accessToken).not.toBe(before.accessToken)
    expect(after.grantId).toBe(before.grantId)
  })

  it('logout revokes the grant: the binding is gone, logged-out, and the token deleted', async () => {
    const dir = config()
    await login(hub, dir)
    const tab = await fakeTab({
      hubUrl: hub.url,
      commands: commands(newProbe())
    })
    cleanups.push(() => tab.close())
    await attach(hub, dir, tab)
    const events: unknown[] = []
    tab.tab.onBinding(event => events.push(event))
    const run = await cli(['logout'], { configDir: dir }).done
    expect(run.code).toBe(0)
    await pause(100)
    expect(events).toContainEqual(
      expect.objectContaining({ state: 'gone', reason: 'logged-out' })
    )
    expect((await jsonRun(['search', '--ip', '1'], dir)).code).toBe(6)
  })

  it('a grant revoked from the profile: code 6, the bindings are gone, revoked', async () => {
    const dir = config()
    await login(hub, dir)
    const tab = await fakeTab({
      hubUrl: hub.url,
      commands: commands(newProbe())
    })
    cleanups.push(() => tab.close())
    await attach(hub, dir, tab)
    const events: unknown[] = []
    tab.tab.onBinding(event => events.push(event))
    const { grantId } = storedToken(dir, hub.env)
    const grants = (await (
      await fetch(`${hub.url}/cli/grants`, {
        headers: { authorization: 'Bearer dev:u1' }
      })
    ).json()) as { grantId: string }[]
    expect(grants.map(g => g.grantId)).toContain(grantId)
    const response = await fetch(`${hub.url}/cli/grants/${grantId}/revoke`, {
      method: 'POST',
      headers: { authorization: 'Bearer dev:u1' }
    })
    expect(response.status).toBe(200)
    await pause(100)
    expect(events).toContainEqual(
      expect.objectContaining({ state: 'gone', reason: 'revoked' })
    )
    const run = await jsonRun(['search', '--ip', '1'], dir)
    expect(run.code).toBe(6)
  })

  it('revocation closes a live CLI connection', async () => {
    const dir = config()
    await login(hub, dir)
    const tab = await fakeTab({
      hubUrl: hub.url,
      commands: commands(newProbe())
    })
    cleanups.push(() => tab.close())
    await attach(hub, dir, tab)
    const call = cli(['--json', 'slow', '--ms', '3000'], { configDir: dir })
    await pause(400)
    const { grantId } = storedToken(dir, hub.env)
    await fetch(`${hub.url}/cli/grants/${grantId}/revoke`, {
      method: 'POST',
      headers: { authorization: 'Bearer dev:u1' }
    })
    const done = await call.done
    // The hub closed the connection for the revocation: identity, code 6.
    expect(done.code).toBe(6)
    expect(JSON.parse(done.stdout)).toMatchObject({ code: 'revoked' })
  })
})

describe('the browser page reaches the hub (CORS)', () => {
  it('answers the preflight from the web origin and not from another one', async () => {
    const origin = new URL(hub.url).origin
    const ok = await fetch(`${hub.url}/oauth/device/verify`, {
      method: 'OPTIONS',
      headers: { Origin: origin, 'Access-Control-Request-Method': 'POST' }
    })
    expect(ok.status).toBe(204)
    expect(ok.headers.get('access-control-allow-origin')).toBe(origin)
    const other = await fetch(`${hub.url}/oauth/device/verify`, {
      method: 'OPTIONS',
      headers: {
        Origin: 'http://evil.example',
        'Access-Control-Request-Method': 'POST'
      }
    })
    expect(other.headers.get('access-control-allow-origin')).toBeNull()
  })

  const preflight = (url: string, origin: string) =>
    fetch(`${url}/oauth/device/verify`, {
      method: 'OPTIONS',
      headers: { Origin: origin, 'Access-Control-Request-Method': 'POST' }
    }).then(r => r.headers.get('access-control-allow-origin'))

  it('without --web, any loopback origin (a dev server on its own port) and nothing else', async () => {
    for (const origin of [
      'http://127.0.0.1:5186',
      'http://localhost:5186',
      'http://[::1]:5186'
    ]) {
      expect(await preflight(hub.url, origin)).toBe(origin)
    }
    for (const origin of [
      'https://127.0.0.1:5186',
      'http://127.0.0.2:5186',
      'http://localhost.evil.example:5186',
      'http://192.168.1.10:5186',
      'null'
    ]) {
      expect(await preflight(hub.url, origin)).toBeNull()
    }
    expect(hub.stderr()).toMatch(/cors: loopback origins only/)
  })

  it('with --web, that one origin only', async () => {
    const web = await startHub({}, ['--web', 'http://localhost:5186'])
    try {
      expect(await preflight(web.url, 'http://localhost:5186')).toBe(
        'http://localhost:5186'
      )
      expect(await preflight(web.url, 'http://127.0.0.1:5186')).toBeNull()
      expect(await preflight(web.url, new URL(web.url).origin)).toBeNull()
      expect(web.stderr()).toMatch(/cors: http:\/\/localhost:5186 only/)
    } finally {
      await web.stop()
    }
  })
})
