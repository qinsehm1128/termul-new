import type { HookStateDto, HookStateSyncResponse } from '@shared/types/agent-hooks.types'
import { act, cleanup, renderHook, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { getAgentHookRuntimeState, useAgentHookStateStore } from '@/stores/agent-hook-state-store'
import { useTerminalStore } from '@/stores/terminal-store'
import {
  applyAgentHookStateSync,
  syncTerminalAgentSessions,
  useAgentSessionTracker
} from './use-agent-session-tracker'

const mocks = vi.hoisted(() => ({
  list: vi.fn(),
  detect: vi.fn(),
  subscribe: vi.fn(),
  unsubscribe: vi.fn()
}))
vi.mock('@/lib/api', () => ({ terminalApi: { list: mocks.list } }))
vi.mock('@/lib/agent-hooks-api', () => ({ agentHooksApi: { subscribeState: mocks.subscribe } }))
vi.mock('@/lib/agents/live-agent-sessions', async (original) => ({
  ...(await original<object>()),
  detectLiveAgentSessions: mocks.detect
}))
const state = (revision = 1, extra: Partial<HookStateDto> = {}): HookStateDto => ({
  instance_id: 'host',
  terminal_id: 'host-pty',
  project_id: 'project',
  generation: 1,
  adapter_install_id: 'install',
  agent_session_id: 'sess-1',
  state: 'busy',
  revision,
  restore_state: 'confirmed',
  identity: {
    adapter_id: 'claude-code',
    agent_session_id: 'sess-1',
    source: 'hook',
    descriptor_id: 'descriptor',
    cwd: '/repo',
    transcript_path: '/repo/session.jsonl',
    resume_argv: ['claude', '--resume', 'sess-1']
  },
  ...extra
})
function sync(row = state(), kind: 'snapshot' | 'delta' = 'snapshot'): HookStateSyncResponse {
  return {
    kind,
    instance_id: row.instance_id,
    current_revision: row.revision,
    states: [row],
    tombstones: []
  }
}
beforeEach(() => {
  vi.resetAllMocks()
  useAgentHookStateStore.getState().reset()
  useTerminalStore.setState({
    terminals: [{ id: 'tab', ptyId: 'host-pty', projectId: 'project', title: 'shell' } as never]
  })
  mocks.detect.mockResolvedValue([])
  mocks.list.mockResolvedValue({ success: true, data: [{ id: 'host-pty', pid: 42 }] })
  mocks.subscribe.mockReturnValue(mocks.unsubscribe)
})
afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('subscription-fed runtime and identity tracker', () => {
  it('uses immutable PTY/project identity and stores activity only in runtime', () => {
    applyAgentHookStateSync(sync(state(1, { project_id: 'other' })))
    expect(useTerminalStore.getState().terminals[0].agentSession).toBeUndefined()
    applyAgentHookStateSync(sync(state(2)))
    expect(useTerminalStore.getState().terminals[0].agentSession).toEqual({
      agentId: 'claude-code',
      sessionId: 'sess-1',
      source: 'hook',
      descriptor_ref: { descriptor_id: 'descriptor', generation: 1 },
      cwd: '/repo',
      transcript_path: '/repo/session.jsonl',
      resumeArgv: ['claude', '--resume', 'sess-1']
    })
    expect(useTerminalStore.getState().terminals[0].agentState).toBeUndefined()
    expect(getAgentHookRuntimeState('host-pty')?.state).toBe('busy')
  })
  it('100 runtime/tool changes never touch terminal identity or its store', () => {
    applyAgentHookStateSync(sync())
    const identity = useTerminalStore.getState().terminals[0].agentSession
    const changed = vi.fn()
    const unsubscribe = useTerminalStore.subscribe(changed)
    for (let revision = 2; revision <= 101; revision++) {
      applyAgentHookStateSync(
        sync(
          {
            ...state(revision),
            state: revision % 2 ? 'busy' : 'idle',
            hook_kind: 'tool_start',
            hook_at: revision,
            tool_name: `tool-${revision}`
          } as HookStateDto,
          'delta'
        )
      )
    }
    unsubscribe()
    expect(changed).not.toHaveBeenCalled()
    expect(useTerminalStore.getState().terminals[0].agentSession).toBe(identity)
    expect(getAgentHookRuntimeState('host-pty')).toMatchObject({
      revision: 101,
      tool_name: 'tool-101'
    })
  })
  it('keeps durable resume identity on ended state and tombstone', () => {
    applyAgentHookStateSync(sync())
    const identity = useTerminalStore.getState().terminals[0].agentSession
    applyAgentHookStateSync(sync(state(2, { state: 'ended' }), 'delta'))
    applyAgentHookStateSync({
      kind: 'delta',
      instance_id: 'host',
      current_revision: 3,
      states: [],
      tombstones: [{ instance_id: 'host', terminal_id: 'host-pty', revision: 3 }]
    })
    expect(getAgentHookRuntimeState('host-pty')).toBeUndefined()
    expect(useTerminalStore.getState().terminals[0].agentSession).toBe(identity)
  })
  it('ignores delayed runtime and retired host epochs without altering identity', () => {
    applyAgentHookStateSync(sync(state(5)))
    applyAgentHookStateSync(
      sync(state(4, { identity: { ...state().identity, agent_session_id: 'old' } }), 'delta')
    )
    expect(useTerminalStore.getState().terminals[0].agentSession?.sessionId).toBe('sess-1')
    applyAgentHookStateSync(sync(state(1, { instance_id: 'new-host' })))
    applyAgentHookStateSync(
      sync(state(6, { identity: { ...state().identity, agent_session_id: 'old' } }))
    )
    expect(useTerminalStore.getState().terminals[0].agentSession?.sessionId).toBe('sess-1')
    expect(useAgentHookStateStore.getState().instanceId).toBe('new-host')
  })
  it('process discovery cannot overwrite a confirmed hook identity', async () => {
    applyAgentHookStateSync(sync())
    mocks.detect.mockResolvedValue([{ rootPid: 42, agentId: 'pi', sessionId: 'poll', cwd: null }])
    await syncTerminalAgentSessions()
    expect(useTerminalStore.getState().terminals[0].agentSession?.sessionId).toBe('sess-1')
  })
  it('rechecks identity when an older process discovery completes in flight', async () => {
    let resolve!: (value: unknown) => void
    mocks.detect.mockReturnValue(
      new Promise((done) => {
        resolve = done
      })
    )
    const pending = syncTerminalAgentSessions()
    await Promise.resolve()
    applyAgentHookStateSync(sync())
    resolve([{ rootPid: 42, agentId: 'pi', sessionId: 'old', cwd: null }])
    await pending
    expect(useTerminalStore.getState().terminals[0].agentSession?.sessionId).toBe('sess-1')
  })
  it('rejects malformed identity ids without constructing resume metadata', () => {
    applyAgentHookStateSync(
      sync(state(1, { identity: { ...state().identity, agent_session_id: 'x; touch /bad' } }))
    )
    expect(useTerminalStore.getState().terminals[0].agentSession).toBeUndefined()
  })
  it('supports custom adapter identity and exact host display argv', () => {
    applyAgentHookStateSync(
      sync(
        state(1, {
          identity: {
            ...state().identity,
            adapter_id: 'custom-cli',
            resume_argv: ['custom-cli', 'reopen', 'sess-1']
          }
        })
      )
    )
    expect(useTerminalStore.getState().terminals[0].agentSession).toMatchObject({
      agentId: 'custom-cli',
      descriptor_ref: { descriptor_id: 'descriptor', generation: 1 },
      resumeArgv: ['custom-cli', 'reopen', 'sess-1']
    })
  })
  it('subscribes through B facade, tears down, and has only the old process discovery timer', async () => {
    vi.useFakeTimers()
    const hook = renderHook(() => useAgentSessionTracker())
    await act(async () => {
      await Promise.resolve()
    })
    expect(mocks.subscribe).toHaveBeenCalledTimes(1)
    expect(vi.getTimerCount()).toBe(1)
    const receive = mocks.subscribe.mock.calls[0][0] as (response: HookStateSyncResponse) => void
    act(() => receive(sync()))
    expect(getAgentHookRuntimeState('host-pty')?.state).toBe('busy')
    await act(async () => {
      vi.advanceTimersByTime(15_000)
    })
    expect(mocks.list).toHaveBeenCalledTimes(2)
    hook.unmount()
    expect(mocks.unsubscribe).toHaveBeenCalledOnce()
    expect(vi.getTimerCount()).toBe(0)
    receive(sync(state(2, { state: 'error' }), 'delta'))
    expect(getAgentHookRuntimeState('host-pty')?.revision).toBe(1)
  })
  it('keeps legacy process fallback available when no hook state is received', async () => {
    mocks.detect.mockResolvedValue([
      { rootPid: 42, agentId: 'codex', sessionId: 'legacy', cwd: null }
    ])
    const hook = renderHook(() => useAgentSessionTracker())
    await waitFor(() =>
      expect(useTerminalStore.getState().terminals[0].agentSession?.sessionId).toBe('legacy')
    )
    hook.unmount()
  })
})
