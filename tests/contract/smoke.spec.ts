import { afterAll, beforeAll, expect, it } from 'vitest'

import { newProbe, commands } from '../support/commands.ts'
import type { FakeTab, HubProcess } from '../support/harness.ts'
import {
  attach,
  cli,
  fakeTab,
  login,
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

afterAll(async () => {
  await tab?.close()
  await hub?.stop()
  config.remove()
})

it('runs a read line', async () => {
  const run = await cli(['--json', 'search', '--ip', '1.2.3.4'], {
    configDir: config.dir
  }).done
  console.log(run)
  expect(run.code).toBe(0)
})
