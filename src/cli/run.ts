// One line sent to the bound tab and its result (§4, §6): the pre-filter of
// the capability list, the stamp window, the replies, the chunks, the cancel
// of §6.6 and the outcomes of the edge cases in §5.5.

import { randomUUID } from 'node:crypto'

import type {
  CapabilitiesReply,
  ChunkReply,
  CliReply,
  FinishedReply,
  RemoteCommandSpec
} from '@dolusoft/command-protocol'
import {
  isCliReply,
  joinChunks,
  quoteArgument,
  tokenize
} from '@dolusoft/command-protocol'

import { HubError } from '../shared/rpc.ts'
import { closeCodes } from '../shared/wire.ts'
import type { Transport } from '../transport/transport.ts'
import type { Result } from './output.ts'
import { errorResult, markUntrusted } from './output.ts'
import { sendRetrying } from './retry.ts'
import { goneMessage } from './session.ts'
import { unknownCommandMessage } from './suggest.ts'

export interface Interrupts {
  /** Called on the first Ctrl+C; the second ends the wait. */
  onInterrupt(listener: (count: number) => void): () => void
}

export interface RunIo {
  info(text: string): void
  /**
   * The one-line wait indicator (UX U3); rewritten in place. Absent when
   * stderr is no TTY or `--json` is on: nothing is printed then.
   */
  progress?(text: string): void
  /** Removes the indicator line, if one is shown. */
  clearProgress?(): void
}

/** After this long a call shows the wait indicator (UX U3). */
export const progressAfterMs = 700

/** "1,2 sn": seconds with one decimal, Turkish style. */
export function seconds(ms: number): string {
  return `${(ms / 1000).toFixed(1).replace('.', ',')} sn`
}

/** "~2 dk", "~45 sn": the approximate time left of an approval. */
export function approxLeft(ms: number): string {
  const left = Math.max(0, ms)
  if (left >= 60_000) return `~${Math.round(left / 60_000)} dk`
  return `~${Math.ceil(left / 1000)} sn`
}

export interface RunOptions {
  transport: Transport
  capabilities: CapabilitiesReply
  line: string
  view?: string
  io: RunIo
  interrupts: Interrupts
  /** Added to the stamp window before `not-delivered`. Default 2 s. */
  marginMs?: number
  /** How long a cancel answer is awaited (§6.6: 3 s). */
  cancelWaitMs?: number
  /** Gives up after this long with `timeout` (`gecko status`: 2 s). */
  deadlineMs?: number
}

/** The command a line names, by name or alias. */
export function commandOf(
  capabilities: CapabilitiesReply,
  name: string
): RemoteCommandSpec | undefined {
  return capabilities.commands.find(
    spec => spec.name === name || spec.aliases.includes(name)
  )
}

/** Whether the line sets the command's confirm field to true (§7.1). */
export function confirms(tokens: readonly string[], field: string): boolean {
  for (let i = 0; i < tokens.length; i++) {
    const token = tokens[i]
    if (token === `--${field}`) {
      const next = tokens[i + 1]
      return next !== 'false'
    }
    if (token === `--${field}=true`) return true
  }
  return false
}

type Check =
  | { ok: true; spec: RemoteCommandSpec | undefined; change: boolean }
  | { ok: false; result: Result }

/**
 * The CLI's pre-filter (§4.1): a sensitive line never leaves the CLI, an
 * unknown command never reaches the tab. Not a permission: the tab decides.
 */
export function precheck(capabilities: CapabilitiesReply, line: string): Check {
  const parsed = tokenize(line)
  const tokens = parsed.tokens.map(token => token.value)
  const name = tokens[0]
  if (name === undefined || name === '') {
    return {
      ok: false,
      result: errorResult('invalid-args', 'boş satır')
    }
  }
  if (capabilities.sensitive.includes(name)) {
    return {
      ok: false,
      result: errorResult(
        'not-allowed',
        `${name} hassas bir komut; CLI'dan çalışmaz, tarayıcıda yazın`
      )
    }
  }
  const spec = commandOf(capabilities, name)
  // `help` is the tab's: its permission filter and paging decide (§4).
  if (!spec && name !== 'help') {
    const { message, near } = unknownCommandMessage(capabilities, name)
    return {
      ok: false,
      result: errorResult('unknown-command', message, {
        ...(near.length > 0
          ? {
              suggest: [near[0], ...tokens.slice(1)]
                .map(quoteArgument)
                .join(' ')
            }
          : {})
      })
    }
  }
  const change =
    spec?.danger === 'change' &&
    (spec.confirm === undefined || confirms(tokens.slice(1), spec.confirm))
  return { ok: true, spec, change }
}

export async function runLine(options: RunOptions): Promise<Result> {
  const started = Date.now()
  const check = precheck(options.capabilities, options.line)
  if (!check.ok) {
    return check.result
  }
  const { transport, io } = options
  const id = randomUUID()
  const envelope: Record<string, unknown> = {
    v: 1,
    kind: 'cli-request',
    id,
    line: options.line
  }
  if (options.view !== undefined) {
    envelope.view = options.view
  }

  return new Promise<Result>(resolve => {
    const chunks: ChunkReply[] = []
    let received: Extract<CliReply, { kind: 'received' }> | null = null
    let approving = false
    let change = check.change
    let ended = false
    let deliveryTimer: NodeJS.Timeout | null = null
    let cancelTimer: NodeJS.Timeout | null = null
    let cancelAnswer: 'cancelled-before-start' | 'cancel-requested' | null =
      null
    let interrupted = false
    const cleanups: (() => void)[] = []

    // The wait indicator (UX U3): one line from 700 ms on, gone at the end.
    let approvalExpiresAt: number | null = null
    const indicate = () => {
      if (ended || interrupted) return
      if (approvalExpiresAt !== null) {
        io.progress?.(
          `Onay bekleniyor (${approxLeft(approvalExpiresAt - Date.now())}): tarayıcıdaki Gecko sekmesinde onaylayın`
        )
        return
      }
      if (Date.now() - started < progressAfterMs) return
      io.progress?.(
        `${received ? 'sekmede çalışıyor' : 'sekmeye ulaşılıyor'}… ${seconds(Date.now() - started)}`
      )
    }
    const ticker = io.progress ? setInterval(indicate, 100) : null
    ticker?.unref()

    const end = (result: Result) => {
      if (ended) return
      ended = true
      if (deliveryTimer) clearTimeout(deliveryTimer)
      if (cancelTimer) clearTimeout(cancelTimer)
      if (deadlineTimer) clearTimeout(deadlineTimer)
      if (ticker) clearInterval(ticker)
      io.clearProgress?.()
      for (const cleanup of cleanups) cleanup()
      resolve({ ...result, durationMs: Date.now() - started })
    }
    const deadlineTimer =
      options.deadlineMs !== undefined
        ? setTimeout(
            () => end(errorResult('timeout', 'sekme zamanında cevap vermedi')),
            options.deadlineMs
          )
        : null

    const lost = (why: string) => {
      if (interrupted) {
        return end(cancelled('unknown'))
      }
      if (!received && !change) {
        // A read command is safe to run again whether it arrived or not.
        return end(
          errorResult(
            'connection-lost',
            `${why}; okuma komutu güvenle yeniden denenir`
          )
        )
      }
      if (!received) {
        // A changing command may be on its way (§5.5).
        return end(errorResult('unknown', `${why}; istek yolda olabilir`))
      }
      if (change) {
        return end(
          errorResult(
            'unknown-outcome',
            `${why}; istek ulaşmış olabilir, tarayıcıda kontrol edin`
          )
        )
      }
      return end(
        errorResult(
          'connection-lost',
          `${why}; okuma komutu güvenle yeniden denenir`
        )
      )
    }

    const cancelled = (
      code: 'cancelled-before-start' | 'cancel-requested' | 'unknown'
    ): Result =>
      errorResult(
        code,
        code === 'cancelled-before-start'
          ? 'komut çalışmadan iptal edildi'
          : code === 'cancel-requested'
            ? change
              ? 'iptal istendi; komut çalışıyordu, backend’de başlamış iş sürebilir'
              : 'iptal istendi; komutun yerel çalışması kesildi'
            : 'iptalin sonucu bilinmiyor',
        { status: 'cancelled', complete: false }
      )

    const finish = (reply: FinishedReply) => {
      if (
        interrupted &&
        reply.outcome.status === 'error' &&
        reply.outcome.code === 'cancelled'
      ) {
        return end(cancelled(cancelAnswer ?? 'cancel-requested'))
      }
      const base = {
        hidden: received?.hidden,
        dataAgeMs: received ? received.dataAgeMs : undefined,
        view: reply.view
      }
      if (reply.outcome.status === 'error') {
        const outcome = reply.outcome
        return end(
          markUntrusted(
            {
              ...errorResult(outcome.code, outcome.message ?? outcome.code),
              ...(outcome.line ? { suggest: outcome.line } : {}),
              ...strip(base)
            },
            outcome.message !== undefined
          )
        )
      }
      let data: unknown
      let complete = true
      if (reply.chunks > 0) {
        const text = joinChunks(reply, chunks)
        if (text === undefined) {
          complete = false
        } else {
          try {
            data = JSON.parse(text)
          } catch {
            complete = false
          }
        }
      }
      const preview = reply.preview === true
      const suggest =
        preview && check.spec?.confirm
          ? `${options.line} --${check.spec.confirm}`
          : undefined
      end(
        markUntrusted(
          {
            v: 1,
            status: 'ok',
            ...(reply.outcome.summary !== undefined
              ? { summary: reply.outcome.summary }
              : {}),
            ...(data !== undefined ? { data } : {}),
            ...strip(base),
            preview,
            ...(suggest ? { suggest } : {}),
            complete,
            untrusted: []
          },
          false
        )
      )
    }

    cleanups.push(
      transport.onReply(raw => {
        if (!isCliReply(raw) || !('id' in raw) || raw.id !== id) {
          return
        }
        switch (raw.kind) {
          case 'received':
            received = raw
            if (deliveryTimer) clearTimeout(deliveryTimer)
            if (raw.hidden) {
              // `dataAgeMs` is the age of the task snapshot whatever ran
              // (UX U4): only a `tasks.*` line gets the staleness part.
              const tasks = check.spec?.name.startsWith('tasks.') === true
              io.clearProgress?.()
              io.info(
                tasks && typeof raw.dataAgeMs === 'number'
                  ? `sekme arka planda, liste ${Math.round(raw.dataAgeMs / 1000)} sn eski`
                  : 'sekme arka planda'
              )
            }
            break
          case 'awaiting-approval': {
            approving = true
            change = true
            // The tab's clock: the time left is approximate (UX U3).
            // On a TTY the indicator line counts it down instead.
            approvalExpiresAt = raw.expiresAt
            if (io.progress) {
              indicate()
            } else {
              io.info(
                `Onay bekleniyor (${approxLeft(raw.expiresAt - Date.now())}): tarayıcıdaki Gecko sekmesinde onaylayın.`
              )
            }
            break
          }
          case 'chunk':
            chunks.push(raw)
            break
          case 'finished':
            finish(raw)
            break
          case 'cancelled-before-start':
          case 'cancel-requested':
            cancelAnswer = raw.kind
            break
        }
      }),
      transport.onBinding(binding => {
        if (binding.state === 'gone' && binding.reason === 'revoked') {
          end(revokedResult())
        } else if (binding.state === 'gone') {
          lost(goneMessage(binding.reason))
        } else if (binding.state === 'suspended') {
          lost('bağlı sekmenin bağlantısı düştü')
        }
      }),
      options.interrupts.onInterrupt(count => {
        if (count > 1) {
          // The second Ctrl+C ends the wait (§6.6, Fable B-15).
          return end(cancelled('unknown'))
        }
        interrupted = true
        io.clearProgress?.()
        transport
          .send({ v: 1, kind: 'cancel', id })
          .catch(() => end(cancelled('unknown')))
        cancelTimer = setTimeout(
          () => end(cancelled(cancelAnswer ?? 'unknown')),
          options.cancelWaitMs ?? 3000
        )
      })
    )
    transport.onClose(info => {
      if (info.code === closeCodes.revoked) {
        // The grant was revoked (§8): an identity error, whatever ran.
        return end(revokedResult())
      }
      lost('hub bağlantısı koptu')
    })

    sendRetrying(transport, envelope, {
      stopped: () => ended || interrupted
    }).then(
      receipt => {
        if (ended) return
        // The window is the hub's (`expiresAt - stampedAt`), measured on
        // this clock from the receipt: no clock comparison across machines.
        const windowMs = receipt.expiresAt - receipt.stampedAt
        deliveryTimer = setTimeout(
          () => {
            if (!received && !interrupted) {
              // Past `expiresAt` the tab does not run it (Fable B-6), but it
              // may have run it before, with its `received` lost on the way:
              // a changing command is then of unknown outcome, never safe to
              // run again (#10).
              if (change) {
                return end(
                  errorResult(
                    'unknown-outcome',
                    'sekmeden cevap gelmedi; istek çalışmış olabilir, tarayıcıda kontrol edin'
                  )
                )
              }
              end(
                errorResult(
                  'not-delivered',
                  'istek sekmeye ulaşmadı ve çalışmadı; güvenle yeniden denenir'
                )
              )
            }
          },
          windowMs + (options.marginMs ?? 2000)
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
          if (error.code === 'no-binding') {
            return end(errorResult('no-binding', 'bağlama yok: gecko attach'))
          }
          if (error.code === 'rate-limited') {
            // sendRetrying waited up to 5 s already.
            return end(
              errorResult(
                'busy',
                'hız sınırı: yetki başına saniyede en çok 5 istek; biraz sonra yeniden deneyin'
              )
            )
          }
          if (error.code === 'busy') {
            return end(errorResult('busy', 'terminal meşgul; yeniden deneyin'))
          }
          if (error.code === 'connection-lost') {
            return lost('hub bağlantısı koptu')
          }
          return end(errorResult(error.code, error.message))
        }
        end(errorResult('failed', String(error)))
      }
    )
    void approving
  })
}

function strip<T extends Record<string, unknown>>(value: T): Partial<T> {
  return Object.fromEntries(
    Object.entries(value).filter(([, v]) => v !== undefined)
  ) as Partial<T>
}

/** The grant was revoked (§8): an identity error, whatever was running. */
function revokedResult(): Result {
  return errorResult(
    'revoked',
    'CLI yetkisi iptal edildi; başlamış bir iş sürüyor olabilir. Yeniden: gecko login'
  )
}
