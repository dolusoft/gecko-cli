import type { CapabilitiesReply } from '@dolusoft/command-protocol'
import { describe, expect, it } from 'vitest'

import { confirms, precheck } from '../../src/cli/run.ts'

const caps: CapabilitiesReply = {
  v: 1,
  kind: 'capabilities',
  bindingId: 'b1',
  envelope: 1,
  commands: [
    { name: 'search', aliases: ['s'], danger: 'read' },
    { name: 'tasks.stop', aliases: [], danger: 'change', confirm: 'yes' },
    { name: 'wipe.now', aliases: [], danger: 'change' }
  ],
  sensitive: ['login', 'signin']
}

describe('precheck (§4.1)', () => {
  it('a sensitive name stops in the CLI, by alias too', () => {
    expect(precheck(caps, 'signin hunter2')).toMatchObject({
      ok: false,
      result: { code: 'not-allowed' }
    })
  })

  it('an unknown command stops in the CLI; help passes', () => {
    expect(precheck(caps, 'nope')).toMatchObject({
      ok: false,
      result: { code: 'unknown-command' }
    })
    expect(precheck(caps, 'help').ok).toBe(true)
  })

  it('an alias finds its command', () => {
    expect(precheck(caps, 's --ip 1')).toMatchObject({
      ok: true,
      change: false
    })
  })

  it('a change is a change only with its confirm field true', () => {
    expect(precheck(caps, 'tasks.stop --id 1')).toMatchObject({ change: false })
    expect(precheck(caps, 'tasks.stop --id 1 --yes')).toMatchObject({
      change: true
    })
    expect(precheck(caps, 'wipe.now')).toMatchObject({ change: true })
  })

  it('confirms', () => {
    expect(confirms(['--yes'], 'yes')).toBe(true)
    expect(confirms(['--yes', 'false'], 'yes')).toBe(false)
    expect(confirms(['--yes=true'], 'yes')).toBe(true)
    expect(confirms(['--id', '1'], 'yes')).toBe(false)
  })
})
