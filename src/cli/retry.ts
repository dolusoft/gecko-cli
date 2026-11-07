// A request the hub refused for the rate limit (`rate-limited`) is sent again
// after the hub's `retryAfterMs`, up to a total wait; a `busy` (another live
// call) is never retried.

import { HubError } from '../shared/rpc.ts'
import type { SendReceipt, Transport } from '../transport/transport.ts'

/** The most a call waits for the rate limit in all, in ms. */
export const RATE_WAIT_MAX_MS = 5000

export interface RetryOptions {
  /** Default {@link RATE_WAIT_MAX_MS}. */
  maxWaitMs?: number
  /** True once the call ended (Ctrl+C): no further try. */
  stopped?: () => boolean
}

/** `transport.send`, retried while the hub answers `rate-limited`. */
export async function sendRetrying(
  transport: Transport,
  envelope: Record<string, unknown>,
  options: RetryOptions = {}
): Promise<SendReceipt> {
  const maxWaitMs = options.maxWaitMs ?? RATE_WAIT_MAX_MS
  let waited = 0
  for (;;) {
    try {
      return await transport.send(envelope)
    } catch (error) {
      if (!(error instanceof HubError) || error.code !== 'rate-limited') {
        throw error
      }
      const wait = Math.max(1, error.retryAfterMs ?? 200)
      if (waited + wait > maxWaitMs || options.stopped?.()) {
        throw error
      }
      waited += wait
      await new Promise(resolve => setTimeout(resolve, wait))
      if (options.stopped?.()) throw error
    }
  }
}
