import { describe, expect, it } from 'vitest'

import { sendRetrying } from '../../src/cli/retry.ts'
import { HubError } from '../../src/shared/rpc.ts'
import type { Transport } from '../../src/transport/transport.ts'

const receipt = { stampedAt: 1, expiresAt: 2 }

function transportOf(
  answers: (HubError | null)[]
): Transport & { sent: number } {
  const t = {
    sent: 0,
    send: () => {
      const answer = answers[t.sent++] ?? null
      return answer ? Promise.reject(answer) : Promise.resolve(receipt)
    }
  }
  return t as unknown as Transport & { sent: number }
}

const limited = (ms: number) =>
  new HubError({ code: 'rate-limited', retryAfterMs: ms })

describe('sendRetrying', () => {
  it('waits retryAfterMs and sends again on rate-limited', async () => {
    const t = transportOf([limited(20), limited(20), null])
    await expect(sendRetrying(t, {})).resolves.toEqual(receipt)
    expect(t.sent).toBe(3)
  })

  it('gives up past the total wait', async () => {
    const t = transportOf([limited(60), limited(60), null])
    await expect(sendRetrying(t, {}, { maxWaitMs: 100 })).rejects.toMatchObject(
      {
        code: 'rate-limited'
      }
    )
    expect(t.sent).toBe(2)
  })

  it('never retries busy', async () => {
    const t = transportOf([new HubError({ code: 'busy' }), null])
    await expect(sendRetrying(t, {})).rejects.toMatchObject({ code: 'busy' })
    expect(t.sent).toBe(1)
  })
})
