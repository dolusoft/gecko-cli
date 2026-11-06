import { connectDev, isLocalHost } from './dev.ts'
import { connectSignalR } from './signalr.ts'
import type { ConnectOptions, Transport } from './transport.ts'

/**
 * The transport of an environment: the development hub for localhost, the
 * real hub otherwise. `GECKO_TRANSPORT=dev` on another host is refused: a
 * production build never reaches a development hub (§10.4).
 */
export function connect(
  options: ConnectOptions,
  forced = process.env.GECKO_TRANSPORT
): Promise<Transport> {
  if (forced === 'dev' || isLocalHost(options.host)) {
    return connectDev(options)
  }
  return connectSignalR(options)
}
