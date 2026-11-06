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
