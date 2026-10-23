import type {
  ScreenObservationStatus,
  TerminalScreenReadResult
} from '@shared/types/terminal-screen.types'
import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useScreenObservationStore } from '@/stores/screen-observation-store'
import { useTerminalStore } from '@/stores/terminal-store'
import type { Terminal } from '@/types/project'
import { useAgentTerminalState } from './use-agent-terminal-state'

const mocks = vi.hoisted(() => ({ read: vi.fn(), xterm: vi.fn(), log: vi.fn() }))
vi.mock('@/lib/terminal-api', () => ({ readTerminalScreen: mocks.read }))
vi.mock('@/lib/api', () => ({ terminalApi: { onOscTitleChanged: () => () => {} } }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: mocks.log }))
vi.mock('@/lib/agents/agent-screen-source', () => ({ readTerminalScreenTail: mocks.xterm }))

const ready: ScreenObservationStatus = {
  terminalScreenEngine: 'ghostty-vt',
  state: 'ready',
  reason: null,
  capturedSeq: 7,
  appliedSeq: 7,
  observerBuildId: 'test-build'
}
const terminal = {
  id: 'agent-1',
  ptyId: 'pty-1',
  claim: 'lease-1',
  name: 'Agent',
  kind: 'agent',
  agentId: 'claude-code',
  projectId: 'project-1',
  agentState: 'unknown'
} as Terminal

async function flush() {
  await act(async () => {
    await Promise.resolve()
    await Promise.resolve()
  })
}
function ghostty(status?: ScreenObservationStatus) {
  useScreenObservationStore
    .getState()
    .recordHost('pty-1', { terminalScreenEngine: 'ghostty-vt', status })
}
function reply(status = ready): TerminalScreenReadResult {
  return { status, text: 'esc to interrupt', appliedSeq: status.appliedSeq, truncated: false }
}

describe('explicit host screen Agent evidence', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.clearAllMocks()
    useTerminalStore.setState({ terminals: [{ ...terminal }] })
    useScreenObservationStore.setState({ byPtyId: {} })
    mocks.xterm.mockReturnValue('waiting for permission')
    mocks.read.mockResolvedValue({ success: true, data: reply() })
    mocks.log.mockResolvedValue(undefined)
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it('keeps existing/default inference and adds zero host calls', async () => {
    const { unmount } = renderHook(() => useAgentTerminalState())
    await act(async () => {
      await vi.advanceTimersByTimeAsync(4000)
    })
    expect(mocks.read).not.toHaveBeenCalled()
    expect(mocks.xterm).toHaveBeenCalled()
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('blocked')
    unmount()
  })

  it('uses consistent ready host text instead of the xterm cache', async () => {
    ghostty()
    const { unmount } = renderHook(() => useAgentTerminalState())
    await flush()
    expect(mocks.read).toHaveBeenCalledWith('pty-1', 'lease-1', 40)
    expect(mocks.xterm).not.toHaveBeenCalled()
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('working')
    expect(useScreenObservationStore.getState().byPtyId['pty-1'].status?.state).toBe('ready')
    unmount()
  })

  it.each([
    'starting',
    'catching-up',
    'degraded',
    'unsupported'
  ] as const)('does not infer activity or use xterm when host state is %s', async (state) => {
    ghostty()
    mocks.read.mockResolvedValue({
      success: true,
      data: reply({ ...ready, state, reason: state === 'degraded' ? 'overflow' : null })
    })
    const { unmount } = renderHook(() => useAgentTerminalState())
    await flush()
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('unknown')
    expect(mocks.xterm).not.toHaveBeenCalled()
    unmount()
  })

  it('rejects mismatched sequence evidence even if a host labels it ready', async () => {
    ghostty()
    mocks.read.mockResolvedValue({ success: true, data: reply({ ...ready, capturedSeq: 8 }) })
    const { unmount } = renderHook(() => useAgentTerminalState())
    await flush()
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('unknown')
    expect(useScreenObservationStore.getState().byPtyId['pty-1'].status?.state).toBe('degraded')
    unmount()
  })

  it('cannot let a late ready reply recover an already degraded observation', async () => {
    ghostty()
    let resolve!: (value: unknown) => void
    mocks.read.mockImplementation(
      () =>
        new Promise((done) => {
          resolve = done
        })
    )
    const { unmount } = renderHook(() => useAgentTerminalState())
    ghostty({ ...ready, state: 'degraded', reason: 'timeout' })
    resolve({ success: true, data: reply() })
    await flush()
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('unknown')
    expect(useScreenObservationStore.getState().byPtyId['pty-1'].status?.state).toBe('degraded')
    unmount()
  })

  it('discards a reply after the claim changes', async () => {
    ghostty()
    let resolve!: (value: unknown) => void
    mocks.read.mockImplementation(
      () =>
        new Promise((done) => {
          resolve = done
        })
    )
    const { unmount } = renderHook(() => useAgentTerminalState())
    useTerminalStore.setState({ terminals: [{ ...terminal, claim: 'lease-2' }] })
    resolve({ success: true, data: reply() })
    await flush()
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('unknown')
    expect(useScreenObservationStore.getState().byPtyId['pty-1'].status).toBeUndefined()
    unmount()
  })

  it('reports unavailable evidence without logging terminal text or transport errors', async () => {
    ghostty()
    mocks.read.mockRejectedValue(new Error('PRIVATE SCREEN CONTENT'))
    const { unmount } = renderHook(() => useAgentTerminalState())
    await flush()
    expect(useTerminalStore.getState().terminals[0].agentState).toBe('unknown')
    expect(JSON.stringify(mocks.log.mock.calls)).not.toContain('PRIVATE SCREEN CONTENT')
    expect(mocks.log).toHaveBeenCalled()
    unmount()
  })
})
