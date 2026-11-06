// A client of the development hub's frames over the platform WebSocket. The
// CLI (Node 22+) and the browser tab use the same code; only Node can send
// headers, which is what the CLI needs (§8: the token goes in
// `Authorization`, never in the URL).

import type { EventFrame, HubErrorBody, ResultFrame } from './wire.ts'
import { parseFrame } from './wire.ts'

/** A refused call or connection, with the hub's code. */
export class HubError extends Error {
  readonly code: string
  readonly reason?: string
  constructor(body: HubErrorBody) {
    super(body.message ?? body.code)
    this.name = 'HubError'
    this.code = body.code
    this.reason = body.reason
  }
}

export interface RpcCloseInfo {
  code: number
  reason: string
}

export interface RpcConnection {
  invoke<T = unknown>(method: string, args?: unknown): Promise<T>
  on(event: string, listener: (payload: unknown) => void): () => void
  onClose(listener: (info: RpcCloseInfo) => void): void
  readonly closed: boolean
  close(): Promise<void>
}

export interface RpcOptions {
  url: string
  /** Node only: sent with the upgrade request. */
  headers?: Record<string, string>
  /** Ends the opening after this many ms. Default 10 s. */
  timeoutMs?: number
}

type Pending = {
  resolve: (value: unknown) => void
  reject: (error: Error) => void
}

/**
 * Opens the connection. Resolves once the hub said `ready`; rejects with a
 * `HubError` when the hub closes it first (its close code gives the cause).
 */
export function connectRpc(options: RpcOptions): Promise<RpcConnection> {
  const init = options.headers ? { headers: options.headers } : undefined
  // The second argument is undici's options object in Node; a browser never
  // gets headers.
  const socket = new WebSocket(
    options.url,
    init as unknown as string[] | undefined
  )
  const pending = new Map<number, Pending>()
  const listeners = new Map<string, Set<(payload: unknown) => void>>()
  const closeListeners: ((info: RpcCloseInfo) => void)[] = []
  let next = 0
  let closed = false

  const connection: RpcConnection = {
    invoke<T>(method: string, args?: unknown): Promise<T> {
      if (closed) {
        return Promise.reject(
          new HubError({ code: 'connection-lost', message: 'closed' })
        )
      }
      const id = ++next
      return new Promise<T>((resolve, reject) => {
        pending.set(id, { resolve: resolve as (v: unknown) => void, reject })
        socket.send(JSON.stringify({ t: 'invoke', id, method, args }))
      })
    },
    on(event, listener) {
      let set = listeners.get(event)
      if (!set) {
        set = new Set()
        listeners.set(event, set)
      }
      set.add(listener)
      return () => set.delete(listener)
    },
    onClose(listener) {
      closeListeners.push(listener)
    },
    get closed() {
      return closed
    },
    close() {
      if (closed) {
        return Promise.resolve()
      }
      return new Promise<void>(resolve => {
        closeListeners.push(() => resolve())
        socket.close(1000, 'bye')
      })
    }
  }

  return new Promise<RpcConnection>((resolve, reject) => {
    let ready = false
    const timer = setTimeout(() => {
      if (!ready) {
        socket.close()
        reject(new HubError({ code: 'unreachable', message: 'timeout' }))
      }
    }, options.timeoutMs ?? 10_000)
    socket.addEventListener('message', event => {
      const frame = parseFrame(String(event.data))
      if (!frame) {
        return
      }
      if (frame.t === 'result') {
        settle(frame)
      } else if (frame.t === 'event') {
        if (frame.name === 'ready' && !ready) {
          ready = true
          clearTimeout(timer)
          resolve(connection)
          return
        }
        emit(frame)
      }
    })
    socket.addEventListener('error', () => {
      // A close follows; the cause is read there.
    })
    socket.addEventListener('close', event => {
      closed = true
      clearTimeout(timer)
      const info = { code: event.code, reason: event.reason }
      for (const entry of pending.values()) {
        entry.reject(
          new HubError({ code: 'connection-lost', message: event.reason })
        )
      }
      pending.clear()
      if (!ready) {
        reject(closeError(info))
        return
      }
      for (const listener of closeListeners.splice(0)) {
        listener(info)
      }
    })
  })

  function settle(frame: ResultFrame) {
    const entry = pending.get(frame.id)
    if (!entry) {
      return
    }
    pending.delete(frame.id)
    if (frame.ok) {
      entry.resolve(frame.value)
    } else {
      entry.reject(new HubError(frame.error))
    }
  }

  function emit(frame: EventFrame) {
    for (const listener of [...(listeners.get(frame.name) ?? [])]) {
      listener(frame.payload)
    }
  }
}

/** The error a close before `ready` means. */
export function closeError(info: RpcCloseInfo): HubError {
  switch (info.code) {
    case 4401:
    case 4410:
      return new HubError({ code: 'unauthorized', message: info.reason })
    case 4403:
      return new HubError({ code: 'forbidden', message: info.reason })
    case 4409:
      return new HubError({ code: 'busy', message: info.reason })
    default:
      return new HubError({ code: 'unreachable', message: info.reason })
  }
}
