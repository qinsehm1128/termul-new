import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useProjectStore } from '@/stores/project-store'
import type { Project } from '@/types/project'

const { writeDebouncedMock } = vi.hoisted(() => ({
  writeDebouncedMock: vi.fn(async () => ({ success: true }))
}))

vi.mock('@/lib/api', () => ({
  persistenceApi: {
    read: vi.fn(async () => ({ success: false })),
    write: vi.fn(async () => ({ success: true })),
    writeDebounced: writeDebouncedMock,
    delete: vi.fn(async () => ({ success: true }))
  },
  secureStorageApi: {
    setSecret: vi.fn(async () => ({ success: true })),
    getSecret: vi.fn(async () => ({ success: false })),
    deleteSecret: vi.fn(async () => ({ success: true }))
  },
  syncProjects: vi.fn(async () => ({ success: true })),
  terminalApi: { kill: vi.fn(async () => ({ success: true })) },
  worktreeApi: { list: vi.fn(async () => ({ success: false })) }
}))
vi.mock('@/lib/tauri-runtime', () => ({
  isTauriContext: () => true,
  cleanupTauriListener: vi.fn()
}))
// The remote bridge is off here: this file is about what a click costs on the
// persistence path, and the sync branch has its own test file.
vi.mock('@/stores/remote-status-store', () => ({
  useRemoteStatusStore: {
    getState: () => ({ status: { running: false } }),
    subscribe: () => () => {}
  }
}))
vi.mock('@/stores/acp-store', () => ({
  useAcpStore: { getState: () => ({ syncMcpRegistryToProjectFile: vi.fn() }) }
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

import { flushPendingProjectsSnapshot, useProjectsAutoSave } from '../use-projects-persistence'

function project(id: string, overrides: Partial<Project> = {}): Project {
  return { id, name: id.toUpperCase(), color: 'blue', path: `/${id}`, ...overrides } as Project
}

const FOCUS_ONLY_COALESCE_MS = 500

describe('useProjectsAutoSave write coalescing', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.useFakeTimers()
    useProjectStore.setState({
      projects: [project('p1', { isActive: true }), project('p2'), project('p3')],
      groups: [],
      activeProjectId: 'p1',
      activeGroupId: null,
      isLoaded: true
    })
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  /** Drive past the initialization guard so later changes are observed. */
  function mountArmed(): ReturnType<typeof renderHook<void, unknown>> {
    const rendered = renderHook(() => useProjectsAutoSave())
    act(() => {
      useProjectStore.setState({ activeGroupId: 'warmup' })
    })
    writeDebouncedMock.mockClear()
    return rendered
  }

  it('does not serialise on a focus-only switch until the coalesce window closes', async () => {
    const { unmount } = mountArmed()

    act(() => {
      useProjectStore.getState().selectProject('p2')
    })
    expect(writeDebouncedMock).not.toHaveBeenCalled()

    await act(async () => {
      await vi.advanceTimersByTimeAsync(FOCUS_ONLY_COALESCE_MS)
    })
    expect(writeDebouncedMock).toHaveBeenCalledTimes(1)

    unmount()
  })

  it('collapses a burst of switches into one write carrying the final selection', async () => {
    const { unmount } = mountArmed()

    act(() => {
      useProjectStore.getState().selectProject('p2')
      useProjectStore.getState().selectProject('p3')
      useProjectStore.getState().selectProject('p1')
    })

    await act(async () => {
      await vi.advanceTimersByTimeAsync(FOCUS_ONLY_COALESCE_MS)
    })

    expect(writeDebouncedMock).toHaveBeenCalledTimes(1)
    const [, data] = writeDebouncedMock.mock.calls[0] as [string, { activeProjectId: string }]
    expect(data.activeProjectId).toBe('p1')

    unmount()
  })

  it('serialises a structural change immediately rather than deferring it', async () => {
    const { unmount } = mountArmed()

    act(() => {
      useProjectStore.setState({
        projects: [...useProjectStore.getState().projects, project('p4')]
      })
    })

    // Flush the promise chain inside persistProjectsSnapshot without advancing
    // the coalesce window — an add must not wait it out.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(writeDebouncedMock).toHaveBeenCalledTimes(1)

    unmount()
  })

  it('lets a structural change subsume a pending focus-only write', async () => {
    const { unmount } = mountArmed()

    act(() => {
      useProjectStore.getState().selectProject('p2')
    })
    act(() => {
      useProjectStore.setState({
        projects: [...useProjectStore.getState().projects, project('p4')]
      })
    })

    await act(async () => {
      await vi.advanceTimersByTimeAsync(FOCUS_ONLY_COALESCE_MS * 2)
    })

    // One write, not two: the immediate structural flush cancelled the queued
    // focus-only one instead of racing it.
    expect(writeDebouncedMock).toHaveBeenCalledTimes(1)

    unmount()
  })

  // The close path flushes QUEUED writes. A focus-only change inside its
  // coalesce window has queued none — `persistProjectsSnapshot` has not run —
  // so quitting right after clicking a project used to lose that selection,
  // while a comment claimed the close path already covered it.
  it('flushes a queued focus-only write for the close path', async () => {
    const { unmount } = mountArmed()

    act(() => {
      useProjectStore.getState().selectProject('p2')
    })
    expect(writeDebouncedMock).not.toHaveBeenCalled()

    await act(async () => {
      await flushPendingProjectsSnapshot()
    })

    // Queued before the close path's own write flush runs, not 500ms later.
    expect(writeDebouncedMock).toHaveBeenCalledTimes(1)
    const [, payload] = writeDebouncedMock.mock.calls[0] as [string, { activeProjectId?: string }]
    expect(payload.activeProjectId).toBe('p2')

    // The timer it replaced must not fire a second write.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(FOCUS_ONLY_COALESCE_MS * 2)
    })
    expect(writeDebouncedMock).toHaveBeenCalledTimes(1)

    unmount()
  })

  // The close-path handler lives in a module singleton. An unmount that clears
  // it unconditionally would take out a handler a later mount had already
  // installed, leaving the close path with nothing to flush.
  it.each([
    ['the older instance unmounts first', 'first'],
    ['the newer instance unmounts first', 'second']
  ])('still flushes a mounted instance when %s', async (_label, unmountFirst) => {
    const first = renderHook(() => useProjectsAutoSave())
    const second = mountArmed()

    act(() => {
      useProjectStore.getState().selectProject('p2')
    })
    const [gone, staying] = unmountFirst === 'first' ? [first, second] : [second, first]
    gone.unmount()

    await act(async () => {
      await flushPendingProjectsSnapshot()
    })
    expect(writeDebouncedMock).toHaveBeenCalled()

    staying.unmount()
  })

  it('has nothing to flush once the hook is unmounted', async () => {
    const { unmount } = mountArmed()
    act(() => {
      useProjectStore.getState().selectProject('p2')
    })
    unmount()

    await act(async () => {
      await flushPendingProjectsSnapshot()
    })
    expect(writeDebouncedMock).not.toHaveBeenCalled()
  })

  it('drops a queued focus-only write on unmount', async () => {
    const { unmount } = mountArmed()

    act(() => {
      useProjectStore.getState().selectProject('p2')
    })
    unmount()

    await act(async () => {
      await vi.advanceTimersByTimeAsync(FOCUS_ONLY_COALESCE_MS * 2)
    })
    expect(writeDebouncedMock).not.toHaveBeenCalled()
  })
})
