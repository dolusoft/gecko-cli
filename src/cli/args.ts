// The CLI's own options come before the command name (§10.1): what follows
// the name belongs to the line and goes through `quoteArgument` one token at
// a time, so nothing a shell passed is lost.

import { quoteArgument } from '@dolusoft/command-protocol'

export const ownCommands = [
  'login',
  'logout',
  'attach',
  'detach',
  'help',
  'status',
  'dev-hub'
] as const

export type OwnCommand = (typeof ownCommands)[number]

export interface GlobalOptions {
  json: boolean
  env?: string
  view?: string
  out?: string
  /** The line as given, or `-` for stdin. */
  line?: string
  version: boolean
}

export type Parsed =
  | {
      kind: 'own'
      command: OwnCommand
      options: GlobalOptions
      /** The arguments after the command name. */
      args: string[]
    }
  | { kind: 'line'; options: GlobalOptions; line: string | '-' }
  | {
      kind: 'usage'
      options: GlobalOptions
      /** The own command whose usage is asked; none is the CLI's. */
      command?: OwnCommand
    }
  | { kind: 'error'; options: GlobalOptions; message: string }

const valued = new Set(['--env', '--view', '--out', '--line'])

export function parseArgs(argv: readonly string[]): Parsed {
  const options: GlobalOptions = { json: false, version: false }
  let index = 0
  while (index < argv.length) {
    const token = argv[index]
    if (!token.startsWith('-') || token === '-') {
      break
    }
    if (token === '--') {
      index++
      break
    }
    const equals = token.indexOf('=')
    const name = equals > 0 ? token.slice(0, equals) : token
    if (name === '--json' && equals < 0) {
      options.json = true
      index++
      continue
    }
    if ((name === '--version' || name === '-v') && equals < 0) {
      options.version = true
      index++
      continue
    }
    if ((name === '--help' || name === '-h') && equals < 0) {
      return { kind: 'usage', options }
    }
    if (valued.has(name)) {
      let value: string | undefined
      if (equals > 0) {
        value = token.slice(equals + 1)
        index++
      } else {
        value = argv[index + 1]
        index += 2
      }
      if (value === undefined) {
        return { kind: 'error', options, message: `${name} needs a value` }
      }
      const key = name.slice(2) as 'env' | 'view' | 'out' | 'line'
      options[key] = value
      continue
    }
    return {
      kind: 'error',
      options,
      message: `unknown option ${name}; gecko's own options come before the command name`
    }
  }
  const rest = argv.slice(index)
  if (options.line !== undefined) {
    if (rest.length > 0) {
      return {
        kind: 'error',
        options,
        message: '--line takes the whole line; give no command after it'
      }
    }
    return { kind: 'line', options, line: options.line }
  }
  if (rest.length === 0) {
    return { kind: 'usage', options }
  }
  const [name, ...args] = rest
  if ((ownCommands as readonly string[]).includes(name)) {
    // `gecko <own> --help` prints that command's usage, starts nothing.
    if (args.some(isHelpFlag)) {
      return { kind: 'usage', options, command: name as OwnCommand }
    }
    const misplaced = misplacedOption(name, args, true)
    if (misplaced) return { kind: 'error', options, message: misplaced }
    return { kind: 'own', command: name as OwnCommand, options, args }
  }
  // `gecko <command> --help` alone is the tab's help of that command.
  if (args.length === 1 && isHelpFlag(args[0])) {
    return { kind: 'own', command: 'help', options, args: [name] }
  }
  const misplaced = misplacedOption(name, args, false)
  if (misplaced) return { kind: 'error', options, message: misplaced }
  return { kind: 'line', options, line: toLine(rest) }
}

/** CLI options that are never a tab command's field (UX U5). */
const cliOnly = new Set(['--json', '--out', '--line'])

/**
 * `gecko tasks.list --json`: a CLI option after the command name (UX U5).
 * The line is not moved; the message names the right order. In a line, a
 * token right after another flag is that flag's value (`--q --json` looks
 * for "--json") and is left alone, as is everything after `--`. `--view`
 * and `--env` may be a command's own fields and are never flagged there.
 */
function misplacedOption(
  name: string,
  args: readonly string[],
  own: boolean
): string | undefined {
  for (let i = 0; i < args.length; i++) {
    const token = args[i]
    if (token === '--') return undefined
    const flag = token.includes('=')
      ? token.slice(0, token.indexOf('='))
      : token
    const flagged =
      cliOnly.has(flag) ||
      (own && name !== 'login' && (flag === '--env' || flag === '--view'))
    if (!flagged) continue
    if (!own && i > 0 && args[i - 1].startsWith('-')) continue
    const valued = flag !== '--json' && !token.includes('=')
    const moved = valued ? [token, args[i + 1] ?? '<değer>'] : [token]
    const others = args.filter((_, n) => n !== i && !(valued && n === i + 1))
    const fixed = ['gecko', ...moved, name, ...others]
      .map(part => (part === 'gecko' ? part : quoteArgument(part)))
      .join(' ')
    return `CLI seçenekleri komuttan önce gelir: ${fixed}`
  }
  return undefined
}

function isHelpFlag(token: string): boolean {
  return token === '--help' || token === '-h'
}

/** The tokens as one line the tab's tokenizer reads back unchanged. */
export function toLine(tokens: readonly string[]): string {
  return tokens.map(token => quoteArgument(token)).join(' ')
}

/** The value of `--name` in an own command's arguments. */
export function ownOption(
  args: readonly string[],
  name: string
): string | boolean | undefined {
  for (let i = 0; i < args.length; i++) {
    const token = args[i]
    if (token === `--${name}`) {
      const next = args[i + 1]
      return next === undefined || next.startsWith('--') ? true : next
    }
    if (token.startsWith(`--${name}=`)) {
      return token.slice(name.length + 3)
    }
  }
  return undefined
}
