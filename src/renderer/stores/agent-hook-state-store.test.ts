import type { HookStateDto, HookStateSyncResponse } from '@shared/types/agent-hooks.types'
import { beforeEach, describe, expect, it } from 'vitest'
import {
  agentHookStateKey,
  getAgentHookRuntimeState,
  useAgentHookStateStore
} from './agent-hook-state-store'

const row = (revision: number, terminalId = 'pty', instanceId = 'host'): HookStateDto => ({
  instance_id: instanceId,
  terminal_id: terminalId,
  generation: 1,
  adapter_install_id: 'install',
  agent_session_id: 'session',
  project_id: 'project',
  state: 'idle',
  restore_state: 'confirmed',
  revision,
  identity: {
    adapter_id: 'pi',
    agent_session_id: 'session',
    transcript_path: '/repo/exact.jsonl',
    cwd: '/repo',
    source: 'hook',
    descriptor_id: 'descriptor',
    resume_argv: ['pi', '--session', '/repo/exact.jsonl']
  }
})
const apply = (response: HookStateSyncResponse): void =>
  useAgentHookStateStore.getState().applySync(response)
const snapshot = (
  instanceId: string,
  revision: number,
  states: HookStateDto[] = []
): HookStateSyncResponse => ({
  kind: 'snapshot',
  instance_id: instanceId,
  current_revision: revision,
  states,
  tombstones: []
})
const delta = (
  revision: number,
  states: HookStateDto[] = [],
  tombstones: { instance_id: string; terminal_id: string; revision: number }[] = []
): HookStateSyncResponse => ({
  kind: 'delta',
  instance_id: 'host',
  current_revision: revision,
  states,
  tombstones
})
beforeEach(() => useAgentHookStateStore.getState().reset())

describe('agent hook runtime store', () => {
  it('accepts an initial revision zero snapshot and global revision gaps', () => {
    apply(snapshot('host', 0))
    apply(delta(20, [row(3)]))
    expect(useAgentHookStateStore.getState().currentRevision).toBe(20)
    expect(getAgentHookRuntimeState('pty')?.revision).toBe(3)
  })
  it('drops stale envelopes and repeated/older per-terminal rows', () => {
    apply(snapshot('host', 10, [row(8)]))
    const before = getAgentHookRuntimeState('pty')
    apply(delta(9, [row(9)]))
    expect(getAgentHookRuntimeState('pty')).toBe(before)
    apply(delta(20, [row(7)]))
    expect(getAgentHookRuntimeState('pty')).toBe(before)
    expect(useAgentHookStateStore.getState().currentRevision).toBe(20)
  })
  it('retains tombstone revisions so a delayed row cannot resurrect a terminal', () => {
    apply(snapshot('host', 1, [row(1)]))
    apply(delta(4, [], [{ instance_id: 'host', terminal_id: 'pty', revision: 4 }]))
    apply(delta(8, [row(3)]))
    expect(getAgentHookRuntimeState('pty')).toBeUndefined()
    expect(useAgentHookStateStore.getState().revisions[agentHookStateKey('host', 'pty')]).toBe(4)
    apply(delta(9, [row(9)]))
    expect(getAgentHookRuntimeState('pty')?.revision).toBe(9)
  })
  it('orders multiple terminal updates and tombstones within a delta', () => {
    apply(snapshot('host', 0))
    apply(
      delta(
        12,
        [row(10), row(2), row(7)],
        [
          { instance_id: 'host', terminal_id: 'pty', revision: 8 },
          { instance_id: 'host', terminal_id: 'pty', revision: 12 }
        ]
      )
    )
    expect(getAgentHookRuntimeState('pty')).toBeUndefined()
    expect(useAgentHookStateStore.getState().revisions[agentHookStateKey('host', 'pty')]).toBe(12)
  })
  it('snapshot replaces authorized view and marks missing terminals removed', () => {
    apply(snapshot('host', 4, [row(1, 'first'), row(2, 'second')]))
    apply(snapshot('host', 10, [row(2, 'second')]))
    expect(getAgentHookRuntimeState('first')).toBeUndefined()
    expect(getAgentHookRuntimeState('second')?.revision).toBe(2)
    apply(delta(12, [row(6, 'first')]))
    expect(getAgentHookRuntimeState('first')).toBeUndefined()
  })
  it('host restart requires snapshot, clears prior runtime, rejects retired instance messages', () => {
    apply(snapshot('host', 100, [row(99)]))
    apply({
      kind: 'delta',
      instance_id: 'new',
      current_revision: 1,
      states: [row(1, 'new-pty', 'new')],
      tombstones: []
    })
    expect(useAgentHookStateStore.getState().instanceId).toBe('host')
    apply(snapshot('new', 1, [row(1, 'new-pty', 'new')]))
    expect(getAgentHookRuntimeState('pty')).toBeUndefined()
    expect(getAgentHookRuntimeState('new-pty')?.instance_id).toBe('new')
    apply(snapshot('host', 200, [row(200)]))
    expect(useAgentHookStateStore.getState().instanceId).toBe('new')
    expect(Object.keys(useAgentHookStateStore.getState().states)).toEqual([
      agentHookStateKey('new', 'new-pty')
    ])
  })
  it('rejects cross-instance rows/tombstones and future rows', () => {
    apply(snapshot('host', 1, [row(1)]))
    apply(
      delta(
        5,
        [row(2, 'foreign', 'other'), row(6, 'future')],
        [{ instance_id: 'other', terminal_id: 'pty', revision: 3 }]
      )
    )
    expect(getAgentHookRuntimeState('pty')?.revision).toBe(1)
    expect(getAgentHookRuntimeState('foreign')).toBeUndefined()
    expect(getAgentHookRuntimeState('future')).toBeUndefined()
  })
  it('has no persistence fields and reset clears runtime and old host knowledge', () => {
    apply(snapshot('host', 1, [row(1)]))
    useAgentHookStateStore.getState().reset()
    expect(useAgentHookStateStore.getState()).toMatchObject({
      instanceId: null,
      currentRevision: 0,
      states: {},
      revisions: {}
    })
  })
})
