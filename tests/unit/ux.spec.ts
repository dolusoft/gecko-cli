import type { CapabilitiesReply } from '@dolusoft/command-protocol'
import { describe, expect, it } from 'vitest'

import { parseArgs } from '../../src/cli/args.ts'
import { formatData, humanDuration } from '../../src/cli/output.ts'
import { approxLeft, seconds } from '../../src/cli/run.ts'
import type { Session } from '../../src/cli/session.ts'
import { withBinding } from '../../src/cli/session.ts'
import { distance, nearCommands, positionalFix } from '../../src/cli/suggest.ts'
import type { BindingInfo, Transport } from '../../src/transport/transport.ts'

const caps: CapabilitiesReply = {
  v: 1,
  kind: 'capabilities',
  bindingId: 'b1',
  envelope: 1,
  commands: [
    { name: 'tasks.list', aliases: ['tl'], danger: 'read' },
    { name: 'tasks.stop', aliases: [], danger: 'change', confirm: 'yes' },
    { name: 'nav.find', aliases: [], danger: 'read' }
  ],
  sensitive: ['tasks.lsx']
}

describe('U5 suggestions', () => {
  it('Damerau-Levenshtein counts a swap as one', () => {
    expect(distance('tasks.lst', 'tasks.list')).toBe(1)
    expect(distance('tsaks.list', 'tasks.list')).toBe(1)
    expect(distance('ığ', 'ğı')).toBe(1)
  })

  it('at most three names within 2, nearest first; no sensitive ones', () => {
    expect(nearCommands(caps, 'tasks.lst')).toEqual(['tasks.list'])
    expect(nearCommands(caps, 'tasks.sto')).toEqual(['tasks.stop'])
    expect(nearCommands(caps, 'xyz')).toEqual([])
    expect(nearCommands(caps, 'tl.')).toEqual(['tasks.list'])
  })

  it('bare words for the single required string field', () => {
    const input = {
      properties: { text: { type: 'string' }, limit: { type: 'integer' } },
      required: ['text']
    }
    expect(positionalFix('nav.find data view', input)).toBe(
      "nav.find --text 'data view'"
    )
    expect(positionalFix('nav.find --text a b', input)).toBeUndefined()
    expect(positionalFix('nav.find', input)).toBeUndefined()
    expect(
      positionalFix('x a', {
        properties: { n: { type: 'integer' } },
        required: ['n']
      })
    ).toBeUndefined()
  })

  it('a CLI option after the command name is a usage error with the order', () => {
    expect(parseArgs(['tasks.list', '--json'])).toMatchObject({
      kind: 'error',
      message: 'CLI seçenekleri komuttan önce gelir: gecko --json tasks.list'
    })
    expect(
      parseArgs(['tasks.list', '--limit', '5', '--out', 'a.txt'])
    ).toMatchObject({
      kind: 'error',
      message:
        'CLI seçenekleri komuttan önce gelir: gecko --out a.txt tasks.list --limit 5'
    })
    // A flag's value, `--view` and anything after `--` belong to the line.
    expect(parseArgs(['syslogs.search', '--q', '--json']).kind).toBe('line')
    expect(parseArgs(['exports.list', '--view', 'v1']).kind).toBe('line')
    expect(parseArgs(['echo', '--', '--json']).kind).toBe('line')
    expect(parseArgs(['attach', '--json']).kind).toBe('error')
    expect(parseArgs(['login', '--env', 'x']).kind).toBe('own')
    expect(parseArgs(['status']).kind).toBe('own')
  })
})

describe('U6 tables', () => {
  const a = '6f1c2a9e-3b7d-4c55-9a10-2d4e8f7b1c03'
  const b = 'b2d47e10-8c3a-4f9b-a6e2-71c0d5e9f348'

  it('ids are never cut; an empty column is hidden and named', () => {
    const text = formatData(
      [
        { id: a, name: 'export', percent: null, note: '' },
        { id: b, name: 'x', percent: null, note: '' }
      ],
      { columns: 100 }
    )
    expect(text).toContain(a)
    expect(text).toContain(b)
    expect(text).not.toMatch(/percent\s/)
    expect(text).toContain('(boş sütunlar gizlendi: percent, note)')
  })

  it('0 and false are not empty', () => {
    const text = formatData([{ n: 0, f: false }])
    expect(text.split('\n')[0]).toBe('n  f')
  })

  it('ids past half the terminal go vertical', () => {
    const text = formatData([{ id: a, workId: b, name: 'export' }], {
      columns: 100
    })
    expect(text).toBe(`id      ${a}\nworkId  ${b}\nname    export`)
  })

  it('nested values: [n], member names, {…}', () => {
    const text = formatData([
      {
        list: [1, 2, 3],
        members: [
          { value: 's1', title: 'Güvenlik duvarı' },
          { value: 's2', title: 'Proxy' }
        ],
        who: { id: 'u1' }
      }
    ])
    const row = text.split('\n')[2]
    expect(row).toContain('[3]')
    expect(row).toContain('Güvenlik duvarı, Proxy')
    expect(row).toContain('{…}')
  })

  it('known duration fields in words; startMs stays a number', () => {
    expect(humanDuration(454558943)).toBe('5 g 6 sa')
    expect(humanDuration(61000)).toBe('1 dk 1 sn')
    expect(humanDuration(850)).toBe('850 ms')
    const text = formatData([{ runningMs: 454558943, startMs: 1762560000000 }])
    expect(text).toContain('5 g 6 sa')
    expect(text).toContain('1762560000000')
  })
})

describe('U3 wait texts', () => {
  it('seconds with a comma; approval time left rounded', () => {
    expect(seconds(1234)).toBe('1,2 sn')
    expect(approxLeft(119_000)).toBe('~2 dk')
    expect(approxLeft(44_100)).toBe('~45 sn')
    expect(approxLeft(-5)).toBe('~0 sn')
  })
})

describe('U9 every error path closes the connection', () => {
  const binding = (state: BindingInfo['state']): BindingInfo => ({
    bindingId: 'b1',
    state,
    pageId: 'p1',
    approvedAt: 0
  })
  const session = (b: BindingInfo | null) => {
    const closed = { count: 0 }
    const transport = {
      close: async () => {
        closed.count++
      }
    } as unknown as Transport
    const value: Session = {
      env: 'e',
      token: {} as Session['token'],
      transport,
      status: {
        user: { id: 'u', name: 'u' },
        env: 'e',
        grantId: 'g',
        binding: b
      }
    }
    return { value, closed }
  }
  const steps = {
    autoPair: false,
    pair: () => Promise.resolve(binding('active')),
    capabilities: () => Promise.resolve(caps)
  }

  it.each([
    ['suspended', binding('suspended'), steps, 'tab-unreachable'],
    ['gone', binding('gone'), steps, 'gone'],
    ['no binding, no TTY', null, steps, 'no-binding'],
    [
      'pairing fails',
      null,
      { ...steps, autoPair: true, pair: () => Promise.reject(new Error('x')) },
      undefined
    ],
    [
      'capabilities fail',
      binding('active'),
      { ...steps, capabilities: () => Promise.reject(new Error('y')) },
      undefined
    ]
  ] as const)('%s', async (_, b, given, code) => {
    const { value, closed } = session(b)
    const error = await withBinding(value, given).catch(e => e as Error)
    expect(error).toBeInstanceOf(Error)
    if (code) expect(error).toMatchObject({ code })
    expect(closed.count).toBe(1)
  })

  it('a good path leaves the connection to its caller', async () => {
    const { value, closed } = session(binding('active'))
    await withBinding(value, steps)
    expect(closed.count).toBe(0)
  })
})
