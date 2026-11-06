// The browser side of the development hub (`@dolusoft/gecko-cli/tab-client`):
// what a Gecko tab says to the hub. It runs in a browser and in Node 22+
// (the platform WebSocket), so frontendx's pages and tests and this
// repository's fake tab use the same calls. docs/browser-side.md describes
// the messages.

import type { CliReply } from '@dolusoft/command-protocol'

import type { RpcConnection } from '../shared/rpc.ts'
import { connectRpc } from '../shared/rpc.ts'
import type { BindingState, GoneReason } from '../shared/wire.ts'

export { HubError } from '../shared/rpc.ts'
export type { BindingState, GoneReason } from '../shared/wire.ts'

/** What the hub knows of a binding of this tab. */
export interface TabBinding {
  bindingId: string
  grantId: string
  approvedAt: number
}

export interface BindingEvent {
  bindingId: string
  state: BindingState
  reason?: GoneReason
}

export interface TabClientOptions {
  /** The hub's origin: `http://127.0.0.1:4400`. */
  hubUrl: string
  /** The browser session. The development hub takes `dev:<userId>`. */
  token: string
  /** This page load's id (§3); a new one on every load. */
  pageId: string
}

export interface TabClient {
  /** The binding of this page after `hello` (a return within 60 s), or `null`. */
  readonly binding: TabBinding | null
  /** Says the page id again, as after a reconnect (§5.3). */
  hello(): Promise<TabBinding | null>
  /** The server facts of a pairing code, without using it (§5.2). */
  pairInfo(code: string): Promise<{ grantCreatedAt: number; env: string }>
  /** Uses the code and binds this page; the user has approved it. */
  pair(code: string): Promise<TabBinding>
  /** `SendToCli`: a reply to the bound CLI, accepted from this page only. */
  sendToCli(reply: CliReply): Promise<{ delivered: boolean }>
  /** "Bağlantıyı kes" in the terminal: the binding is gone, `detached`. */
  detach(): Promise<void>
  /** The page closes or reloads: the binding is gone, `closed`. */
  unload(): Promise<void>
  /** The user logs out: every binding of the user is gone, `logged-out`. */
  logout(): Promise<void>
  /** Today's `Send`: to the user's other tabs, never to a CLI. */
  broadcast(envelope: unknown): Promise<{ delivered: number }>
  /** A stamped `cli-request` or `cancel`, as the hub routed it. */
  onCliMessage(listener: (raw: unknown) => void): () => void
  onBinding(listener: (event: BindingEvent) => void): () => void
  /** Another CLI connection of the bound grant was refused (§3). */
  onCliBusy(
    listener: (event: { grantId: string; at: number }) => void
  ): () => void
  onBroadcast(listener: (envelope: unknown) => void): () => void
  onClose(listener: () => void): void
  readonly rpc: RpcConnection
  close(): Promise<void>
}

/** Connects the tab and says `hello` with its page id. */
export async function connectTab(
  options: TabClientOptions
): Promise<TabClient> {
  const url = new URL('/hub', options.hubUrl.replace(/^http/, 'ws'))
  // A browser cannot send headers on a WebSocket: the session goes in the
  // query string, as today's hub does. A CLI token never may (§8).
  url.searchParams.set('access_token', options.token)
  const rpc = await connectRpc({ url: url.href })
  let binding: TabBinding | null = null
  rpc.on('binding', payload => {
    const event = payload as BindingEvent
    if (event.state === 'gone' && binding?.bindingId === event.bindingId) {
      binding = null
    }
  })
  const on =
    <T>(name: string) =>
    (listener: (value: T) => void) =>
      rpc.on(name, payload => listener(payload as T))
  const client: TabClient = {
    get binding() {
      return binding
    },
    get rpc() {
      return rpc
    },
    async hello() {
      const result = await rpc.invoke<{ binding: TabBinding | null }>('hello', {
        pageId: options.pageId
      })
      binding = result.binding
      return binding
    },
    pairInfo: code => rpc.invoke('pairInfo', { code }),
    async pair(code) {
      binding = await rpc.invoke<TabBinding>('pair', { code })
      return binding
    },
    sendToCli: reply => rpc.invoke('sendToCli', { reply }),
    async detach() {
      await rpc.invoke('detach')
      binding = null
    },
    async unload() {
      await rpc.invoke('unload')
      binding = null
    },
    async logout() {
      await rpc.invoke('logout')
      binding = null
    },
    broadcast: envelope => rpc.invoke('broadcast', { envelope }),
    onCliMessage: on<unknown>('cli-message'),
    onBinding: on<BindingEvent>('binding'),
    onCliBusy: on<{ grantId: string; at: number }>('cli-busy'),
    onBroadcast: on<unknown>('broadcast'),
    onClose: listener => rpc.onClose(() => listener()),
    close: () => rpc.close()
  }
  await client.hello()
  return client
}

/**
 * A `ConsoleGate` of command-console-core over the tab client: `subscribe`
 * hears the CLI's stamped messages and the user's other tabs, `send`
 * broadcasts, `schedule` is `setTimeout`.
 */
export function createTabGate(tab: TabClient, tabId: string) {
  return {
    tab: tabId,
    async send(envelope: unknown) {
      await tab.broadcast(envelope)
    },
    subscribe(listener: (raw: unknown) => void) {
      const offCli = tab.onCliMessage(listener)
      const offBroadcast = tab.onBroadcast(listener)
      return () => {
        offCli()
        offBroadcast()
      }
    },
    schedule(ms: number, fn: () => void) {
      const timer = setTimeout(fn, ms)
      return () => clearTimeout(timer)
    }
  }
}
