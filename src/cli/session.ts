// One `gecko` call's connection: the token, the hub, the binding and the
// bound page's capability list (§4.1).

import type { CapabilitiesReply } from '@dolusoft/command-protocol'
import { isCliReply, remoteVersion } from '@dolusoft/command-protocol'

import { HubError } from '../shared/rpc.ts'
import type {
  BindingInfo,
  HubStatus,
  Transport
} from '../transport/transport.ts'
import { connect } from '../transport/select.ts'
import { CliFailure, currentToken, resolveEnv } from './auth.ts'
import { openBrowser } from './browser.ts'
import type { StoredToken } from './config.ts'
import {
  readCapabilities,
  readDefaultEnv,
  writeCapabilities
} from './config.ts'

export interface Io {
  info(text: string): void
}

export interface Session {
  env: string
  token: StoredToken
  transport: Transport
  status: HubStatus
}

const goneText: Record<string, string> = {
  replaced: 'bağlama başka bir sekmeye ya da CLI kurulumuna geçti',
  closed: 'bağlı sekme kapandı ya da yenilendi',
  detached: 'bağlantı kesildi',
  'logged-out': 'oturum kapatıldı',
  revoked: 'CLI yetkisi profilden iptal edildi',
  'suspended-timeout': 'bağlı sekme 60 sn içinde geri dönmedi'
}

export function goneMessage(reason: string | undefined): string {
  return `${goneText[reason ?? ''] ?? 'bağlama sona erdi'}; yeniden bağlamak için: gecko attach`
}

export function envOf(given: string | undefined): string {
  const env = given ?? readDefaultEnv()
  if (!env) {
    throw new CliFailure(
      'not-logged-in',
      'giriş yok: önce gecko login --env <ortam>'
    )
  }
  return env
}

/** Connects with the stored token; refreshes it once when the hub refuses it. */
export async function openSession(given: string | undefined): Promise<Session> {
  const env = envOf(given)
  const host = resolveEnv(env)
  let token = await currentToken(env)
  let transport: Transport
  try {
    transport = await connect({ host, token: token.accessToken })
  } catch (error) {
    if (!(error instanceof HubError) || error.code !== 'unauthorized') {
      throw toFailure(error)
    }
    token = await currentToken(env, true)
    try {
      transport = await connect({ host, token: token.accessToken })
    } catch (again) {
      throw toFailure(again)
    }
  }
  let status: HubStatus
  try {
    status = await transport.status()
  } catch (error) {
    await transport.close().catch(() => {})
    throw toFailure(error)
  }
  return { env, token, transport, status }
}

export interface BindingSteps {
  /** Whether a missing binding starts pairing (UX U2: a person at a TTY). */
  autoPair: boolean
  pair(): Promise<BindingInfo>
  capabilities(binding: BindingInfo): Promise<CapabilitiesReply>
  /** Said before pairing starts on its own. */
  onAutoPair?(): void
}

/**
 * The session's binding and its capability list. Every error path closes
 * the connection before it throws (UX U9): no path leaves it to the exit.
 */
export async function withBinding(
  session: Session,
  steps: BindingSteps
): Promise<{ binding: BindingInfo; caps: CapabilitiesReply }> {
  try {
    let binding = session.status.binding
    if (binding?.state === 'suspended') {
      throw new CliFailure(
        'tab-unreachable',
        'bağlı sekmeye şu an ulaşılamıyor (bağlantısı düştü); 60 sn içinde dönerse bağlama sürer'
      )
    }
    if (binding?.state === 'gone') {
      // No new tab on its own (§5.3): the CLI says why and offers attach.
      throw new CliFailure('gone', goneMessage(binding.reason), binding.reason)
    }
    if (!binding) {
      if (!steps.autoPair) {
        throw new CliFailure(
          'no-binding',
          'bağlı sekme yok; bağlamak için: gecko attach'
        )
      }
      steps.onAutoPair?.()
      binding = await steps.pair()
    }
    const caps = await steps.capabilities(binding)
    return { binding, caps }
  } catch (error) {
    await session.transport.close().catch(() => {})
    throw error
  }
}

export function toFailure(error: unknown): CliFailure {
  if (error instanceof CliFailure) {
    return error
  }
  if (error instanceof HubError) {
    switch (error.code) {
      case 'unauthorized':
        return new CliFailure(
          'unauthorized',
          'token geçersiz ya da yetki iptal edildi: gecko login'
        )
      case 'busy':
        return new CliFailure(
          'busy',
          'başka bir gecko çağrısı çalışıyor (aynı yetkiyle tek canlı bağlantı)'
        )
      case 'unreachable':
        return new CliFailure('unreachable', "hub'a ulaşılamadı")
      default:
        return new CliFailure(error.code, error.message)
    }
  }
  return new CliFailure('failed', String((error as Error)?.message ?? error))
}

/**
 * Pairs the grant with a tab (§5.2): a server code, the pairing page in the
 * browser, then the user's approval there. Two minutes at most.
 */
export async function pair(
  session: Session,
  io: Io,
  options: { open: boolean; signal: AbortSignal }
): Promise<BindingInfo> {
  const { transport } = session
  const code = await transport.pair()
  return new Promise<BindingInfo>((resolve, reject) => {
    const cleanups: (() => void)[] = []
    const done = (fn: () => void) => {
      for (const cleanup of cleanups) cleanup()
      fn()
    }
    cleanups.push(
      transport.onPaired(binding => done(() => resolve(binding))),
      transport.onPairRefused(refusal => {
        if (refusal === 'other-account') {
          // The code stays valid; the user can switch accounts (§5.5).
          io.info(
            'tarayıcıda başka hesap açık; kodu bu CLI hesabıyla açılmış sekmede onaylayın'
          )
          return
        }
        done(() =>
          reject(
            new CliFailure(
              refusal,
              refusal === 'no-terminal-access'
                ? 'bu hesapta terminal erişimi yok'
                : `bağlama reddedildi: ${refusal}`
            )
          )
        )
      })
    )
    const timer = setTimeout(
      () =>
        done(() =>
          reject(
            new CliFailure(
              'pair-timeout',
              'bağlama 2 dakika içinde onaylanmadı'
            )
          )
        ),
      // The hub's clock decides; capped at the 2 minutes of §5.2.
      Math.min(Math.max(code.expiresAt - Date.now(), 1000), 120_000) + 1000
    )
    cleanups.push(() => clearTimeout(timer))
    const abort = () =>
      done(() => reject(new CliFailure('cancelled', 'iptal edildi')))
    options.signal.addEventListener('abort', abort, { once: true })
    cleanups.push(() => options.signal.removeEventListener('abort', abort))
    transport.onClose(info =>
      done(() =>
        reject(
          info.code === 4410
            ? new CliFailure('revoked', 'CLI yetkisi iptal edildi')
            : new CliFailure('connection-lost', 'hub bağlantısı koptu')
        )
      )
    )

    const opened = options.open && openBrowser(code.url)
    io.info(
      opened
        ? `Kod: ${code.code}  ·  tarayıcıda Gecko açıldı; "Bu sekme CLI'ya bağlansın mı?" sorusunu onaylayın`
        : `Kod: ${code.code}  ·  şu adresi açın: ${code.url}`
    )
    io.info(
      `(ya da açık bir Gecko sekmesinin terminalinde: cli.attach ${code.code})`
    )
  })
}

/**
 * The capability list of the bound page: from the hub, from the cache of
 * the same page, or as the tab sends it after pairing (up to `waitMs`).
 */
export async function capabilities(
  session: Session,
  binding: BindingInfo,
  waitMs: number
): Promise<CapabilitiesReply> {
  const fresh = readable(binding.capabilities, binding.bindingId)
  if (fresh) {
    writeCapabilities(session.env, {
      pageId: binding.pageId,
      bindingId: binding.bindingId,
      capabilities: fresh
    })
    return fresh
  }
  const cached = readCapabilities(session.env)
  if (cached && cached.pageId === binding.pageId) {
    const value = readable(cached.capabilities, binding.bindingId)
    if (value) {
      return value
    }
  }
  const received = await new Promise<CapabilitiesReply | undefined>(resolve => {
    const timer = setTimeout(() => {
      off()
      resolve(undefined)
    }, waitMs)
    const settle = (value: CapabilitiesReply | undefined) => {
      if (value) {
        clearTimeout(timer)
        off()
        resolve(value)
      }
    }
    const off = session.transport.onReply(reply =>
      settle(readable(reply, binding.bindingId))
    )
    // The reply may have come with the `paired` event, before this listener:
    // the hub keeps it, so ask once more after subscribing.
    session.transport.status().then(
      status =>
        settle(readable(status.binding?.capabilities, binding.bindingId)),
      () => {}
    )
  })
  if (!received) {
    throw new CliFailure(
      'unsupported-version',
      'bağlı sekme CLI komutlarını tanımıyor; Gecko sayfasını yenileyin'
    )
  }
  writeCapabilities(session.env, {
    pageId: binding.pageId,
    bindingId: binding.bindingId,
    capabilities: received
  })
  return received
}

function readable(
  value: unknown,
  bindingId: string
): CapabilitiesReply | undefined {
  if (
    isCliReply(value) &&
    value.kind === 'capabilities' &&
    value.bindingId === bindingId
  ) {
    if (value.envelope !== remoteVersion) {
      return undefined
    }
    return value
  }
  return undefined
}
