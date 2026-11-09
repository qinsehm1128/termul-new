import type { AgentHookEvent } from '@shared/types/agent-hooks.types'
import { act, cleanup, renderHook, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useTerminalStore } from '@/stores/terminal-store'
import {
  applyAgentHookEvent,
  syncTerminalAgentSessions,
  useAgentSessionTracker
} from './use-agent-session-tracker'

const mocks = vi.hoisted(() => ({
  list: vi.fn(),
  detect: vi.fn(),
  events: vi.fn(),
  listen: vi.fn(),
  desktop: true
}))
vi.mock('@/lib/api', () => ({ terminalApi: { list: mocks.list } }))
vi.mock('@/lib/agent-hooks-api', () => ({ agentHooksApi: { events: mocks.events } }))
vi.mock('@/lib/tauri-event', () => ({ listen: mocks.listen }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => mocks.desktop }))
vi.mock('@/lib/agents/live-agent-sessions', async (original) => ({
  ...(await original<object>()),
  detectLiveAgentSessions: mocks.detect
}))
const event: AgentHookEvent = {
  sequence: 1,
  terminal_id: 'host-pty',
  project_id: 'project',
  agent: 'claude-code',
  session_id: 'sess-1',
  kind: 'needs_approval',
  at: 10
}
function terminal() {
  useTerminalStore.setState({
    terminals: [{ id: 'tab', ptyId: 'host-pty', projectId: 'project', title: 'shell' } as never]
  })
}
beforeEach(() => {
  vi.resetAllMocks()
  mocks.desktop = true
  terminal()
  mocks.detect.mockResolvedValue([])
  mocks.events.mockResolvedValue([])
  mocks.list.mockResolvedValue({ success: true, data: [{ id: 'host-pty', pid: 42 }] })
  mocks.listen.mockResolvedValue(vi.fn())
})
afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('hook-first session tracker', () => {
  it('uses host PTY/project identity and applies hook activity immediately', () => {
    applyAgentHookEvent({ ...event, project_id: 'other' })
    expect(useTerminalStore.getState().terminals[0].agentSession).toBeUndefined()
    applyAgentHookEvent(event)
    expect(useTerminalStore.getState().terminals[0]).toMatchObject({
      agentState: 'blocked',
      agentSession: { source: 'hook', hook_kind: 'needs_approval', sessionId: 'sess-1' }
    })
    applyAgentHookEvent({ ...event, kind: 'turn_done', sequence: 2 })
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('idle')
  })
  it('does not let process polling overwrite a hook session, then falls back after session_end', async () => {
    applyAgentHookEvent(event)
    mocks.detect.mockResolvedValue([
      { rootPid: 42, agentId: 'pi', sessionId: 'poll-sess', cwd: null, selfDev: false }
    ])
    await syncTerminalAgentSessions()
    expect(useTerminalStore.getState().terminals[0].agentSession?.source).toBe('hook')
    applyAgentHookEvent({ ...event, session_id: 'old-sess', kind: 'session_end' })
    expect(useTerminalStore.getState().terminals[0].agentSession?.source).toBe('hook')
    applyAgentHookEvent({ ...event, kind: 'session_end' })
    await syncTerminalAgentSessions()
    expect(useTerminalStore.getState().terminals[0].agentSession).toMatchObject({
      source: 'poll',
      sessionId: 'poll-sess'
    })
  })
  it('restores hook evidence from initial web event catchup without desktop subscriptions', async () => {
    mocks.desktop = false
    mocks.events.mockResolvedValue([event])
    const hook = renderHook(() => useAgentSessionTracker())
    await waitFor(() =>
      expect(useTerminalStore.getState().terminals[0].agentSession?.source).toBe('hook')
    )
    expect(mocks.events).toHaveBeenCalledWith(0)
    expect(mocks.listen).not.toHaveBeenCalled()
    hook.unmount()
  })
  it('does not overwrite a newer live event with an in-flight older snapshot', async () => {
    let resolve!: (value: unknown) => void
    mocks.detect.mockReturnValue(
      new Promise((done) => {
        resolve = done
      })
    )
    const pending = syncTerminalAgentSessions()
    await Promise.resolve()
    applyAgentHookEvent({ ...event, kind: 'tool_start' })
    resolve([
      {
        rootPid: 42,
        agentId: event.agent,
        sessionId: event.session_id,
        source: 'hook',
        hook_kind: 'needs_approval'
      }
    ])
    await pending
    expect(useTerminalStore.getState().terminals[0].agentSession?.hook_kind).toBe('tool_start')
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('working')
  })
  it('rejects malformed session ids rather than constructing unsafe resume argv', () => {
    applyAgentHookEvent({ ...event, session_id: 'x; touch /bad' })
    expect(useTerminalStore.getState().terminals[0].agentSession).toBeUndefined()
  })
  it.each([true, false])('preserves hook-derived live fallback for desktop=%s', async (desktop) => {
    mocks.desktop = desktop
    mocks.detect.mockResolvedValue([
      {
        rootPid: 42,
        agentId: 'pi',
        sessionId: 'live-hook',
        cwd: null,
        source: 'hook',
        hook_kind: 'needs_approval',
        transcript_path: '/host/log.jsonl'
      }
    ])
    await syncTerminalAgentSessions()
    expect(useTerminalStore.getState().terminals[0]).toMatchObject({
      agentState: 'blocked',
      agentSession: {
        source: 'hook',
        sessionId: 'live-hook',
        hook_kind: 'needs_approval',
        transcript_path: '/host/log.jsonl'
      }
    })
  })
  it('keeps legacy discovery running when hook commands are unavailable', async () => {
    mocks.events.mockRejectedValue(new Error('unknown command'))
    mocks.detect.mockResolvedValue([
      { rootPid: 42, agentId: 'pi', sessionId: 'legacy', cwd: null, selfDev: false }
    ])
    const hook = renderHook(() => useAgentSessionTracker())
    await waitFor(() =>
      expect(useTerminalStore.getState().terminals[0].agentSession?.sessionId).toBe('legacy')
    )
    expect(useTerminalStore.getState().terminals[0].agentSession?.source).toBe('poll')
    hook.unmount()
  })
  it('catches web session switches within 750ms without waiting for slow process discovery', async () => {
    vi.useFakeTimers()
    mocks.desktop = false
    mocks.list.mockReturnValue(new Promise(() => {}))
    mocks.events
      .mockResolvedValueOnce([])
      .mockResolvedValue([{ ...event, session_id: 'after-clear', kind: 'session_start' }])
    const hook = renderHook(() => useAgentSessionTracker())
    await act(async () => {
      await Promise.resolve()
      await Promise.resolve()
      await Promise.resolve()
    })
    await act(async () => vi.advanceTimersByTimeAsync(749))
    expect(mocks.events).toHaveBeenCalledTimes(1)
    await act(async () => vi.advanceTimersByTimeAsync(1))
    expect(mocks.events).toHaveBeenCalledTimes(2)
    expect(useTerminalStore.getState().terminals[0].agentSession?.sessionId).toBe('after-clear')
    expect(mocks.list).toHaveBeenCalledTimes(1)
    hook.unmount()
  })
  it('recovers lower host-generation sequences and releases old live hook guards', async () => {
    vi.useFakeTimers()
    mocks.desktop = false
    mocks.events.mockResolvedValueOnce([{ ...event, sequence: 100 }])
    const hook = renderHook(() => useAgentSessionTracker())
    await act(async () => vi.advanceTimersByTimeAsync(0))
    useTerminalStore.setState((state) => ({
      terminals: [
        ...state.terminals,
        {
          ...state.terminals[0],
          id: 'other-tab',
          ptyId: 'other-pty',
          agentSession: { ...state.terminals[0].agentSession!, sessionId: 'obsolete' }
        },
        {
          ...state.terminals[0],
          id: 'poll-tab',
          ptyId: 'poll-pty',
          agentSession: { ...state.terminals[0].agentSession!, source: 'poll' }
        }
      ]
    }))
    mocks.events.mockResolvedValueOnce([
      { ...event, sequence: 2, session_id: 'new-generation', kind: 'tool_start' },
      { ...event, sequence: 1, session_id: 'new-generation', kind: 'session_start' }
    ])
    await act(async () => vi.advanceTimersByTimeAsync(750))
    expect(mocks.events).toHaveBeenLastCalledWith(100)
    expect(useTerminalStore.getState().terminals[0]).toMatchObject({
      agentState: 'working',
      agentSession: { source: 'hook', sessionId: 'new-generation', hook_kind: 'tool_start' }
    })
    expect(useTerminalStore.getState().terminals[1].agentSession).toBeUndefined()
    expect(useTerminalStore.getState().terminals[1].agentState).toBe('idle')
    expect(useTerminalStore.getState().terminals[2].agentSession?.source).toBe('poll')
    await act(async () => vi.advanceTimersByTimeAsync(750))
    expect(mocks.events).toHaveBeenLastCalledWith(2)
    hook.unmount()
  })
  it('does not infer restart from empty or equal-sequence recovery batches', async () => {
    vi.useFakeTimers()
    mocks.desktop = false
    mocks.events
      .mockResolvedValueOnce([{ ...event, sequence: 100 }])
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([{ ...event, sequence: 100, session_id: 'duplicate' }])
    const hook = renderHook(() => useAgentSessionTracker())
    await act(async () => vi.advanceTimersByTimeAsync(0))
    await act(async () => vi.advanceTimersByTimeAsync(1500))
    expect(mocks.events).toHaveBeenLastCalledWith(100)
    expect(useTerminalStore.getState().terminals[0]).toMatchObject({
      agentState: 'blocked',
      agentSession: { source: 'hook', sessionId: 'sess-1' }
    })
    hook.unmount()
  })
  it('does not mistake an older catchup batch for restart after a concurrent live event', async () => {
    vi.useFakeTimers()
    let receive!: (value: { payload: AgentHookEvent }) => void
    let resolve!: (value: AgentHookEvent[]) => void
    mocks.listen.mockImplementation(async (_name, cb) => {
      receive = cb
      return vi.fn()
    })
    mocks.events.mockResolvedValueOnce([{ ...event, sequence: 50 }])
    const hook = renderHook(() => useAgentSessionTracker())
    await act(async () => vi.advanceTimersByTimeAsync(0))
    mocks.events.mockReturnValueOnce(
      new Promise<AgentHookEvent[]>((done) => {
        resolve = done
      })
    )
    await act(async () => vi.advanceTimersByTimeAsync(750))
    expect(mocks.events).toHaveBeenLastCalledWith(50)
    act(() =>
      receive({ payload: { ...event, sequence: 100, session_id: 'live', kind: 'tool_start' } })
    )
    await act(async () =>
      resolve([{ ...event, sequence: 60, session_id: 'older', kind: 'turn_done' }])
    )
    expect(useTerminalStore.getState().terminals[0]).toMatchObject({
      agentState: 'working',
      agentSession: { sessionId: 'live', hook_kind: 'tool_start' }
    })
    await act(async () => vi.advanceTimersByTimeAsync(750))
    expect(mocks.events).toHaveBeenLastCalledWith(100)
    hook.unmount()
  })
  it('retries initial catchup after a transport failure and still recognizes a restarted host', async () => {
    vi.useFakeTimers()
    mocks.desktop = false
    mocks.events
      .mockResolvedValueOnce([{ ...event, sequence: 100 }])
      .mockRejectedValueOnce(new Error('Failed to fetch'))
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([
        { ...event, sequence: 1, session_id: 'after-reconnect', kind: 'tool_start' }
      ])
    const hook = renderHook(() => useAgentSessionTracker())
    await act(async () => vi.advanceTimersByTimeAsync(0))
    await act(async () => vi.advanceTimersByTimeAsync(750))
    expect(mocks.events).toHaveBeenLastCalledWith(100)
    expect(useTerminalStore.getState().terminals[0].agentSession?.sessionId).toBe('sess-1')
    await act(async () => vi.advanceTimersByTimeAsync(750))
    expect(mocks.events).toHaveBeenLastCalledWith(0)
    await act(async () => vi.advanceTimersByTimeAsync(750))
    expect(mocks.events).toHaveBeenLastCalledWith(0)
    expect(useTerminalStore.getState().terminals[0]).toMatchObject({
      agentState: 'working',
      agentSession: { sessionId: 'after-reconnect' }
    })
    await act(async () => vi.advanceTimersByTimeAsync(750))
    expect(mocks.events).toHaveBeenLastCalledWith(1)
    hook.unmount()
  })
  it.each([
    'unknown command',
    'Command agent_hook_events not found',
    'HTTP 404 Not Found'
  ])('preserves the cursor and polling fallback on unsupported old host: %s', async (message) => {
    vi.useFakeTimers()
    mocks.events
      .mockResolvedValueOnce([{ ...event, sequence: 100, kind: 'session_end' }])
      .mockRejectedValue(new Error(message))
    mocks.detect.mockResolvedValue([
      { rootPid: 42, agentId: 'pi', sessionId: 'legacy', cwd: null, selfDev: false }
    ])
    const hook = renderHook(() => useAgentSessionTracker())
    await act(async () => vi.advanceTimersByTimeAsync(0))
    await act(async () => vi.advanceTimersByTimeAsync(1500))
    expect(mocks.events).toHaveBeenLastCalledWith(100)
    expect(useTerminalStore.getState().terminals[0].agentSession).toMatchObject({
      source: 'poll',
      sessionId: 'legacy'
    })
    hook.unmount()
  })
  it('ignores a lower-generation request that resolves after unmount', async () => {
    vi.useFakeTimers()
    let resolve!: (value: AgentHookEvent[]) => void
    mocks.events.mockResolvedValueOnce([{ ...event, sequence: 100 }])
    const hook = renderHook(() => useAgentSessionTracker())
    await act(async () => vi.advanceTimersByTimeAsync(0))
    mocks.events.mockReturnValueOnce(
      new Promise<AgentHookEvent[]>((done) => {
        resolve = done
      })
    )
    await act(async () => vi.advanceTimersByTimeAsync(750))
    hook.unmount()
    await act(async () =>
      resolve([{ ...event, sequence: 1, session_id: 'stale', kind: 'tool_start' }])
    )
    expect(useTerminalStore.getState().terminals[0]).toMatchObject({
      agentState: 'blocked',
      agentSession: { sessionId: 'sess-1' }
    })
  })
  it('deduplicates event catchup and live events and cleans up subscriptions', async () => {
    const stop = vi.fn()
    let receive: ((value: { payload: AgentHookEvent }) => void) | undefined
    mocks.listen.mockImplementation(async (_name, cb) => {
      receive = cb
      return stop
    })
    mocks.events.mockResolvedValue([event])
    const hook = renderHook(() => useAgentSessionTracker())
    await waitFor(() => expect(useTerminalStore.getState().terminals[0].agentState).toBe('blocked'))
    act(() => receive?.({ payload: { ...event, sequence: 2, kind: 'turn_done' } }))
    act(() => receive?.({ payload: event }))
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('idle')
    hook.unmount()
    expect(stop).toHaveBeenCalledOnce()
    act(() => receive?.({ payload: { ...event, sequence: 3 } }))
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('idle')
  })
})
