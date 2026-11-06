// The development transport: the development hub's JSON frames over one
// WebSocket. Refused for any host that is not localhost (§10.4).

import type { CliReply } from '@dolusoft/command-protocol'

import { connectRpc } from '../shared/rpc.ts'
import type {
  BindingInfo,
  ConnectOptions,
  HubStatus,
  PairCode,
  SendReceipt,
  Transport
} from './transport.ts'

const loopbackHost = /^(localhost|127\.0\.0\.1|\[::1\])(:\d+)?$/i

export function isLocalHost(host: string): boolean {
  return loopbackHost.test(host)
}

export async function connectDev(options: ConnectOptions): Promise<Transport> {
  if (!isLocalHost(options.host)) {
    throw new Error(
      `The development transport connects to localhost only, not ${options.host}.`
    )
  }
  const rpc = await connectRpc({
    url: `ws://${options.host}/hub`,
    headers: { Authorization: `Bearer ${options.token}` }
  })
  const on =
    <T>(name: string) =>
    (listener: (value: T) => void) =>
      rpc.on(name, payload => listener(payload as T))
  return {
    status: () => rpc.invoke<HubStatus>('status'),
    pair: () => rpc.invoke<PairCode>('pair'),
    send: envelope => rpc.invoke<SendReceipt>('send', { envelope }),
    detach: async () => {
      await rpc.invoke('detach')
    },
    onReply: on<CliReply | unknown>('reply'),
    onBinding: on<BindingInfo>('binding'),
    onPaired: on<BindingInfo>('paired'),
    onPairRefused: listener =>
      rpc.on('pair-refused', payload =>
        listener(String((payload as { code?: unknown }).code))
      ),
    onClose: listener => rpc.onClose(listener),
    close: () => rpc.close()
  }
}
