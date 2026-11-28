import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/lib/terminal-api', () => ({
  terminalApi: {
    closeView: vi.fn(async () => ({ success: true, data: undefined })),
    terminate: vi.fn(async () => ({ success: true, data: undefined })),
    spawn: vi.fn(),
    resume: vi.fn(),
    onCoreRestarted: vi.fn(() => () => {})
  }
}))

vi.mock('@/lib/log-api', () => ({
  logFrontendError: vi.fn(async () => undefined)
}))

import {
  HIDDEN_BUFFER_TRUNCATION_DELAY,
  TRUNCATED_BUFFER_SIZE,
  useTerminalStore
} from './terminal-store'

/**
 * `use-agent-terminal-state` re-derives every agent terminal's state on a
 * 600ms tick, whether or not anything happened. A write that changes nothing
 * must not notify: every store notification fans out to the plugin
 * host-context publisher, the session autosave debounce and every subscribed
 * component, so a no-op write is a permanent idle re-render storm.
 */
describe('terminal-store idle no-op writes', () => {
  beforeEach(() => {
    useTerminalStore.getState().commitTranscripts()
    useTerminalStore.setState({
      terminals: [
        { id: 't1', name: 'Terminal 1', projectId: '1', shell: 'bash', output: [] },
        { id: 't2', name: 'Terminal 2', projectId: '1', shell: 'bash', output: [] }
      ],
      activeTerminalId: 't1',
      recentTerminalIds: [],
      ptyIdIndex: new Map(),
      cleanupRecoveries: {}
    })
  })

  it('notifies once for a real change and not at all for a repeated value', () => {
    const listener = vi.fn()
    const unsubscribe = useTerminalStore.subscribe(listener)
    try {
      useTerminalStore.getState().setTerminalAgentState('t1', 'working')
      expect(listener).toHaveBeenCalledTimes(1)

      // Same value again — the 600ms idle tick re-deriving an unchanged state.
      useTerminalStore.getState().setTerminalAgentState('t1', 'working')
      useTerminalStore.getState().setTerminalAgentState('t1', 'working')
      expect(listener).toHaveBeenCalledTimes(1)

      // Unknown id must also be a no-op.
      useTerminalStore.getState().setTerminalAgentState('missing', 'idle')
      expect(listener).toHaveBeenCalledTimes(1)
    } finally {
      unsubscribe()
    }
  })

  it('notifies once for a real OSC title and not at all for a repainted title', () => {
    const listener = vi.fn()
    const unsubscribe = useTerminalStore.subscribe(listener)
    try {
      useTerminalStore.getState().setTerminalOscTitle('t1', 'claude')
      expect(listener).toHaveBeenCalledTimes(1)

      // A TUI repaints the same title on every spinner frame.
      useTerminalStore.getState().setTerminalOscTitle('t1', 'claude')
      useTerminalStore.getState().setTerminalOscTitle('missing', 'claude')
      expect(listener).toHaveBeenCalledTimes(1)
    } finally {
      unsubscribe()
    }
  })

  it('does not notify when hidden-buffer truncation trims nothing', () => {
    useTerminalStore.setState({
      terminals: [
        {
          id: 't1',
          name: 'Terminal 1',
          projectId: '1',
          shell: 'bash',
          output: [],
          isHidden: true,
          hiddenSince: Date.now() - HIDDEN_BUFFER_TRUNCATION_DELAY - 1_000,
          pendingScrollback: ['one', 'two']
        }
      ]
    })
    const listener = vi.fn()
    const unsubscribe = useTerminalStore.subscribe(listener)
    try {
      useTerminalStore.getState().truncateHiddenTerminalBuffers()
      expect(listener).not.toHaveBeenCalled()
    } finally {
      unsubscribe()
    }
  })

  it('notifies and trims once a hidden terminal exceeds the truncation size', () => {
    const oversized = Array.from({ length: TRUNCATED_BUFFER_SIZE + 50 }, (_, i) => String(i))
    useTerminalStore.setState({
      terminals: [
        {
          id: 't1',
          name: 'Terminal 1',
          projectId: '1',
          shell: 'bash',
          output: [],
          isHidden: true,
          hiddenSince: Date.now() - HIDDEN_BUFFER_TRUNCATION_DELAY - 1_000,
          pendingScrollback: oversized
        }
      ]
    })
    const listener = vi.fn()
    const unsubscribe = useTerminalStore.subscribe(listener)
    try {
      useTerminalStore.getState().truncateHiddenTerminalBuffers()
      expect(listener).toHaveBeenCalledTimes(1)
      expect(useTerminalStore.getState().terminals[0]?.pendingScrollback?.length).toBe(
        TRUNCATED_BUFFER_SIZE
      )
    } finally {
      unsubscribe()
    }
  })
})
