// The commands of the fake tab: one of each kind the contract tables need.

import type { CommandDefinition } from '@dolusoft/command-console-core'

const none = { type: 'object' as const, properties: {} }

export interface Probe {
  /** How often `tasks.stop` ran its change arm. */
  stops: number
  /** The last input of `echo`. */
  echoed: unknown
  /** Aborted signals seen by `slow`. */
  aborted: number
}

export function commands(probe: Probe): CommandDefinition[] {
  return [
    {
      name: 'search',
      aliases: ['s'],
      danger: 'read',
      input: {
        type: 'object',
        properties: { ip: { type: 'string' }, last: { type: 'string' } },
        required: ['ip']
      },
      run: request => ({
        summary: `3 satır · ${String(request.input?.ip)}`,
        data: [
          { ip: '172.16.40.25', bytes: 1200, note: 'ilk' },
          { ip: '172.16.40.26', bytes: 80, note: 'ığüşöç' },
          { ip: '10.0.0.1', bytes: 5, note: 'kaçış \u001b[31mkırmızı' }
        ]
      })
    },
    {
      name: 'echo',
      danger: 'read',
      input: {
        type: 'object',
        properties: { text: { type: 'array', items: { type: 'string' } } }
      },
      // The arguments as the tab parsed them, to check quoteArgument.
      run: request => {
        probe.echoed = request.input?.text
        return { summary: 'ok', data: request.input?.text ?? [] }
      }
    },
    {
      name: 'big',
      danger: 'read',
      input: {
        type: 'object',
        properties: { kb: { type: 'integer' } }
      },
      run: request => {
        const kb = Number(request.input?.kb ?? 20)
        return {
          summary: 'big',
          data: { text: 'ğ'.repeat((kb * 1024) / 2) }
        }
      }
    },
    {
      name: 'slow',
      danger: 'read',
      input: { type: 'object', properties: { ms: { type: 'integer' } } },
      run: (request, run) =>
        new Promise(resolve => {
          const timer = setTimeout(
            () => resolve({ summary: 'slow done' }),
            Number(request.input?.ms ?? 5000)
          )
          run.signal.addEventListener('abort', () => {
            probe.aborted++
            clearTimeout(timer)
            resolve({ summary: 'aborted' })
          })
        })
    },
    {
      name: 'fail',
      danger: 'read',
      input: none,
      run: () => {
        throw new Error('komut patladı')
      }
    },
    {
      name: 'tasks.stop',
      danger: 'change',
      confirm: { field: 'yes' },
      input: {
        type: 'object',
        properties: { id: { type: 'integer' }, yes: { type: 'boolean' } },
        required: ['id']
      },
      plan: input => ({
        targets: [String(input.id)],
        preview: `1 görev durdurulacak (${String(input.id)})`
      }),
      run: request => {
        if (request.input?.yes === true) {
          probe.stops++
          return { summary: `Görev ${String(request.input.id)} durduruldu.` }
        }
        return {
          summary: `1 görev durdurulacak (${String(request.input?.id)})`
        }
      }
    },
    {
      // UX U4 and U6: a tasks.* read with two UUID columns and an empty one.
      name: 'tasks.list',
      danger: 'read',
      input: none,
      run: () => ({
        summary: '2 görev',
        data: [
          {
            id: '6f1c2a9e-3b7d-4c55-9a10-2d4e8f7b1c03',
            workId: 'b2d47e10-8c3a-4f9b-a6e2-71c0d5e9f348',
            name: 'export',
            percent: null,
            runningMs: 454558943
          },
          {
            id: '0a9e4b21-5d6c-4e87-b3f0-9c2a1d7e6b54',
            workId: 'e5c13f88-2a4b-49d0-8e71-3f6b0c9a2d17',
            name: 'reindex',
            percent: null,
            runningMs: 61000
          }
        ]
      })
    },
    {
      // UX U1 and U9: where the tab is.
      name: 'nav.where',
      danger: 'read',
      input: none,
      run: () => ({
        summary: '/dashboards/default/Defaults/network-traffic',
        data: {
          path: '/dashboards/default/Defaults/network-traffic',
          title: 'Ağ Trafiği'
        }
      })
    },
    {
      // UX U5: one required string field.
      name: 'nav.find',
      danger: 'read',
      input: {
        type: 'object',
        properties: { text: { type: 'string' } },
        required: ['text']
      },
      run: request => ({ summary: `bulundu: ${String(request.input?.text)}` })
    },
    {
      name: 'cli.secret',
      sensitive: true,
      run: () => ({ summary: 'never' })
    }
  ]
}

export const newProbe = (): Probe => ({
  stops: 0,
  echoed: undefined,
  aborted: 0
})
