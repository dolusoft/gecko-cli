// The SignalR transport of the real hub (`command-relay/hub`, #1966).
// SKELETON: the backend side is gecko-project-backend#1974. What is known
// today is written here; the method names follow §10.3 and are not final.
//
// TODO(#1974): the hub method and event names, the negotiate address, and
// whether the hub accepts the token in `Authorization` on the WebSocket
// upgrade (§8: Node's WebSocket sends headers; the query string path stays
// closed for a CLI token). Then implement `connectSignalR` on the frames
// below and run the contract specs against it.

import type { ConnectOptions, Transport } from './transport.ts'

/** SignalR's record separator: every JSON message ends with it. */
export const recordSeparator = '\u001e'

/** The JSON hub protocol handshake, the first message on the socket. */
export function handshake(): string {
  return JSON.stringify({ protocol: 'json', version: 1 }) + recordSeparator
}

/** The messages of one received text frame. */
export function splitRecords(text: string): unknown[] {
  return text
    .split(recordSeparator)
    .filter(part => part !== '')
    .map(part => JSON.parse(part) as unknown)
}

/** A SignalR invocation (type 1) that expects a completion (type 3). */
export function invocation(
  invocationId: string,
  target: string,
  args: unknown[]
): string {
  return (
    JSON.stringify({ type: 1, invocationId, target, arguments: args }) +
    recordSeparator
  )
}

/** The planned names of §10.3 on the real hub; to be confirmed in #1974. */
export const plannedMethods = {
  status: 'CliStatus',
  pair: 'CliPair',
  send: 'CliSend',
  detach: 'CliDetach',
  sendToCli: 'SendToCli'
} as const

export async function connectSignalR(
  options: ConnectOptions
): Promise<Transport> {
  void options
  throw new Error(
    'The SignalR transport is not ready yet (gecko-project-backend#1974). Use the development hub: gecko dev-hub, then gecko login --env 127.0.0.1:4400.'
  )
}
