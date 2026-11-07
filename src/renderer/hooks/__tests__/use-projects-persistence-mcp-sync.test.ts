import { act, renderHook, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useProjectStore } from '@/stores/project-store'

const { loadMcpConfigMock, syncProjectsMock, isTauriContextMock } = vi.hoisted(() => ({
  loadMcpConfigMock: vi.fn(async () => undefined),
  syncProjectsMock: vi.fn(
    async (): Promise<{ success: boolean; error?: string }> => ({ success: true })
  ),
  isTauriContextMock: vi.fn(() => true)
}))

vi.mock('@/lib/api', () => ({
  persistenceApi: {
    read: vi.fn(async () => ({ success: false })),
    write: vi.fn(async () => ({ success: true })),
    writeDebounced: vi.fn(async () => ({ success: true })),
    delete: vi.fn(async () => ({ success: true }))
  },
  secureStorageApi: {
    setSecret: vi.fn(async () => ({ success: true })),
    getSecret: vi.fn(async () => ({ success: false })),
    deleteSecret: vi.fn(async () => ({ success: true }))
  },
  syncProjects: syncProjectsMock,
  terminalApi: { kill: vi.fn(async () => ({ success: true })) },
  worktreeApi: { list: vi.fn(async () => ({ success: false })) }
}))
vi.mock('@/lib/tauri-runtime', () => ({
  isTauriContext: isTauriContextMock,
  cleanupTauriListener: vi.fn()
}))
vi.mock('@/stores/remote-status-store', () => ({
  useRemoteStatusStore: {
    getState: () => ({ status: { running: false } }),
    subscribe: () => () => {}
  }
}))
vi.mock('@/stores/mcp-store', () => ({
  useMcpStore: {
    getState: () => ({ load: loadMcpConfigMock })
  }
}))
vi.mock('@/stores/terminal-store', () => ({
  useTerminalStore: { getState: () => ({ terminals: [] }) }
}))
vi.mock('@/stores/workspace-manifest-sync-store', () => ({
  useWorkspaceManifestSyncStore: {
    getState: () => ({
      setBasedRevision: vi.fn(),
      setManifestRestoreInProgress: vi.fn(),
      pendingConflict: null,
      setPendingConflict: vi.fn()
    })
  }
}))
vi.mock('@/lib/workspace-manifest-api', () => ({
  workspaceManifestApi: { deleteManifest: vi.fn(async () => ({ success: true })) }
}))
vi.mock('@/lib/terminal-api', () => ({
  setTerminalProtected: vi.fn(async () => ({ success: true }))
}))
vi.mock('@/lib/acp-transport', () => ({
  getAcpTransport: () => ({ onEvent: () => () => {} })
}))
vi.mock('@/lib/web-server-api', () => ({
  webServerProjects: { list: vi.fn() }
}))

import { persistenceApi } from '@/lib/api'
import { useProjectsAutoSave } from '../use-projects-persistence'

describe('useProjectsAutoSave MCP sync on project switch (CAP-7)', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    syncProjectsMock.mockResolvedValue({ success: true })
    isTauriContextMock.mockReturnValue(true)

    useProjectStore.setState({
      projects: [],
      groups: [],
      activeProjectId: 'warmup',
      activeGroupId: null,
      isLoaded: true
    })
  })

  it('seeds the existing host mirror on mount with remote access off and no env values', async () => {
    useProjectStore.setState({
      projects: [
        {
          id: 'p1',
          name: 'P1',
          color: 'blue',
          path: '/p1',
          isDefault: true,
          envVars: [{ key: 'TOKEN', value: 'do-not-mirror' }]
        }
      ],
      activeProjectId: 'p1'
    })
    const { unmount } = renderHook(() => useProjectsAutoSave())
    await waitFor(() => expect(syncProjectsMock).toHaveBeenCalledTimes(1))
    expect(syncProjectsMock).toHaveBeenCalledWith(
      [expect.objectContaining({ id: 'p1', path: '/p1', isDefault: true })],
      'p1',
      []
    )
    expect(JSON.stringify(syncProjectsMock.mock.calls)).not.toContain('do-not-mirror')
    expect(persistenceApi.write).not.toHaveBeenCalled()
    expect(persistenceApi.writeDebounced).not.toHaveBeenCalled()
    expect(loadMcpConfigMock).not.toHaveBeenCalled()
    unmount()
  })

  it('pushes the initial load transition even though autosave skips it', async () => {
    useProjectStore.setState({ isLoaded: false })
    const { unmount } = renderHook(() => useProjectsAutoSave())
    expect(syncProjectsMock).not.toHaveBeenCalled()
    await act(async () => {
      useProjectStore.setState({
        projects: [{ id: 'p1', name: 'P1', color: 'blue', path: '/p1' }],
        activeProjectId: 'p1',
        isLoaded: true
      })
    })
    expect(syncProjectsMock).toHaveBeenCalledTimes(1)
    expect(syncProjectsMock).toHaveBeenLastCalledWith(expect.any(Array), 'p1', [])
    expect(persistenceApi.writeDebounced).not.toHaveBeenCalled()
    expect(loadMcpConfigMock).not.toHaveBeenCalled()
    unmount()
  })

  it('pushes the first change and group changes without depending on the autosave warmup', async () => {
    const { unmount } = renderHook(() => useProjectsAutoSave())
    syncProjectsMock.mockClear()
    await act(async () => {
      useProjectStore.setState({ activeProjectId: 'p1' })
    })
    expect(syncProjectsMock).toHaveBeenCalledTimes(1)
    expect(syncProjectsMock).toHaveBeenLastCalledWith([], 'p1', [])
    await act(async () => {
      useProjectStore.setState({
        groups: [{ id: 'g1', name: 'Group', projectIds: [] }]
      })
    })
    expect(syncProjectsMock).toHaveBeenCalledTimes(2)
    expect(syncProjectsMock).toHaveBeenLastCalledWith([], 'p1', [
      { id: 'g1', name: 'Group', projectIds: [], color: null, preferredProjectId: null }
    ])
    expect(loadMcpConfigMock).not.toHaveBeenCalled()
    unmount()
  })

  it('does not push the desktop mirror in a web context', async () => {
    isTauriContextMock.mockReturnValue(false)
    const { unmount } = renderHook(() => useProjectsAutoSave())
    await act(async () => {
      useProjectStore.setState({ activeProjectId: 'web-project' })
    })
    expect(syncProjectsMock).not.toHaveBeenCalled()
    expect(persistenceApi.writeDebounced).not.toHaveBeenCalled()
    unmount()
  })

  it('reloads the MCP control plane when activeProjectId changes', async () => {
    const { unmount } = renderHook(() => useProjectsAutoSave())

    // First state change initializes the hasInitialized ref (skipped by guard).
    await act(async () => {
      useProjectStore.setState({ activeProjectId: 'warmup2' })
    })

    // Second state change: real project switch.
    await act(async () => {
      useProjectStore.setState({
        projects: [{ id: 'p1', name: 'P1', color: 'blue', path: '/p1' }],
        groups: [
          {
            id: 'group-1',
            name: 'Workspace',
            projectIds: ['p1'],
            preferredProjectId: 'p1'
          }
        ],
        activeProjectId: 'p1'
      })
    })

    await waitFor(() => {
      expect(syncProjectsMock).toHaveBeenCalled()
    })
    expect(syncProjectsMock).toHaveBeenLastCalledWith(expect.any(Array), 'p1', [
      {
        id: 'group-1',
        name: 'Workspace',
        projectIds: ['p1'],
        color: null,
        preferredProjectId: 'p1'
      }
    ])

    await waitFor(() => {
      expect(loadMcpConfigMock).toHaveBeenCalledTimes(1)
    })

    unmount()
  })

  it('does not reload MCP config when only projects change (no switch)', async () => {
    const { unmount } = renderHook(() => useProjectsAutoSave())

    await act(async () => {
      useProjectStore.setState({ activeProjectId: 'warmup2' })
    })

    await act(async () => {
      useProjectStore.setState({
        projects: [{ id: 'p1', name: 'P1', color: 'blue', path: '/p1' }],
        activeProjectId: 'warmup2'
      })
    })

    await waitFor(() => {
      expect(syncProjectsMock).toHaveBeenCalled()
    })

    expect(loadMcpConfigMock).not.toHaveBeenCalled()

    unmount()
  })

  it('waits for the host mirror before reloading after a stored-default change', async () => {
    const { unmount } = renderHook(() => useProjectsAutoSave())
    await act(async () => {
      useProjectStore.setState({ activeProjectId: 'p1' })
    })
    let resolveSync!: (result: { success: boolean }) => void
    syncProjectsMock.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveSync = resolve
        })
    )
    await act(async () => {
      useProjectStore.setState({
        projects: [{ id: 'p2', name: 'Default', color: 'blue', path: '/p2', isDefault: true }]
      })
    })
    expect(loadMcpConfigMock).not.toHaveBeenCalled()
    await act(async () => resolveSync({ success: true }))
    expect(loadMcpConfigMock).toHaveBeenCalledTimes(1)
    unmount()
  })

  it('does not reload MCP config when syncProjects fails', async () => {
    syncProjectsMock.mockResolvedValue({ success: false, error: 'boom' })

    const { unmount } = renderHook(() => useProjectsAutoSave())

    await act(async () => {
      useProjectStore.setState({ activeProjectId: 'warmup2' })
    })

    await act(async () => {
      useProjectStore.setState({
        projects: [{ id: 'p2', name: 'P2', color: 'green', path: '/p2' }],
        activeProjectId: 'p2'
      })
    })

    await waitFor(() => {
      expect(syncProjectsMock).toHaveBeenCalled()
    })

    expect(loadMcpConfigMock).not.toHaveBeenCalled()

    unmount()
  })
})
