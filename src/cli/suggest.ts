// Typo help (UX U5): the commands near a mistyped name, and the line a
// single required string field would have taken. Suggestions only; nothing
// runs on its own.

import type { CapabilitiesReply } from '@dolusoft/command-protocol'
import { quoteArgument, tokenize } from '@dolusoft/command-protocol'

/** Damerau-Levenshtein distance (optimal string alignment), by code point. */
export function distance(a: string, b: string): number {
  const s = Array.from(a)
  const t = Array.from(b)
  const d: number[][] = []
  for (let i = 0; i <= s.length; i++) {
    d.push([i])
    for (let j = 1; j <= t.length; j++) d[i].push(i === 0 ? j : 0)
  }
  for (let i = 1; i <= s.length; i++) {
    for (let j = 1; j <= t.length; j++) {
      const cost = s[i - 1] === t[j - 1] ? 0 : 1
      d[i][j] = Math.min(
        d[i - 1][j] + 1,
        d[i][j - 1] + 1,
        d[i - 1][j - 1] + cost
      )
      if (i > 1 && j > 1 && s[i - 1] === t[j - 2] && s[i - 2] === t[j - 1]) {
        d[i][j] = Math.min(d[i][j], d[i - 2][j - 2] + 1)
      }
    }
  }
  return d[s.length][t.length]
}

/**
 * Up to three command names within distance 2 of `name`, nearest first.
 * Aliases count, the suggestion is the command's name. Sensitive names are
 * left out: the CLI would refuse them anyway.
 */
export function nearCommands(
  capabilities: CapabilitiesReply,
  name: string,
  max = 3
): string[] {
  const best = new Map<string, number>()
  for (const spec of capabilities.commands) {
    for (const candidate of [spec.name, ...spec.aliases]) {
      const d = distance(name, candidate)
      if (d <= 2 && d < (best.get(spec.name) ?? Infinity)) {
        best.set(spec.name, d)
      }
    }
  }
  return [...best.entries()]
    .sort((x, y) => x[1] - y[1] || x[0].localeCompare(y[0]))
    .slice(0, max)
    .map(([name]) => name)
}

/** The unknown-command message, with the near names when there are some. */
export function unknownCommandMessage(
  capabilities: CapabilitiesReply,
  name: string
): { message: string; near: string[] } {
  const near = nearCommands(capabilities, name)
  return {
    near,
    message:
      near.length > 0
        ? `bilinmeyen komut: ${name} · bunu mu demek istediniz: ${near.join(', ')}`
        : `bilinmeyen komut: ${name}. Komutlar için: gecko help`
  }
}

interface InputSchema {
  properties?: Record<string, { type?: unknown }>
  required?: unknown
}

/**
 * `nav.find data view` → `nav.find --text "data view"`: when the line has
 * no flag and the command's only required field is a string, the bare words
 * are that field's value. Undefined otherwise.
 */
export function positionalFix(
  line: string,
  input: unknown
): string | undefined {
  const tokens = tokenize(line).tokens.map(token => token.value)
  const [name, ...rest] = tokens
  if (name === undefined || rest.length === 0) return undefined
  if (rest.some(token => token.startsWith('-'))) return undefined
  if (typeof input !== 'object' || input === null) return undefined
  const schema = input as InputSchema
  const required = Array.isArray(schema.required)
    ? (schema.required as unknown[]).filter(
        (field): field is string => typeof field === 'string'
      )
    : []
  if (required.length !== 1) return undefined
  const field = required[0]
  if (schema.properties?.[field]?.type !== 'string') return undefined
  return `${name} --${field} ${quoteArgument(rest.join(' '))}`
}
