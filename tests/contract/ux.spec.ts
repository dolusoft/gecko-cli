// The UX round (frontend-v2-design design/desktop-cli-ux.md): `gecko status`
// (U1), the wait indicator (U3), typo help (U5) and the robustness items
// (U9), against the development hub as a real process.

import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest'

import { commands, newProbe } from '../support/commands.ts'
import type { HubProcess } from '../support/harness.ts'
import {
  attach,
  cli,
  fakeTab,
  jsonRun,
  login,
  startHub,
  tempConfig
} from '../support/harness.ts'

const fast = {
  GECKO_DELIVERY_MARGIN_MS: '300',
  GECKO_CAPABILITIES_WAIT_MS: '800',
  GECKO_STATUS_WAIT_MS: '600'
}

let hub: HubProcess
const cleanups: (() => Promise<void> | void)[] = []

beforeAll(async () => {
  hub = await startHub({
    GECKO_DEV_HUB_TIMINGS: JSON.stringify({
      intervalMs: 50,
      suspendMs: 1500,
      requestTtlMs: 1500
    })
  })
})

afterEach(async () => {
  for (const cleanup of cleanups.splice(0).reverse()) await cleanup()
})

afterAll(async () => {
  await hub?.stop()
})

async function loggedIn(own = hub) {
  const config = tempConfig()
  cleanups.push(() => config.remove())
  await login(own, config.dir)
  return config.dir
}

async function bound() {
  const dir = await loggedIn()
  const probe = newProbe()
  const tab = await fakeTab({ hubUrl: hub.url, commands: commands(probe) })
  cleanups.push(() => tab.close())
  await attach(hub, dir, tab)
  return { dir, tab, probe }
}

const run = (args: string[], dir: string) => jsonRun(args, dir, fast)

describe('U1 gecko status', () => {
  it('no login: code 6', async () => {
    const config = tempConfig()
    cleanups.push(() => config.remove())
    const done = await run(['status'], config.dir)
    expect(done.code).toBe(6)
    expect(done.result).toMatchObject({ code: 'not-logged-in', exitCode: 6 })
  })

  it('no binding: says so and how, code 0, starts no pairing', async () => {
    const dir = await loggedIn()
    const done = await cli(['status'], {
      configDir: dir,
      env: { ...fast, GECKO_TEST_TTY: '1' }
    }).done
    expect(done.code).toBe(0)
    expect(done.stdout).toMatch(/^ortam {3}\S+ · zahid$/m)
    expect(done.stdout).toContain('sekme   yok · bağlamak için: gecko attach')
    expect(done.stdout).toMatch(/^hub {5}ulaşılıyor · \d+ ms$/m)
    expect(done.stderr).not.toMatch(/Kod: /)
  })

  it('bound: title and path from nav.where; --json has the fields', async () => {
    const { dir } = await bound()
    const plain = await cli(['status'], { configDir: dir, env: fast }).done
    expect(plain.code).toBe(0)
    expect(plain.stdout).toMatch(
      /sekme {3}bağlı \(onay \d\d\.\d\d \d\d:\d\d\) · Ağ Trafiği · \/dashboards\/default\/Defaults\/network-traffic/
    )
    const json = await run(['status'], dir)
    expect(json.result.data).toMatchObject({
      user: { name: 'zahid' },
      env: hub.env,
      hub: { reachable: true },
      binding: {
        state: 'active',
        title: 'Ağ Trafiği',
        path: '/dashboards/default/Defaults/network-traffic',
        tab: 'ok'
      }
    })
  })

  it('the tab does not answer: "yanıt yok", code 0', async () => {
    const { dir, tab } = await bound()
    tab.deaf = true
    const done = await cli(['status'], { configDir: dir, env: fast }).done
    expect(done.code).toBe(0)
    expect(done.stdout).toContain('sekme: yanıt yok')
  })

  it('suspended and gone are named', async () => {
    const { dir, tab } = await bound()
    await tab.drop()
    const suspended = await run(['status'], dir)
    expect(suspended.result.data).toMatchObject({
      binding: { state: 'suspended' }
    })
    await tab.reconnect()
    await tab.tab.unload()
    const gone = await cli(['status'], { configDir: dir, env: fast }).done
    expect(gone.stdout).toMatch(/sekme {3}sona erdi: .*gecko attach/)
  })

  it('another gecko call holds the hub: the local part and a line, code 0', async () => {
    const { dir } = await bound()
    const slow = cli(['slow', '--ms', '3000'], { configDir: dir, env: fast })
    await new Promise(r => setTimeout(r, 600))
    const done = await cli(['status'], { configDir: dir, env: fast }).done
    expect(done.code).toBe(0)
    expect(done.stdout).toContain('başka bir gecko çağrısı sürüyor')
    expect(done.stdout).toMatch(/^hub {5}ulaşılıyor$/m)
    slow.interrupt()
    await slow.done
  })

  it('no hub: code 4', async () => {
    const other = await startHub()
    const dir = await loggedIn(other)
    await other.stop()
    const done = await run(['status'], dir)
    expect(done.code).toBe(4)
    expect(done.result).toMatchObject({ exitCode: 4 })
  })
})

describe('U3 the wait indicator', () => {
  it('on a TTY a call past 700 ms shows one line, then clears it', async () => {
    const { dir } = await bound()
    const done = await cli(['slow', '--ms', '1500'], {
      configDir: dir,
      env: { ...fast, GECKO_TEST_TTY: '1' }
    }).done
    expect(done.code).toBe(0)
    expect(done.stderr).toMatch(/sekmede çalışıyor… \d,\d sn/)
    expect(done.stderr.endsWith('\r\u001b[2K')).toBe(true)
    expect(done.stdout).toBe('slow done\n')
  })

  it('in a pipe or with --json nothing is printed', async () => {
    const { dir } = await bound()
    const pipe = await cli(['slow', '--ms', '1500'], {
      configDir: dir,
      env: fast
    }).done
    expect(pipe.stderr).not.toContain('çalışıyor')
    const json = await cli(['--json', 'slow', '--ms', '1500'], {
      configDir: dir,
      env: { ...fast, GECKO_TEST_TTY: '1' }
    }).done
    expect(json.stderr).not.toContain('çalışıyor')
  })
})

describe('U5 typo help', () => {
  it('an unknown command names the near ones; --json has suggest', async () => {
    const { dir, tab } = await bound()
    const before = tab.heard.length
    const done = await run(['tasks.lst'], dir)
    expect(done.code).toBe(2)
    expect(done.result.message).toBe(
      'bilinmeyen komut: tasks.lst · bunu mu demek istediniz: tasks.list'
    )
    expect(done.result.suggest).toBe('tasks.list')
    expect(tab.heard.length).toBe(before)
  })

  it('bare words for a single string field: the line is suggested, not run', async () => {
    const { dir } = await bound()
    const done = await cli(['nav.find', 'data', 'view'], {
      configDir: dir,
      env: fast
    }).done
    expect(done.code).toBe(2)
    expect(done.stderr).toContain('kullanım: gecko help nav.find')
    expect(done.stderr).toContain(
      "şunu mu demek istediniz: gecko nav.find --text 'data view'"
    )
    const json = await run(['nav.find', 'data', 'view'], dir)
    expect(json.result).toMatchObject({
      code: 'invalid-args',
      // quoteArgument quotes with single quotes, as the tab reads them.
      suggest: "nav.find --text 'data view'",
      exitCode: 2
    })
  })

  it('a CLI option after the command: code 2 with the right order', async () => {
    const { dir, tab } = await bound()
    const before = tab.heard.length
    const done = await cli(['tasks.list', '--json'], {
      configDir: dir,
      env: fast
    }).done
    expect(done.code).toBe(2)
    expect(done.stderr).toContain(
      'CLI seçenekleri komuttan önce gelir: gecko --json tasks.list'
    )
    expect(tab.heard.length).toBe(before)
  })
})

describe('U9 robustness', () => {
  it('attach names the tab by title and path; a second attach too', async () => {
    const dir = await loggedIn()
    const tab = await fakeTab({
      hubUrl: hub.url,
      commands: commands(newProbe())
    })
    cleanups.push(() => tab.close())
    const first = await attach(hub, dir, tab)
    expect(first.stdout).toContain(
      'Bağlandı: "Ağ Trafiği" · /dashboards/default/Defaults/network-traffic'
    )
    const again = await cli(['attach'], { configDir: dir, env: fast }).done
    expect(again.stdout).toContain('Zaten bağlı: "Ağ Trafiği"')
  })

  it('attach on the pairing page says only "Bağlandı"', async () => {
    const dir = await loggedIn()
    const onPairPage = commands(newProbe()).map(command =>
      command.name === 'nav.where'
        ? {
            ...command,
            run: () => ({
              summary: '/cli-pair',
              data: { path: '/cli-pair', title: 'Connect the desktop CLI' }
            })
          }
        : command
    )
    const tab = await fakeTab({ hubUrl: hub.url, commands: onPairPage })
    cleanups.push(() => tab.close())
    const done = await attach(hub, dir, tab)
    expect(done.stdout).toContain('Bağlandı')
    expect(done.stdout).not.toContain('Connect the desktop CLI')
    expect(done.stdout).not.toContain('/cli-pair')
  })

  it('the view line: only for dataview.* or an explicit --view', async () => {
    const dir = await loggedIn()
    const tab = await fakeTab({
      hubUrl: hub.url,
      commands: [
        ...commands(newProbe()),
        {
          name: 'dataview.rows',
          danger: 'read',
          input: { type: 'object', properties: {} },
          run: () => ({ summary: '2 satır' })
        }
      ],
      extra: { activeView: () => 'v-1' },
      viewTitle: () => 'Tab Log View'
    })
    cleanups.push(() => tab.close())
    await attach(hub, dir, tab)
    const other = await cli(['nav.where'], { configDir: dir, env: fast }).done
    expect(other.code).toBe(0)
    expect(other.stderr).not.toContain('Görünüm:')
    const rows = await cli(['dataview.rows'], { configDir: dir, env: fast })
      .done
    expect(rows.stderr).toContain('Görünüm: Tab Log View')
    const explicit = await cli(['--view', 'v-1', 'nav.where'], {
      configDir: dir,
      env: fast
    }).done
    expect(explicit.stderr).toContain('Görünüm: Tab Log View')
    const json = await run(['nav.where'], dir)
    expect(json.result.view).toMatchObject({ id: 'v-1', title: 'Tab Log View' })
  })

  it('detach without a binding says there was none, code 0', async () => {
    const dir = await loggedIn()
    const done = await cli(['detach'], { configDir: dir }).done
    expect(done.code).toBe(0)
    expect(done.stdout).toBe('Bağlama yoktu\n')
  })

  it('--json help without a login: a JSON error, code 6', async () => {
    const config = tempConfig()
    cleanups.push(() => config.remove())
    const done = await run(['help'], config.dir)
    expect(done.code).toBe(6)
    expect(done.result).toMatchObject({
      status: 'error',
      code: 'not-logged-in',
      exitCode: 6
    })
  })
})
