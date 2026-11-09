import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useScreenObservationStore } from '@/stores/screen-observation-store'
import { useTerminalStore } from '@/stores/terminal-store'
import type { Terminal } from '@/types/project'
import { useAgentTerminalState } from './use-agent-terminal-state'

const mocks = vi.hoisted(() => ({ xterm: vi.fn(), read: vi.fn() }))
vi.mock('@/lib/agents/agent-screen-source', () => ({ readTerminalScreenTail: mocks.xterm }))
vi.mock('@/lib/terminal-api', () => ({ readTerminalScreen: mocks.read }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn() }))
vi.mock('@/lib/api', () => ({ terminalApi: { onOscTitleChanged: () => () => {} } }))
const epoch = 1_800_000_000_000
function terminal(activity = epoch + 1): Terminal {
  return {
    id: 'tab',
    ptyId: 'pty',
    projectId: 'p',
    kind: 'agent',
    agentId: 'claude-code',
    claim: 'claim',
    lastActivityTimestamp: activity,
    agentSession: {
      agentId: 'claude-code',
      sessionId: 's',
      resumeArgv: ['claude', '--resume', 's'],
      source: 'hook',
      hook_kind: 'idle',
      hook_at: epoch
    }
  } as Terminal
}
beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(epoch)
  vi.clearAllMocks()
  mocks.xterm.mockReturnValue('esc to interrupt')
  useTerminalStore.setState({ terminals: [terminal()] })
  useScreenObservationStore.setState({ byPtyId: {} })
})
afterEach(() => vi.useRealTimers())
describe('stale hook activity precedence', () => {
  it('keeps fresh hooks authoritative, then permits changed screen state only after 120 seconds', async () => {
    const hook = renderHook(() => useAgentTerminalState())
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('idle')
    expect(mocks.xterm).not.toHaveBeenCalled()
    await act(async () => vi.advanceTimersByTimeAsync(120_000))
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('idle')
    await act(async () => vi.advanceTimersByTimeAsync(600))
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('working')
    hook.unmount()
  })
  it('never treats mere hook silence without new output as screen-change evidence', async () => {
    useTerminalStore.setState({ terminals: [terminal(epoch)] })
    const hook = renderHook(() => useAgentTerminalState())
    await act(async () => vi.advanceTimersByTimeAsync(180_000))
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('idle')
    expect(mocks.xterm).not.toHaveBeenCalled()
    hook.unmount()
  })
  it('keeps stale hook state when screen evidence is unknown, and renews priority on new hook evidence', async () => {
    vi.setSystemTime(epoch + 180_000)
    mocks.xterm.mockReturnValue(null)
    const hook = renderHook(() => useAgentTerminalState())
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('idle')
    mocks.xterm.mockReturnValue('esc to interrupt')
    await act(async () => vi.advanceTimersByTimeAsync(600))
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('working')
    const current = useTerminalStore.getState().terminals[0]
    useTerminalStore.getState().setTerminalAgentSession('tab', {
      ...current.agentSession!,
      hook_at: Date.now(),
      hook_kind: 'needs_approval'
    })
    await act(async () => vi.advanceTimersByTimeAsync(600))
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('blocked')
    hook.unmount()
  })
  it('allows stale hooks to yield to ready host screen evidence without consulting xterm', async () => {
    vi.setSystemTime(epoch + 180_000)
    const status = {
      terminalScreenEngine: 'ghostty-vt' as const,
      state: 'ready' as const,
      reason: null,
      capturedSeq: 1,
      appliedSeq: 1,
      observerBuildId: 'test'
    }
    useScreenObservationStore
      .getState()
      .recordHost('pty', { terminalScreenEngine: 'ghostty-vt', status })
    mocks.read.mockResolvedValue({
      success: true,
      data: { status, text: 'esc to interrupt', appliedSeq: 1, truncated: false }
    })
    const hook = renderHook(() => useAgentTerminalState())
    await act(async () => {
      await Promise.resolve()
      await Promise.resolve()
    })
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('working')
    expect(mocks.xterm).not.toHaveBeenCalled()
    hook.unmount()
  })
})
