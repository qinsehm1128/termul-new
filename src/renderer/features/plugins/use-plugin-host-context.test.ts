import type { HostContextPublishRequest } from '@shared/types/plugin.types'
import { renderHook } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  desktop: true,
  restore: false,
  publish: vi.fn(),
  heartbeat: vi.fn(),
  ack: vi.fn(),
  pending: vi.fn()
}))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => mocks.desktop }))
vi.mock('@/hooks/useTerminalAutoSave', () => ({ isTerminalRestoreInProgress: () => mocks.restore }))
vi.mock('./plugin-api', () => ({
  pluginApi: { publishHostContext: mocks.publish, heartbeatHostContext: mocks.heartbeat }
}))
vi.mock('./host-ui-store', () => ({
  usePluginHostUiStore: {
    getState: () => ({ acknowledgeContext: mocks.ack, setContextPending: mocks.pending })
  }
}))

import { createElement, StrictMode } from 'react'
import { create } from 'zustand'

vi.mock('@/stores/project-store', () => ({
  useProjectStore: create(() => ({ projects: [], activeProjectId: '', isLoaded: true }))
}))
vi.mock('@/stores/terminal-store', () => ({ useTerminalStore: create(() => ({ terminals: [] })) }))
vi.mock('@/features/agent-session/stores/acp-store', () => ({
  useAcpStore: create(() => ({ sessions: {} }))
}))
vi.mock('@/features/quick-terminal/quick-terminal-store', () => ({
  useQuickTerminalStore: create(() => ({ records: [] }))
}))
vi.mock('@/features/quick-terminal/quick-terminal-workspace-store', () => ({
  useQuickWorkspaceStore: create(() => ({ layouts: {} }))
}))
vi.mock('@/stores/workspace-store', () => ({
  useWorkspaceStore: create(() => ({
    tab: undefined,
    getActiveTab() {
      return useWorkspaceStore.getState().tab
    }
  }))
}))
vi.mock('@/stores/plugins-store', () => ({
  usePluginsStore: create(() => ({ connected: false, bootId: null }))
}))
vi.mock('@/stores/session-workspace-sync-store', () => ({
  useSessionWorkspaceSyncStore: create(() => ({
    restoreInProgressByConversation: {},
    loadOutcomeByConversation: {}
  }))
}))
vi.mock('@/stores/workspace-manifest-sync-store', () => ({
  useWorkspaceManifestSyncStore: create(() => ({ manifestRestoreInProgressByProject: {} }))
}))

import { useAcpStore } from '@/features/agent-session/stores/acp-store'
import { useQuickTerminalStore } from '@/features/quick-terminal/quick-terminal-store'
import { useQuickWorkspaceStore } from '@/features/quick-terminal/quick-terminal-workspace-store'
import { usePluginsStore } from '@/stores/plugins-store'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { useWorkspaceStore } from '@/stores/workspace-store'
import { readPluginHostContext, usePluginHostContext } from './use-plugin-host-context'

afterEach(() => {
  vi.useRealTimers()
  vi.clearAllMocks()
  mocks.desktop = true
  mocks.restore = false
  usePluginsStore.setState({ connected: false, bootId: null })
  useProjectStore.setState({ projects: [], activeProjectId: '', isLoaded: true })
  useTerminalStore.setState({ terminals: [] })
  useAcpStore.setState({ sessions: {} })
  useWorkspaceStore.setState({ tab: undefined } as never)
  useQuickTerminalStore.setState({ records: [] })
  useQuickWorkspaceStore.setState({ layouts: {} })
  window.history.replaceState(null, '', '#/')
})
function prepare() {
  vi.useFakeTimers()
  mocks.publish.mockResolvedValue({
    success: true,
    data: { contextRevision: 1, sourceSequence: 1 }
  })
  mocks.heartbeat.mockResolvedValue({
    success: true,
    data: { contextRevision: 1, sourceSequence: 1 }
  })
}

function quickWorkspace() {
  useTerminalStore.setState({ terminals: [{ id: 'primary' }, { id: 'quick' }] } as never)
  useWorkspaceStore.setState({
    tab: { id: 'primary', type: 'terminal', terminalId: 'primary' }
  } as never)
  useQuickTerminalStore.setState({ records: [{ id: 'q', terminalIds: ['quick'] }] } as never)
  useQuickWorkspaceStore.setState({
    layouts: {
      q: {
        tabs: [
          { id: 'terminal:quick', type: 'terminal', terminalId: 'quick' },
          { id: 'editor', type: 'editor', filePath: '/file' }
        ],
        activeTabId: 'terminal:quick',
        lastTerminalTabId: 'terminal:quick'
      }
    }
  })
}

describe('displayed workspace context', () => {
  it.each(['/', '/terminal', '/c/conversation'])('uses the primary pane on %s', (path) => {
    quickWorkspace()
    window.history.replaceState(null, '', `#${path}`)
    expect(readPluginHostContext().activeTerminalId).toBe('primary')
  })

  it.each([
    '/settings',
    '/projects',
    '/workspace',
    '/quick-terminals/missing'
  ])('clears hidden primary selection on %s but preserves all managed terminals', (path) => {
    quickWorkspace()
    window.history.replaceState(null, '', `#${path}`)
    const snapshot = readPluginHostContext()
    expect(snapshot.activeTerminalId).toBeNull()
    expect(snapshot.activeSessionId).toBeNull()
    expect(snapshot.terminals.map((terminal) => terminal.id).sort()).toEqual(['primary', 'quick'])
  })

  it('selects only an owned active quick terminal, never an editor, stale or unmanaged ID', () => {
    quickWorkspace()
    window.history.replaceState(null, '', '#/quick-terminals/q?view=terminal')
    expect(readPluginHostContext().activeTerminalId).toBe('quick')
    const layout = useQuickWorkspaceStore.getState().layouts.q
    useQuickWorkspaceStore.setState({ layouts: { q: { ...layout, activeTabId: 'editor' } } })
    expect(readPluginHostContext().activeTerminalId).toBeNull()
    useQuickWorkspaceStore.setState({ layouts: { q: layout } })
    useQuickTerminalStore.setState({ records: [{ id: 'q', terminalIds: [] }] } as never)
    expect(readPluginHostContext().activeTerminalId).toBeNull()
    useQuickTerminalStore.setState({ records: [{ id: 'q', terminalId: 'quick' }] } as never)
    expect(readPluginHostContext().activeTerminalId).toBe('quick')
    useTerminalStore.setState({ terminals: [{ id: 'primary' }] } as never)
    expect(readPluginHostContext().activeTerminalId).toBeNull()
    expect(readPluginHostContext().terminals.map((terminal) => terminal.id)).toEqual(['primary'])
  })

  it('does not expose the primary agent session on quick or non-workspace routes', () => {
    quickWorkspace()
    useWorkspaceStore.setState({
      tab: { id: 'agent:s', type: 'agent-chat', sessionId: 's' }
    } as never)
    useAcpStore.setState({
      sessions: {
        s: { id: 's', projectId: 'p', status: 'active', activeTurn: false, openTurnId: null }
      }
    } as never)
    expect(readPluginHostContext().activeSessionId).toBe('s')
    window.history.replaceState(null, '', '#/quick-terminals/q')
    expect(readPluginHostContext().activeSessionId).toBeNull()
    window.history.replaceState(null, '', '#/settings')
    expect(readPluginHostContext().activeSessionId).toBeNull()
    expect(readPluginHostContext().agentSessions).toHaveLength(1)
  })

  it.each([
    'popstate',
    'hashchange'
  ])('captures %s navigation without history wrappers', async (event) => {
    prepare()
    quickWorkspace()
    usePluginsStore.setState({ connected: true })
    const navigate = window.history.replaceState.bind(window.history)
    const { unmount } = renderHook(() => usePluginHostContext())
    await vi.advanceTimersByTimeAsync(0)
    mocks.pending.mockClear()
    navigate(null, '', '#/quick-terminals/q')
    window.dispatchEvent(new Event(event))
    expect(mocks.pending).toHaveBeenCalledWith(true)
    await vi.advanceTimersByTimeAsync(50)
    expect(mocks.publish.mock.calls.at(-1)?.[0]).toMatchObject({
      reset: true,
      snapshot: { activeTerminalId: 'quick' }
    })
    unmount()
  })

  it('captures route and quick-store changes before returning, resets routes, and restores history', async () => {
    prepare()
    quickWorkspace()
    usePluginsStore.setState({ connected: true })
    const originalPush = window.history.pushState
    const originalReplace = window.history.replaceState
    const { unmount } = renderHook(() => usePluginHostContext())
    await vi.advanceTimersByTimeAsync(0)
    mocks.pending.mockClear()
    window.history.pushState(null, '', '#/quick-terminals/q')
    expect(mocks.pending).toHaveBeenCalledWith(true)
    await vi.advanceTimersByTimeAsync(50)
    expect(mocks.publish.mock.calls.at(-1)?.[0]).toMatchObject({
      reset: true,
      snapshot: { activeTerminalId: 'quick' }
    })
    const layout = useQuickWorkspaceStore.getState().layouts.q
    mocks.pending.mockClear()
    useQuickWorkspaceStore.setState({ layouts: { q: { ...layout, activeTabId: 'editor' } } })
    expect(mocks.pending).toHaveBeenCalledWith(true)
    await vi.advanceTimersByTimeAsync(50)
    expect(mocks.publish.mock.calls.at(-1)?.[0].snapshot.activeTerminalId).toBeNull()
    useQuickWorkspaceStore.setState({ layouts: { q: layout } })
    mocks.pending.mockClear()
    useQuickTerminalStore.setState({ records: [] })
    expect(mocks.pending).toHaveBeenCalledWith(true)
    window.history.replaceState(null, '', '#/settings')
    expect(mocks.pending).toHaveBeenLastCalledWith(true)
    await vi.advanceTimersByTimeAsync(50)
    for (const [request] of mocks.publish.mock.calls) {
      expect(
        request.events.some((event: { topic: string }) => event.topic === 'terminal.closed')
      ).toBe(false)
    }
    unmount()
    expect(window.history.pushState).toBe(originalPush)
    expect(window.history.replaceState).toBe(originalReplace)
    const pendingCount = mocks.pending.mock.calls.length
    window.history.pushState(null, '', '#/')
    window.dispatchEvent(new PopStateEvent('popstate'))
    window.dispatchEvent(new HashChangeEvent('hashchange'))
    expect(mocks.pending).toHaveBeenCalledTimes(pendingCount)
    expect(vi.getTimerCount()).toBe(0)
  })
})

describe('desktop context root hook', () => {
  it('browser mode creates no transport calls or native listeners', async () => {
    prepare()
    mocks.desktop = false
    const originalPush = window.history.pushState
    const originalReplace = window.history.replaceState
    const { unmount } = renderHook(() => usePluginHostContext())
    usePluginsStore.setState({ connected: true })
    quickWorkspace()
    window.history.pushState(null, '', '#/quick-terminals/q')
    expect(window.history.pushState).toBe(originalPush)
    expect(window.history.replaceState).toBe(originalReplace)
    expect(vi.getTimerCount()).toBe(0)
    expect(mocks.pending).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(4000)
    expect(mocks.publish).not.toHaveBeenCalled()
    expect(mocks.heartbeat).not.toHaveBeenCalled()
    unmount()
  })

  it('captures rapid session transitions synchronously and never closes a hidden terminal', async () => {
    prepare()
    usePluginsStore.setState({ connected: true })
    useTerminalStore.setState({ terminals: [{ id: 't', ptyId: 'pty', projectId: 'p' }] } as never)
    useWorkspaceStore.setState({
      tab: { id: 'terminal:t', type: 'terminal', terminalId: 't' }
    } as never)
    const { unmount } = renderHook(() => usePluginHostContext())
    await vi.advanceTimersByTimeAsync(0)
    useWorkspaceStore.setState({ tab: undefined } as never)
    const session = {
      id: 's',
      projectId: 'p',
      status: 'active',
      activeTurn: true,
      openTurnId: 'turn',
      replaying: false
    }
    useAcpStore.setState({ sessions: { s: session } } as never)
    useAcpStore.setState({
      sessions: { s: { ...session, activeTurn: false, openTurnId: null } }
    } as never)
    await vi.advanceTimersByTimeAsync(50)
    const request = mocks.publish.mock.calls[1][0] as HostContextPublishRequest
    expect(request.events.filter((e) => e.topic === 'agent.session.stateChanged')).toHaveLength(2)
    expect(request.events.some((e) => e.topic === 'terminal.closed')).toBe(false)
    useTerminalStore.setState({ terminals: [] })
    await vi.advanceTimersByTimeAsync(50)
    expect(
      mocks.publish.mock.calls[2][0].events.some(
        (e: { topic: string }) => e.topic === 'terminal.closed'
      )
    ).toBe(true)
    expect(mocks.ack).toHaveBeenCalledWith(1, expect.objectContaining({ terminals: [] }))
    unmount()
    const count = mocks.publish.mock.calls.length
    usePluginsStore.setState({ connected: false })
    usePluginsStore.setState({ connected: true })
    await vi.advanceTimersByTimeAsync(4000)
    expect(mocks.publish).toHaveBeenCalledTimes(count)
    expect(mocks.heartbeat).not.toHaveBeenCalled()
  })

  it('StrictMode owns one surviving heartbeat and cleans all timers on final unmount', async () => {
    prepare()
    usePluginsStore.setState({ connected: true })
    const { unmount } = renderHook(() => usePluginHostContext(), {
      wrapper: ({ children }) => createElement(StrictMode, null, children)
    })
    await vi.advanceTimersByTimeAsync(0)
    expect(mocks.publish).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(2000)
    expect(mocks.heartbeat).toHaveBeenCalledTimes(1)
    unmount()
    expect(vi.getTimerCount()).toBe(0)
  })

  it('publishes resets for hydration, restore completion and reconnect', async () => {
    prepare()
    usePluginsStore.setState({ connected: true, bootId: 'boot' })
    const { unmount } = renderHook(() => usePluginHostContext())
    await vi.advanceTimersByTimeAsync(0)
    useProjectStore.setState({ isLoaded: false })
    await vi.advanceTimersByTimeAsync(50)
    expect(mocks.publish.mock.calls.at(-1)?.[0].reset).toBe(true)
    useProjectStore.setState({ isLoaded: true })
    await vi.advanceTimersByTimeAsync(50)
    expect(mocks.publish.mock.calls.at(-1)?.[0].reset).toBe(true)
    mocks.restore = true
    await vi.advanceTimersByTimeAsync(50)
    mocks.restore = false
    await vi.advanceTimersByTimeAsync(50)
    expect(mocks.publish.mock.calls.at(-1)?.[0].reset).toBe(true)
    usePluginsStore.setState({ connected: false })
    usePluginsStore.setState({ connected: true, bootId: 'new-boot' })
    await vi.advanceTimersByTimeAsync(50)
    expect(mocks.publish.mock.calls.at(-1)?.[0].reset).toBe(true)
    unmount()
  })
})
