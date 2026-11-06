import { describe, expect, it } from 'vitest'

import { connectDev, isLocalHost } from '../../src/transport/dev.ts'
import { connect } from '../../src/transport/select.ts'
import {
  handshake,
  invocation,
  recordSeparator,
  splitRecords
} from '../../src/transport/signalr.ts'

describe('§10.4 the development transport is localhost only', () => {
  it('isLocalHost', () => {
    for (const host of [
      'localhost',
      'localhost:4400',
      '127.0.0.1:1',
      '[::1]:4400'
    ]) {
      expect(isLocalHost(host)).toBe(true)
    }
    for (const host of [
      'develop.example',
      '127.0.0.1.evil.com',
      '10.0.0.1:4400'
    ]) {
      expect(isLocalHost(host)).toBe(false)
    }
  })

  it('refuses another host even when forced', async () => {
    await expect(
      connectDev({ host: 'develop.example', token: 't' })
    ).rejects.toThrow(/localhost only/)
    await expect(
      connect({ host: 'develop.example', token: 't' }, 'dev')
    ).rejects.toThrow(/localhost only/)
  })

  it('another host goes to the SignalR skeleton', async () => {
    await expect(
      connect({ host: 'develop.example', token: 't' }, undefined)
    ).rejects.toThrow(/#1974/)
  })
})

describe('the SignalR framing', () => {
  it('handshake and invocation end with the record separator', () => {
    expect(handshake()).toBe(
      '{"protocol":"json","version":1}' + recordSeparator
    )
    expect(splitRecords(invocation('1', 'CliSend', [{ a: 1 }]))).toEqual([
      { type: 1, invocationId: '1', target: 'CliSend', arguments: [{ a: 1 }] }
    ])
  })

  it('splits several records of one frame', () => {
    expect(
      splitRecords(`{"type":6}${recordSeparator}{"type":7}${recordSeparator}`)
    ).toEqual([{ type: 6 }, { type: 7 }])
  })
})
