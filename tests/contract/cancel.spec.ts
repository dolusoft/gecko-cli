// §6.6: Ctrl+C. The specs press it through the CLI's stdin hook, as
// Windows cannot send SIGINT to a child process; CI on Linux also runs the
// real signal.

import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest'

import { commands, newProbe } from '../support/commands.ts'
import type { FakeTab, HubProcess } from '../support/harness.ts'
import {
  attach,
  cli,
  fakeTab,
  login,
  pause,
  startHub,
  tempConfig
} from '../support/harness.ts'

let hub: HubProcess
let tab: FakeTab
const config = tempConfig()
const probe = newProbe()

beforeAll(async () => {
  hub = await startHub({
    GECKO_DEV_HUB_TIMINGS: JSON.stringify({ intervalMs: 50 })
  })
  await login(hub, config.dir)
  tab = await fakeTab({ hubUrl: hub.url, commands: commands(probe) })
  await attach(hub, config.dir, tab)
})

beforeEach(async () => {
  // A run the deaf tab never heard cancelled stops before the next spec.
  tab.console.cancel()
  await pause(50)
  tab.deaf = false
  probe.aborted = 0
  probe.stops = 0
})

afterAll(async () => {
  await tab?.close()
  await hub?.stop()
  config.remove()
})

const parse = (stdout: string) => JSON.parse(stdout) as Record<string, unknown>

describe('§6.6 cancel', () => {
  it('before the command started: cancelled-before-start, 130', async () => {
    const call = cli(['--json', 'tasks.stop', '--id', '5', '--yes'], {
      configDir: config.dir
    })
    await call.waitStderr(/Onay bekleniyor/)
    call.interrupt()
    const done = await call.done
    expect(done.code).toBe(130)
    expect(parse(done.stdout)).toMatchObject({
      status: 'cancelled',
      code: 'cancelled-before-start'
    })
    expect(probe.stops).toBe(0)
  })

  it('while it runs: cancel-requested, its signal aborts, 130', async () => {
    const call = cli(['--json', 'slow', '--ms', '5000'], {
      configDir: config.dir
    })
    await pause(400)
    call.interrupt()
    const done = await call.done
    expect(done.code).toBe(130)
    expect(parse(done.stdout)).toMatchObject({
      status: 'cancelled',
      code: 'cancel-requested'
    })
    expect(probe.aborted).toBe(1)
  })

  it('no answer within the wait: unknown, 130', async () => {
    const call = cli(['--json', 'slow', '--ms', '3000'], {
      configDir: config.dir,
      env: { GECKO_CANCEL_WAIT_MS: '300' }
    })
    await pause(400)
    tab.deaf = true
    call.interrupt()
    const done = await call.done
    expect(done.code).toBe(130)
    expect(parse(done.stdout)).toMatchObject({
      status: 'cancelled',
      code: 'unknown'
    })
  })

  it('a second Ctrl+C ends the wait at once: unknown, 130', async () => {
    const call = cli(['--json', 'slow', '--ms', '4000'], {
      configDir: config.dir
    })
    await pause(400)
    tab.deaf = true
    const started = Date.now()
    call.interrupt()
    await pause(100)
    call.interrupt()
    const done = await call.done
    expect(Date.now() - started).toBeLessThan(2500)
    expect(done.code).toBe(130)
    expect(parse(done.stdout)).toMatchObject({ code: 'unknown' })
  })

  it('a cancelled change never claims a sure cancel', async () => {
    const call = cli(['--json', 'tasks.stop', '--id', '5', '--yes'], {
      configDir: config.dir
    })
    await call.waitStderr(/Onay bekleniyor/)
    tab.deaf = true
    call.interrupt()
    call.interrupt()
    const done = await call.done
    expect(done.code).toBe(130)
    expect(parse(done.stdout).complete).toBe(false)
    await pause(50)
    tab.deaf = false
    // The plan is still in the tab; the user rejects it there.
    await tab.rejectNext()
  })
})

describe('§6.6 with the real signal', () => {
  it.skipIf(process.platform === 'win32')(
    'SIGINT cancels a running command',
    async () => {
      const call = cli(['--json', 'slow', '--ms', '5000'], {
        configDir: config.dir
      })
      await pause(400)
      call.child.kill('SIGINT')
      const done = await call.done
      expect(done.code).toBe(130)
      expect(parse(done.stdout)).toMatchObject({ code: 'cancel-requested' })
    }
  )
})
