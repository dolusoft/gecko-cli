import { describe, expect, it } from 'vitest'

import {
  asciiJson,
  errorResult,
  escapeControls,
  exitCodeOf,
  exitOf,
  formatData,
  plainText
} from '../../src/cli/output.ts'

describe('§6.4 the exit code table', () => {
  const table: [number, string[]][] = [
    [1, ['failed', 'target-not-found']],
    [
      2,
      ['invalid-args', 'unknown-command', 'too-large', 'unsupported-version']
    ],
    [3, ['not-allowed', 'no-terminal-access', 'pair-timeout']],
    [
      4,
      [
        'busy',
        'not-delivered',
        'tab-unreachable',
        'gone',
        'clock-skew',
        'no-binding',
        'connection-lost'
      ]
    ],
    [5, ['unknown', 'unknown-outcome']],
    [6, ['not-logged-in', 'token-expired', 'revoked', 'unauthorized']],
    [130, ['cancelled', 'cancelled-before-start', 'cancel-requested']]
  ]
  for (const [code, names] of table) {
    it(`${code}: ${names.join(', ')}`, () => {
      for (const name of names) expect(exitCodeOf(name)).toBe(code)
    })
  }

  it('an ok result, a preview too, is 0', () => {
    expect(
      exitOf({
        v: 1,
        status: 'ok',
        preview: true,
        complete: true,
        untrusted: []
      })
    ).toBe(0)
  })

  it('a code 5 result is unknown and incomplete', () => {
    expect(errorResult('unknown-outcome', 'x')).toMatchObject({
      status: 'unknown',
      complete: false
    })
  })
})

describe('§6.1 JSON and text', () => {
  it('asciiJson escapes every non-ASCII character and reads back the same', () => {
    const value = { a: 'ığüşöç İĞÜŞÖÇ → ✓ 😀', b: '\u007f' }
    const text = asciiJson(value)
    expect(/^[\x20-\x7e]*$/.test(text)).toBe(true)
    expect(JSON.parse(text)).toEqual(value)
  })

  it('escapeControls keeps line breaks and tabs, escapes the rest', () => {
    expect(escapeControls('a\u001b[31mb\nc\td\u0007\u009b')).toBe(
      'a\\x1b[31mb\nc\td\\x07\\x9b'
    )
  })

  it('rows print as a table', () => {
    expect(formatData([{ a: 1, b: 'ğ' }, { a: 22 }])).toBe(
      'a   b\n--  -\n1   ğ\n22'
    )
  })

  it('an object with an items array prints its fields and a table', () => {
    expect(
      formatData({ total: 2, items: [{ id: 1, n: 'x' }, { id: 2 }] })
    ).toBe('total: 2\nitems:\nid  n\n--  -\n1   x\n2')
  })

  it('a columns + rows grid (dataview.rows) prints as a table', () => {
    expect(
      formatData({
        total: 2,
        columns: ['ip', 'port'],
        rows: [
          ['10.0.0.1', 22],
          ['10.0.0.22', null]
        ]
      })
    ).toBe(
      'total: 2\nip         port\n---------  ----\n10.0.0.1   22\n10.0.0.22'
    )
  })

  it('columns without rows of arrays stay key: value', () => {
    expect(formatData({ columns: ['a'], rows: [{ a: 1 }] })).toBe(
      'columns: ["a"]\nrows:\na\n-\n1'
    )
  })

  it('an object without rows prints key: value, never raw JSON', () => {
    expect(formatData({ ok: true, who: { id: 'u1' }, list: [] })).toBe(
      'ok: true\nwho: {"id":"u1"}\nlist: (boş)'
    )
  })

  it('a cell is cut at 40 code points', () => {
    const text = formatData([{ a: 'ğ'.repeat(50) }])
    const row = text.split('\n')[2]
    expect(Array.from(row)).toHaveLength(40)
    expect(row.endsWith('…')).toBe(true)
  })

  it('columns that do not fit the terminal are dropped and named', () => {
    const text = formatData([{ a: 'x'.repeat(10), b: 'y'.repeat(10) }], {
      columns: 15
    })
    expect(text.split('\n')[0]).toBe('a')
    expect(text).toContain('+1 sütun sığmadı: b')
  })

  it('a preview names the line to confirm', () => {
    expect(
      plainText({
        v: 1,
        status: 'ok',
        summary: '1 görev durdurulacak',
        preview: true,
        suggest: 'tasks.stop --id 8812 --yes',
        complete: true,
        untrusted: []
      })
    ).toBe(
      'Önizleme: 1 görev durdurulacak. Çalıştırmak için: gecko tasks.stop --id 8812 --yes\n'
    )
  })

  it('an incomplete result says so', () => {
    expect(
      plainText({
        v: 1,
        status: 'ok',
        preview: false,
        complete: false,
        untrusted: []
      })
    ).toContain('eksik')
  })
})
