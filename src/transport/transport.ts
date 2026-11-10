// What the CLI says to the hub, whatever carries it (§10.4): the development
// hub over plain WebSocket now, the real hub over SignalR when
// the backend hub (#1974) lands. Only the transport changes then.

import type { CliReply } from '@dolusoft/command-protocol'

import type { BindingState, GoneReason } from '../shared/wire.ts'

export interface BindingInfo {
  bindingId: string
  state: BindingState
  reason?: GoneReason
  pageId: string
  approvedAt: number
  /** The tab's last capabilities reply, when the hub has one. */
  capabilities?: unknown
}

export interface HubStatus {
  user: { id: string; name: string }
  env: string
  grantId: string
  binding: BindingInfo | null
}

export interface PairCode {
  code: string
  expiresAt: number
  url: string
}

/** The stamp the hub wrote on a message (§4). */
export interface SendReceipt {
  stampedAt: number
  expiresAt: number
}

/** A CLI connection to the hub. Errors are `HubError`s with the hub's code. */
export interface Transport {
  status(): Promise<HubStatus>
  pair(): Promise<PairCode>
  /** A `cli-request` or `cancel`; the hub stamps and routes it. */
  send(envelope: Record<string, unknown>): Promise<SendReceipt>
  detach(): Promise<void>
  onReply(listener: (reply: CliReply | unknown) => void): () => void
  onBinding(listener: (binding: BindingInfo) => void): () => void
  onPaired(listener: (binding: BindingInfo) => void): () => void
  onPairRefused(listener: (code: string) => void): () => void
  /** The connection ended; `code` is the WebSocket close code. */
  onClose(listener: (info: { code: number; reason: string }) => void): void
  close(): Promise<void>
}

export interface ConnectOptions {
  /** The environment's host, `127.0.0.1:4400` or `develop.example`. */
  host: string
  /** The access token; sent in `Authorization`, never in the URL (§8). */
  token: string
}
