import { tokenize } from '@dolusoft/command-protocol'
import { describe, expect, it } from 'vitest'

import { ownOption, parseArgs, toLine } from '../../src/cli/args.ts'

describe('parseArgs (§10.1)', () => {
  it('reads the own options before the command name only', () => {
    expect(parseArgs(['--json', '--env', 'x.y', 'search', '--json'])).toEqual({
      kind: 'line',
      options: { json: true, env: 'x.y', version: false },
      line: 'search --json'
    })
  })

  it('takes --name=value', () => {
    const parsed = parseArgs(['--view=v-1', '--out=a.txt', 'search'])
    expect(parsed.options).toMatchObject({ view: 'v-1', out: 'a.txt' })
  })

  it('refuses an unknown option before the name', () => {
    expect(parseArgs(['--bogus', 'search']).kind).toBe('error')
  })

  it('--line takes the whole line and no command after it', () => {
    expect(parseArgs(['--line', '-'])).toMatchObject({
      kind: 'line',
      line: '-'
    })
    expect(parseArgs(['--line', 'a', 'b']).kind).toBe('error')
    expect(parseArgs(['--line']).kind).toBe('error')
  })

  it('own commands; help with a command goes to the tab', () => {
    expect(parseArgs(['attach', '--new'])).toMatchObject({
      kind: 'own',
      command: 'attach',
      args: ['--new']
    })
    expect(parseArgs(['help', 'tasks.stop'])).toMatchObject({
      kind: 'line',
      line: 'help tasks.stop'
    })
    expect(parseArgs([]).kind).toBe('usage')
    expect(parseArgs(['-h']).kind).toBe('usage')
  })

  it('-- ends the own options', () => {
    expect(parseArgs(['--', '--json'])).toMatchObject({
      kind: 'line',
      line: '--json'
    })
  })

  it('ownOption', () => {
    expect(ownOption(['--port', '9'], 'port')).toBe('9')
    expect(ownOption(['--new'], 'new')).toBe(true)
    expect(ownOption(['--env=a'], 'env')).toBe('a')
    expect(ownOption([], 'env')).toBeUndefined()
  })
})

describe('toLine: argv survives the tab tokenizer (DC-3 matrix)', () => {
  const cases = [
    ['--filter', 'a b', '', 'x"y', 'c:\\yol\\', 'ığ', '--'],
    ["it's", 'say "hi"', '\\', '\\"', 'tab\there', 'new\nline', "'", '"']
  ]
  for (const tokens of cases) {
    it(JSON.stringify(tokens), () => {
      const parsed = tokenize(toLine(['cmd', ...tokens]))
      expect(parsed.ok).toBe(true)
      expect(parsed.tokens.map(token => token.value)).toEqual([
        'cmd',
        ...tokens
      ])
    })
  }
})
