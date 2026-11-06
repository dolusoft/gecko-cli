// §5.5 (tabs and bindings), §5.3 (the life of a binding) and the extra
// checks of §13, against the development hub as a real process.

import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest'

import { commands, newProbe } from '../support/commands.ts'
import type { FakeTab, HubProcess } from '../support/harness.ts'
import {
  attach,
  cli,
  fakeTab,
  jsonRun,
  login,
  pause,
  rawCli,
  startHub,
  tempConfig
} from '../support/harness.ts'

const timings = {
  intervalMs: 50,
  suspendMs: 1500,
  pairTtlMs: 3000,
  requestTtlMs: 1500
}
const fast = {
  GECKO_DELIVERY_MARGIN_MS: '300',
  GECKO_CAPABILITIES_WAIT_MS: '800'
}

let hub: HubProcess
const cleanups: (() => Promise<void> | void)[] = []

beforeAll(async () => {
  hub = await startHub({
    GECKO_DEV_HUB_TIMINGS: JSON.stringify(timings),
    GECKO_DEV_HUB_USERS: JSON.stringify([
      { id: 'u1', name: 'zahid' },
      { id: 'u2', name: 'enes' },
      { id: 'u3', name: 'misafir', terminal: false }
    ])
  })
})

afterEach(async () => {
  for (const cleanup of cleanups.splice(0).reverse()) await cleanup()
})

afterAll(async () => {
  await hub?.stop()
})

async function loggedIn(user = 'u1') {
  const config = tempConfig()
  cleanups.push(() => config.remove())
  await login(hub, config.dir, user)
  return config.dir
}

async function tabOf(options: Partial<Parameters<typeof fakeTab>[0]> = {}) {
  const probe = newProbe()
  const tab = await fakeTab({
    hubUrl: hub.url,
    commands: commands(probe),
    ...options
  })
  cleanups.push(() => tab.close())
  return { tab, probe }
}

async function bound(options: Partial<Parameters<typeof fakeTab>[0]> = {}) {
  const dir = await loggedIn(options.user)
  const { tab, probe } = await tabOf(options)
  await attach(hub, dir, tab)
  return { dir, tab, probe }
}

const run = (args: string[], dir: string) => jsonRun(args, dir, fast)

describe('§5.5 tabs', () => {
  it('no tab: the first command opens pairing, then runs', async () => {
    const dir = await loggedIn()
    const { tab } = await tabOf()
    const call = cli(['--json', 'search', '--ip', '1'], { configDir: dir })
    const [, code] = await call.waitStderr(/Kod: (\S+)/)
    await tab.pair(code)
    const done = await call.done
    expect(done.code).toBe(0)
    expect(JSON.parse(done.stdout).status).toBe('ok')
  })

  it('no approval within the pairing time: code 3', async () => {
    const dir = await loggedIn()
    const done = await cli(['--json', 'attach'], { configDir: dir }).done
    expect(done.code).toBe(3)
    expect(JSON.parse(done.stdout).code).toBe('pair-timeout')
  })

  it('no desktop: the CLI prints the code and the address', async () => {
    const dir = await loggedIn()
    const call = cli(['attach'], { configDir: dir })
    const [line] = await call.waitStderr(/Kod: \S+ .*cli-pair#\S+/)
    expect(line).toContain(`${hub.url}/cli-pair#`)
    await call.waitStderr(/cli\.attach \S+/)
    call.interrupt()
    expect((await call.done).code).toBe(130)
  })

  it('another account in the browser: the hub refuses, the CLI says so', async () => {
    const dir = await loggedIn('u1')
    const { tab: other } = await tabOf({ user: 'u2' })
    const call = cli(['attach'], { configDir: dir })
    const [, code] = await call.waitStderr(/Kod: (\S+)/)
    await expect(other.tab.pair(code)).rejects.toMatchObject({
      code: 'other-account'
    })
    await call.waitStderr(/tarayıcıda başka hesap açık/)
    // The code stays usable in the right account.
    const { tab } = await tabOf({ user: 'u1' })
    await tab.pair(code)
    expect((await call.done).code).toBe(0)
  })

  it('another environment: the code does not exist there', async () => {
    const { tab } = await tabOf()
    await expect(tab.tab.pairInfo('KPWD-7Q4M')).rejects.toMatchObject({
      code: 'invalid-code'
    })
  })

  it('no terminal access: no binding, the code is not used, code 3', async () => {
    const dir = await loggedIn('u3')
    const { tab } = await tabOf({ user: 'u3' })
    const call = cli(['--json', 'attach'], { configDir: dir })
    const [, code] = await call.waitStderr(/Kod: (\S+)/)
    await expect(tab.tab.pairInfo(code)).rejects.toMatchObject({
      code: 'no-terminal-access'
    })
    const done = await call.done
    expect(done.code).toBe(3)
    expect(JSON.parse(done.stdout).code).toBe('no-terminal-access')
  })

  it('the bound tab closed or reloaded: gone, closed, code 4', async () => {
    const { dir, tab } = await bound()
    await tab.tab.unload()
    const result = await run(['search', '--ip', '1'], dir)
    // No new tab on its own: gone, with the reason and `gecko attach`.
    expect(result.code).toBe(4)
    expect(result.result).toMatchObject({ code: 'gone', reason: 'closed' })
    expect(result.result.message).toContain('gecko attach')
  })

  it('a gone binding refuses the request with its reason', async () => {
    const { dir, tab } = await bound()
    await tab.tab.unload()
    const transport = await rawCli(hub, dir)
    await expect(
      transport.send({
        v: 1,
        kind: 'cli-request',
        id: 'r-gone',
        line: 'search --ip 1'
      })
    ).rejects.toMatchObject({ code: 'gone', reason: 'closed' })
    await transport.close()
  })

  it('Wi-Fi drops and returns within 60 s: tab-unreachable between, then it runs', async () => {
    const { dir, tab } = await bound()
    await tab.drop()
    await pause(100)
    const between = await run(['search', '--ip', '1'], dir)
    expect(between.code).toBe(4)
    expect(between.result.code).toBe('tab-unreachable')
    await tab.reconnect()
    const after = await run(['search', '--ip', '1'], dir)
    expect(after.code).toBe(0)
  })

  it('the suspension runs out: gone, suspended-timeout', async () => {
    const { dir, tab } = await bound()
    await tab.drop()
    await pause(timings.suspendMs + 300)
    const transport = await rawCli(hub, dir)
    const status = await transport.status()
    await transport.close()
    expect(status.binding).toMatchObject({
      state: 'gone',
      reason: 'suspended-timeout'
    })
  })

  it('no received before expiresAt: not-delivered, code 4, and it did not run', async () => {
    const { dir, tab, probe } = await bound()
    tab.deaf = true
    const result = await run(['tasks.stop', '--id', '1', '--yes'], dir)
    expect(result.code).toBe(4)
    expect(result.result.code).toBe('not-delivered')
    tab.deaf = false
    expect(probe.stops).toBe(0)
  })

  it('no received and the hub connection breaks: unknown, code 5', async () => {
    const own = await startHub({
      GECKO_DEV_HUB_TIMINGS: JSON.stringify(timings)
    })
    const config = tempConfig()
    cleanups.push(() => config.remove())
    await login(own, config.dir)
    const probe = newProbe()
    const tab = await fakeTab({ hubUrl: own.url, commands: commands(probe) })
    await attach(own, config.dir, tab)
    tab.deaf = true
    const call = cli(['--json', 'search', '--ip', '1'], {
      configDir: config.dir,
      env: fast
    })
    await pause(300)
    own.child.kill()
    const done = await call.done
    expect(done.code).toBe(5)
    expect(JSON.parse(done.stdout).code).toBe('unknown')
    await tab.close()
  })

  it('received, then the tab drops: a read is connection-lost (4)', async () => {
    const { dir, tab } = await bound()
    const call = cli(['--json', 'slow', '--ms', '3000'], {
      configDir: dir,
      env: fast
    })
    await pause(400)
    await tab.drop()
    const done = await call.done
    expect(done.code).toBe(4)
    expect(JSON.parse(done.stdout).code).toBe('connection-lost')
  })

  it('a drop during the approval: unknown-outcome (5), the plan is dropped', async () => {
    const { dir, tab, probe } = await bound()
    const call = cli(['--json', 'tasks.stop', '--id', '9', '--yes'], {
      configDir: dir,
      env: fast
    })
    await call.waitStderr(/Onay bekleniyor/)
    await tab.drop()
    const done = await call.done
    expect(done.code).toBe(5)
    expect(JSON.parse(done.stdout).code).toBe('unknown-outcome')
    expect(tab.console.getState().remote?.approval ?? null).toBeNull()
    expect(probe.stops).toBe(0)
  })

  it('the user is running a command in the browser: busy', async () => {
    const { dir, tab } = await bound()
    void tab.console.run('slow --ms 1500')
    await pause(50)
    const result = await run(['search', '--ip', '1'], dir)
    expect(result.code).toBe(4)
    expect(result.result.code).toBe('busy')
  })

  it('two terminal windows, one grant: the second is busy and the tab sees it', async () => {
    const { dir, tab } = await bound()
    const first = cli(['--json', 'slow', '--ms', '1500'], { configDir: dir })
    await pause(300)
    const second = await run(['search', '--ip', '1'], dir)
    expect(second.code).toBe(4)
    expect(second.result.code).toBe('busy')
    expect(tab.busyAttempts).toHaveLength(1)
    expect((await first.done).code).toBe(0)
  })

  it('a second grant binds the same tab: the first is gone, replaced', async () => {
    const { dir, tab } = await bound()
    const otherDir = await loggedIn()
    await attach(hub, otherDir, tab)
    const transport = await rawCli(hub, dir)
    const status = await transport.status()
    await transport.close()
    expect(status.binding).toMatchObject({ state: 'gone', reason: 'replaced' })
    expect((await run(['search', '--ip', '1'], otherDir)).code).toBe(0)
  })

  it('a tab that predates the CLI sends no capabilities: refresh the page, code 2', async () => {
    const dir = await loggedIn()
    const { tab } = await tabOf()
    const call = cli(['--json', 'attach'], { configDir: dir, env: fast })
    const [, code] = await call.waitStderr(/Kod: (\S+)/)
    await tab.pairOnly(code)
    const done = await call.done
    expect(done.code).toBe(2)
    expect(JSON.parse(done.stdout)).toMatchObject({
      code: 'unsupported-version'
    })
    expect(JSON.parse(done.stdout).message).toContain('yenileyin')
  })

  it('the same request id twice: the tab ignores the second', async () => {
    const { dir, tab, probe } = await bound()
    const transport = await rawCli(hub, dir)
    const replies: unknown[] = []
    transport.onReply(reply => replies.push(reply))
    const envelope = {
      v: 1,
      kind: 'cli-request',
      id: 'same-id',
      line: 'echo --text a'
    }
    await transport.send(envelope)
    await pause(200)
    await transport.send(envelope)
    await pause(300)
    await transport.close()
    const finished = replies.filter(
      r => (r as { kind: string }).kind === 'finished'
    )
    expect(finished).toHaveLength(1)
    expect(
      tab.heard.filter(m => (m as { id: string }).id === 'same-id')
    ).toHaveLength(2)
    expect(probe.echoed).toEqual(['a'])
  })

  it('the tab clock is more than 30 s behind: clock-skew, code 4', async () => {
    const { dir } = await bound({ clockOffsetMs: -45_000 })
    const result = await run(['search', '--ip', '1'], dir)
    expect(result.code).toBe(4)
    expect(result.result.code).toBe('clock-skew')
  })
})

describe('§13 extra checks', () => {
  it('two calls in a row', async () => {
    const { dir } = await bound()
    expect((await run(['search', '--ip', '1'], dir)).code).toBe(0)
    expect((await run(['search', '--ip', '2'], dir)).code).toBe(0)
  })

  it('a CLI connection hears no browser broadcast (passive listening)', async () => {
    const { dir, tab } = await bound()
    const transport = await rawCli(hub, dir)
    const heard: unknown[] = []
    transport.onReply(reply => heard.push(reply))
    const { tab: sibling } = await tabOf()
    const got: unknown[] = []
    sibling.tab.onBroadcast(envelope => got.push(envelope))
    const sent = await tab.tab.broadcast({
      v: 1,
      kind: 'request',
      secret: 'trafik'
    })
    await pause(200)
    expect(sent.delivered).toBeGreaterThanOrEqual(1)
    expect(got).toHaveLength(1)
    expect(heard).toHaveLength(0)
    await transport.close()
  })

  it('a CLI cannot call the browser methods; another tab cannot send to the CLI', async () => {
    const { dir } = await bound()
    const transport = await rawCli(hub, dir)
    const heard: unknown[] = []
    transport.onReply(reply => heard.push(reply))
    const { tab: intruder } = await tabOf()
    await expect(
      intruder.tab.sendToCli({
        v: 1,
        kind: 'finished',
        id: 'x',
        outcome: { status: 'ok' },
        chunks: 0,
        bytes: 0
      })
    ).rejects.toMatchObject({ code: 'not-bound' })
    await pause(100)
    expect(heard).toHaveLength(0)
    await transport.close()
  })

  it('a CLI token in the query string is refused', async () => {
    const dir = await loggedIn()
    const { storedToken } = await import('../support/harness.ts')
    const token = storedToken(dir, hub.env).accessToken
    const socket = new WebSocket(`ws://${hub.env}/hub?access_token=${token}`)
    const code = await new Promise<number>(resolve =>
      socket.addEventListener('close', event => resolve(event.code))
    )
    expect(code).toBe(4403)
  })

  it('one code used twice at once: one binding', async () => {
    const dir = await loggedIn()
    const { tab: a } = await tabOf()
    const { tab: b } = await tabOf()
    const call = cli(['attach'], { configDir: dir })
    const [, code] = await call.waitStderr(/Kod: (\S+)/)
    const results = await Promise.allSettled([a.pair(code), b.pair(code)])
    expect(results.filter(r => r.status === 'fulfilled')).toHaveLength(1)
    expect(results.find(r => r.status === 'rejected')).toMatchObject({
      reason: { code: 'invalid-code' }
    })
    expect((await call.done).code).toBe(0)
  })

  it('a late request (the tab clock ahead): expired, not run, code 4', async () => {
    const { dir, probe } = await bound({ clockOffsetMs: 45_000 })
    const result = await run(['tasks.stop', '--id', '1', '--yes'], dir)
    expect(result.code).toBe(4)
    expect(result.result.code).toBe('expired')
    expect(probe.stops).toBe(0)
  })

  it('the rate limit: the sixth request in a second is busy', async () => {
    const own = await startHub({
      GECKO_DEV_HUB_TIMINGS: JSON.stringify(timings),
      GECKO_DEV_HUB_RATE: '5'
    })
    const config = tempConfig()
    cleanups.push(() => config.remove())
    await login(own, config.dir)
    const tab = await fakeTab({
      hubUrl: own.url,
      commands: commands(newProbe())
    })
    tab.deaf = true
    await attach(own, config.dir, tab)
    const transport = await rawCli(own, config.dir)
    const outcomes = await Promise.allSettled(
      Array.from({ length: 6 }, (_, i) =>
        transport.send({
          v: 1,
          kind: 'cli-request',
          id: `rate-${i}`,
          line: 'search --ip 1'
        })
      )
    )
    await transport.close()
    await tab.close()
    await own.stop()
    expect(outcomes.filter(o => o.status === 'fulfilled')).toHaveLength(5)
    expect(outcomes[5]).toMatchObject({ reason: { code: 'busy' } })
  })

  it('the hub overwrites a stamp the client wrote', async () => {
    const { dir, tab } = await bound()
    tab.deaf = true
    const transport = await rawCli(hub, dir)
    await transport.send({
      v: 1,
      kind: 'cli-request',
      id: 'forged',
      line: 'search --ip 1',
      origin: { kind: 'cli', grantId: 'someone-else', bindingId: 'b-x' },
      stampedAt: 1,
      expiresAt: 9_999_999_999_999
    })
    await pause(100)
    await transport.close()
    const seen = tab.heard.find(m => (m as { id: string }).id === 'forged') as {
      origin: { grantId: string }
      expiresAt: number
      stampedAt: number
    }
    expect(seen.origin.grantId).not.toBe('someone-else')
    expect(seen.expiresAt - seen.stampedAt).toBe(timings.requestTtlMs)
  })
})
