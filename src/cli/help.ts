// `gecko help [namespace|command]`: the bound tab's help (§4). The tab's own
// `help` command is not open to the CLI, so the CLI sends a `help-request`
// and the tab answers with its agent `help` tool's output, which filters by
// permission and pages. docs/browser-side.md has the envelopes.

import { randomUUID } from 'node:crypto'

import type { CapabilitiesReply } from '@dolusoft/command-protocol'

import { HubError } from '../shared/rpc.ts'
import type { HelpInput } from '../shared/wire.ts'
import { isHelpResult } from '../shared/wire.ts'
import type { Transport } from '../transport/transport.ts'
import { ownOption } from './args.ts'
import type { Result } from './output.ts'
import { errorResult, markUntrusted } from './output.ts'
import { commandOf } from './run.ts'
import { goneMessage } from './session.ts'

/** The `help` tool's input from `gecko help` arguments. */
export function helpInput(
  capabilities: CapabilitiesReply,
  args: readonly string[]
): HelpInput {
  const input: HelpInput = {}
  const valued = new Set(['--cursor', '--limit'])
  let target: string | undefined
  for (let i = 0; i < args.length; i++) {
    const token = args[i]
    if (valued.has(token)) {
      i++
      continue
    }
    if (token.startsWith('--')) continue
    target ??= token
  }
  if (target !== undefined) {
    // A command by name or alias gets its usage; anything else is a prefix.
    const spec = commandOf(capabilities, target)
    if (spec) input.command = spec.name
    else input.prefix = target
  }
  const cursor = ownOption(args, 'cursor')
  if (typeof cursor === 'string') input.cursor = cursor
  const limit = ownOption(args, 'limit')
  if (typeof limit === 'string' && /^\d+$/.test(limit)) {
    input.limit = Number(limit)
  }
  return input
}

/** The tab's help output as a CLI result. */
export function helpResult(input: HelpInput, output: unknown): Result {
  if (typeof output !== 'object' || output === null) {
    return errorResult('failed', 'sekmenin help cevabı okunamadı')
  }
  const record = output as Record<string, unknown>
  if (record.status === 'error') {
    const code = typeof record.code === 'string' ? record.code : 'failed'
    const message =
      code === 'unknown-command'
        ? `bilinmeyen komut: ${input.command ?? ''}. Komutlar için: gecko help`
        : code === 'no-terminal-access'
          ? 'bu hesabın terminal erişimi yok'
          : typeof record.message === 'string'
            ? record.message
            : code
    return errorResult(code, message)
  }
  let summary: string
  if (Array.isArray(record.commands)) {
    const scope = input.prefix ? `${input.prefix}: ` : ''
    summary = `${scope}${record.commands.length} komut`
    if (typeof record.nextCursor === 'string') {
      const target = input.prefix ? `${input.prefix} ` : ''
      summary += `; devamı: gecko help ${target}--cursor ${record.nextCursor}`
    }
  } else {
    const name = typeof record.name === 'string' ? record.name : ''
    const usage = typeof record.usage === 'string' ? ` · ${record.usage}` : ''
    summary = `${name}${usage}`
  }
  return markUntrusted(
    {
      v: 1,
      status: 'ok',
      summary,
      data: output,
      preview: false,
      complete: true,
      untrusted: []
    },
    false
  )
}

export interface HelpOptions {
  transport: Transport
  input: HelpInput
  /** Added to the stamp window before `not-delivered`. Default 2 s. */
  marginMs?: number
}

/** Sends one `help-request` and waits for its `help-result`. */
export function requestHelp(options: HelpOptions): Promise<Result> {
  const started = Date.now()
  const id = randomUUID()
  const { transport, input } = options
  return new Promise<Result>(resolve => {
    let ended = false
    let timer: NodeJS.Timeout | null = null
    const end = (result: Result) => {
      if (ended) return
      ended = true
      if (timer) clearTimeout(timer)
      off()
      resolve({ ...result, durationMs: Date.now() - started })
    }
    const off = transport.onReply(raw => {
      if (isHelpResult(raw) && raw.id === id) {
        end(helpResult(input, raw.result))
      }
    })
    transport.onClose(() =>
      end(
        errorResult(
          'connection-lost',
          'hub bağlantısı koptu; help güvenle yeniden denenir'
        )
      )
    )
    transport.send({ v: 1, kind: 'help-request', id, input }).then(
      receipt => {
        if (ended) return
        timer = setTimeout(
          () =>
            end(
              errorResult(
                'not-delivered',
                'sekme help isteğine cevap vermedi; Gecko sayfasını yenileyip yeniden deneyin'
              )
            ),
          receipt.expiresAt - receipt.stampedAt + (options.marginMs ?? 2000)
        )
      },
      error => {
        if (error instanceof HubError) {
          if (error.code === 'gone') {
            return end(
              errorResult('gone', goneMessage(error.reason), {
                ...(error.reason ? { reason: error.reason } : {})
              })
            )
          }
          if (error.code === 'tab-unreachable') {
            return end(
              errorResult(
                'tab-unreachable',
                'bağlı sekmeye şu an ulaşılamıyor (bağlantısı düştü); 60 sn içinde dönerse bağlama sürer'
              )
            )
          }
          return end(errorResult(error.code, error.message))
        }
        end(errorResult('failed', String(error)))
      }
    )
  })
}
