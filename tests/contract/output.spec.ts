// §6.1–§6.5 and §6.7: what returns to the CLI, on which channel, with which
// exit code; §10.1: the arguments reach the tab unchanged.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'

import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest'

import { commands, newProbe } from '../support/commands.ts'
import type { FakeTab, HubProcess } from '../support/harness.ts'
import {
  attach,
  cli,
  fakeTab,
  jsonRun,
  login,
  startHub,
  tempConfig
} from '../support/harness.ts'

let hub: HubProcess
let tab: FakeTab
const config = tempConfig()
const probe = newProbe()
let dropChunk = false
let hidden = false

beforeAll(async () => {
  hub = await startHub({
    GECKO_DEV_HUB_TIMINGS: JSON.stringify({ intervalMs: 50 })
  })
  await login(hub, config.dir)
  tab = await fakeTab({
    hubUrl: hub.url,
    commands: commands(probe),
    status: () =>
      hidden ? { hidden: true, dataAgeMs: 42_000 } : { hidden: false },
    dropReplies: reply =>
      dropChunk &&
      reply.kind === 'chunk' &&
      (reply as { index?: number }).index === 1,
    extra: { activeView: () => 'v-91c0' }
  })
  await attach(hub, config.dir, tab)
})

beforeEach(() => {
  dropChunk = false
  hidden = false
  probe.stops = 0
})

afterAll(async () => {
  await tab?.close()
  await hub?.stop()
  config.remove()
})

describe('§6.4 exit codes', () => {
  it('0: ok, with the view the tab fixed on accept', async () => {
    const run = await jsonRun(['search', '--ip', '172.16.40.25'], config.dir)
    expect(run.code).toBe(0)
    expect(run.result).toMatchObject({
      v: 1,
      status: 'ok',
      preview: false,
      complete: true,
      view: { id: 'v-91c0' }
    })
    expect(run.result.untrusted).toEqual(['summary', 'data'])
  })

  it('0: a preview, with preview true and the line to confirm', async () => {
    const run = await jsonRun(['tasks.stop', '--id', '8812'], config.dir)
    expect(run.code).toBe(0)
    expect(run.result).toMatchObject({
      status: 'ok',
      preview: true,
      suggest: 'tasks.stop --id 8812 --yes'
    })
    expect(probe.stops).toBe(0)
  })

  it('1: failed', async () => {
    const run = await jsonRun(['fail'], config.dir)
    expect(run.code).toBe(1)
    expect(run.result).toMatchObject({ status: 'error', code: 'failed' })
  })

  it('2: invalid-args from the tab', async () => {
    const run = await jsonRun(['search'], config.dir)
    expect(run.code).toBe(2)
    expect(run.result.code).toBe('invalid-args')
  })

  it('2: unknown-command in the CLI; the line never reaches the tab', async () => {
    const before = tab.heard.length
    const run = await jsonRun(['nope.command', '--x', '1'], config.dir)
    expect(run.code).toBe(2)
    expect(run.result.code).toBe('unknown-command')
    expect(tab.heard.length).toBe(before)
  })

  it('2: too-large data', async () => {
    const run = await jsonRun(['big', '--kb', '40'], config.dir)
    expect(run.code).toBe(2)
    expect(run.result.code).toBe('too-large')
  })

  it('2: a usage error of the CLI itself', async () => {
    const run = await jsonRun(['--bogus', 'search'], config.dir)
    expect(run.code).toBe(2)
    expect(run.result.code).toBe('usage')
  })

  it('3: a sensitive line never leaves the CLI', async () => {
    const before = tab.heard.length
    const run = await jsonRun(['cli.secret', 'hunter2'], config.dir)
    expect(run.code).toBe(3)
    expect(run.result.code).toBe('not-allowed')
    expect(tab.heard.length).toBe(before)
    expect(JSON.stringify(tab.heard)).not.toContain('hunter2')
  })

  it('3: the approval is rejected in the tab', async () => {
    const run = cli(['--json', 'tasks.stop', '--id', '8812', '--yes'], {
      configDir: config.dir
    })
    await run.waitStderr(/Onay bekleniyor/)
    await tab.rejectNext()
    const done = await run.done
    expect(done.code).toBe(3)
    expect(JSON.parse(done.stdout)).toMatchObject({ code: 'not-allowed' })
    expect(probe.stops).toBe(0)
  })

  it('0: the approved plan runs once', async () => {
    const run = cli(['tasks.stop', '--id', '8812', '--yes'], {
      configDir: config.dir
    })
    await run.waitStderr(
      /Onay bekleniyor: tarayıcıdaki Gecko sekmesinde onaylayın/
    )
    await tab.approveNext()
    const done = await run.done
    expect(done.code).toBe(0)
    expect(done.stdout).toBe('Görev 8812 durduruldu.\n')
    expect(probe.stops).toBe(1)
  })
})

describe('§6.2 channels and §6.7', () => {
  it('stdout carries the result only; info goes to stderr; no colour in a pipe', async () => {
    hidden = true
    const run = await cli(['search', '--ip', '1.1.1.1'], {
      configDir: config.dir,
      env: { NO_COLOR: '' }
    }).done
    expect(run.code).toBe(0)
    expect(run.stdout.startsWith('3 satır · 1.1.1.1\n')).toBe(true)
    expect(run.stdout).not.toContain('\u001b')
    expect(run.stderr).toContain('sekme arka planda, liste 42 sn eski')
  })

  it('received carries hidden and the data age into the JSON', async () => {
    hidden = true
    const run = await jsonRun(['search', '--ip', '1'], config.dir)
    expect(run.result).toMatchObject({ hidden: true, dataAgeMs: 42_000 })
  })

  it('rows print as a table; control characters in data are escaped', async () => {
    const run = await cli(['search', '--ip', '1'], { configDir: config.dir })
      .done
    expect(run.stdout).toContain('ip            bytes  note')
    expect(run.stdout).toContain('kaçış \\x1b[31mkırmızı')
    expect(run.stdout).not.toContain('\u001b')
  })

  it('--json is ASCII only: every non-ASCII character is \\uXXXX', async () => {
    const run = await jsonRun(['search', '--ip', 'ığüşöç'], config.dir)
    expect([...run.bytes].every(byte => byte < 0x80)).toBe(true)
    expect(run.stdout).toContain('\\u0131\\u011f')
    expect(run.result.summary).toBe('3 satır · ığüşöç')
  })

  it('an error in --json is a JSON object on stdout', async () => {
    const run = await jsonRun(['fail'], config.dir)
    expect(run.stdout.trim().startsWith('{')).toBe(true)
    expect(run.result.v).toBe(1)
  })

  it('a plain error goes to stderr, stdout stays empty', async () => {
    const run = await cli(['fail'], { configDir: config.dir }).done
    expect(run.code).toBe(1)
    expect(run.stdout).toBe('')
    expect(run.stderr).toContain('hata (failed)')
  })

  it('--out writes the result as UTF-8 to the file', async () => {
    const file = join(config.dir, 'out.txt')
    const run = await cli(['--out', file, 'search', '--ip', 'ığ'], {
      configDir: config.dir
    }).done
    expect(run.code).toBe(0)
    expect(run.stdout).toBe('')
    const bytes = readFileSync(file)
    expect(bytes[0]).not.toBe(0xff)
    expect(bytes.toString('utf8')).toContain('3 satır · ığ')
  })
})

describe('§6.3 chunks', () => {
  it('data in two chunks joins back', async () => {
    const run = await jsonRun(['big', '--kb', '20'], config.dir)
    expect(run.code).toBe(0)
    expect(run.result.complete).toBe(true)
    expect((run.result.data as { text: string }).text).toHaveLength(10 * 1024)
  })

  it('a missing chunk: complete false and no data', async () => {
    dropChunk = true
    const run = await jsonRun(['big', '--kb', '20'], config.dir)
    expect(run.result.complete).toBe(false)
    expect(run.result.data).toBeUndefined()
  })
})

describe('§10.1 arguments', () => {
  it('argv reaches the tab unchanged through quoteArgument', async () => {
    const values = ['a b', '', 'x"y', 'c:\\yol\\', 'ığ', "it's", '--', '-']
    const args = values.flatMap(value => ['--text', value])
    const run = await jsonRun(['echo', ...args], config.dir)
    expect(run.code).toBe(0)
    expect(probe.echoed).toEqual(values)
  })

  it('--line sends the line as is', async () => {
    const run = await jsonRun(
      ['--line', "echo --text 'a b' --text ''"],
      config.dir
    )
    expect(run.code).toBe(0)
    expect(probe.echoed).toEqual(['a b', ''])
  })

  it('--line - reads the line from stdin, past every shell', async () => {
    const run = await cli(['--json', '--line', '-'], {
      configDir: config.dir,
      stdin: `echo --text "x\\"y" --text 'ığ'\n`
    }).done
    expect(run.code).toBe(0)
    expect(probe.echoed).toEqual(['x"y', 'ığ'])
  })

  it('own options after the command name belong to the line', async () => {
    const run = await jsonRun(['echo', '--text', '--json'], config.dir)
    expect(run.code).toBe(0)
    expect(probe.echoed).toEqual(['--json'])
  })

  it('--view goes to the request', async () => {
    const run = await jsonRun(
      ['--view', 'v-2', 'search', '--ip', '1'],
      config.dir
    )
    expect(run.result.view).toMatchObject({ id: 'v-2' })
  })
})

describe('§4 help', () => {
  it('gecko help runs the tab’s help', async () => {
    const run = await jsonRun(['help'], config.dir)
    expect(run.code).toBe(0)
    expect(run.result.summary).toContain('tasks.stop')
  })
})
