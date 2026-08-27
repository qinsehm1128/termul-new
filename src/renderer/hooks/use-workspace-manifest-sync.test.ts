import { act, cleanup, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const { getManifestMock, writeManifestMock, deleteManifestMock } = vi.hoisted(() => ({
  getManifestMock: vi.fn(),
  writeManifestMock: vi.fn(),
  deleteManifestMock: vi.fn()
}))

vi.mock('@/lib/workspace-manifest-api', () => ({
  workspaceManifestApi: {
    getManifest: getManifestMock,
    writeManifest: writeManifestMock,
    deleteManifest: deleteManifestMock
  }
}))

import type { WorkspaceManifest } from '@shared/types/workspace-manifest.types'
import { useEditorStore } from '@/stores/editor-store'
import { useWorkspaceStore } from '@/stores/workspace-store'
import {
  buildPortableManifest,
  inspectLegacyWorkspaceManifest,
  loadWorkspaceManifest,
  performManifestWrite,
  rebuildTopologyFromManifest,
  resolveManifestConflict,
  useWorkspaceManifestSync
} from './use-workspace-manifest-sync'

function manifest(overrides: Partial<WorkspaceManifest> = {}): WorkspaceManifest {
  return {
    projectId: 'project-one',
    revision: 7,
    updatedAt: 10,
    topology: {
      type: 'leaf',
      id: 'legacy-leaf',
      terminalIds: ['legacy-terminal'],
      editorIds: ['edit-/legacy/file.ts'],
      activeTabId: 'term-legacy-terminal'
    },
    activePaneId: 'legacy-leaf',
    focusedSessionId: 'opaque-agent-session',
    terminals: [
      {
        terminalId: 'legacy-terminal',
        projectId: 'project-one',
        shell: 'bash',
        cwd: '/legacy',
        name: 'legacy'
      }
    ],
    editors: [{ editorId: 'edit-/legacy/file.ts', filePath: '/legacy/file.ts' }],
    ...overrides
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  useWorkspaceStore.getState().resetLayout()
  useEditorStore.getState().clearAllFiles()
})

afterEach(() => {
  vi.useRealTimers()
  cleanup()
})

describe('read-only legacy workspace-manifest boundary', () => {
  it('builds only an empty compatibility shape and serializes no live project state', () => {
    useWorkspaceStore.getState().addTerminalTab('terminal-one')
    const value = buildPortableManifest('project-one')

    expect(value).toEqual({
      projectId: 'project-one',
      revision: 0,
      updatedAt: 0,
      terminals: [],
      editors: [],
      topology: undefined,
      activePaneId: null,
      focusedSessionId: null
    })
  })

  it('rebuilds editor evidence only and never restores terminal ownership', () => {
    const rebuilt = rebuildTopologyFromManifest(manifest())

    expect(rebuilt.root).toMatchObject({
      type: 'leaf',
      id: 'legacy-leaf',
      tabs: [{ type: 'editor', filePath: '/legacy/file.ts' }]
    })
    expect(rebuilt.activePaneId).toBe('legacy-leaf')
    expect(rebuilt.focusedSessionId).toBeNull()
  })

  it('returns preserved bytes through the read facade without applying topology', async () => {
    const legacy = manifest()
    getManifestMock.mockResolvedValue({ success: true, data: legacy })
    const beforeRoot = useWorkspaceStore.getState().root
    const openFile = vi.spyOn(useEditorStore.getState(), 'openFile').mockResolvedValue(undefined)

    expect(await inspectLegacyWorkspaceManifest('project-one')).toEqual(legacy)
    expect(await loadWorkspaceManifest('project-one')).toBe(false)
    expect(getManifestMock).toHaveBeenCalledWith('project-one')
    expect(openFile).toHaveBeenCalledWith('/legacy/file.ts')
    expect(useWorkspaceStore.getState().root).toBe(beforeRoot)
    openFile.mockRestore()
  })

  it('degrades failed inspection to null without any mutation attempt', async () => {
    getManifestMock.mockResolvedValue({
      success: false,
      error: 'read unavailable',
      code: 'WORKSPACE_MANIFEST_GET_FAILED'
    })

    expect(await inspectLegacyWorkspaceManifest('project-one')).toBeNull()
    expect(await loadWorkspaceManifest('project-one')).toBe(false)
    expect(writeManifestMock).not.toHaveBeenCalled()
    expect(deleteManifestMock).not.toHaveBeenCalled()
  })

  it('skips every legacy write/conflict mutation action', async () => {
    expect(await performManifestWrite('project-one')).toBe('skipped')
    await resolveManifestConflict('project-one', 'overwrite')
    await resolveManifestConflict('project-one', 'dismiss')

    expect(writeManifestMock).not.toHaveBeenCalled()
    expect(deleteManifestMock).not.toHaveBeenCalled()
  })

  it('allows reload only as a fresh read-only inspection', async () => {
    getManifestMock.mockResolvedValue({ success: true, data: manifest() })

    await resolveManifestConflict('project-one', 'reload')

    expect(getManifestMock).toHaveBeenCalledWith('project-one')
    expect(writeManifestMock).not.toHaveBeenCalled()
  })

  it('mounts no subscriptions and never debounces project-manifest writes', async () => {
    vi.useFakeTimers()
    const { unmount } = renderHook(() => useWorkspaceManifestSync('project-one'))

    act(() => useWorkspaceStore.setState({ activePaneId: 'changed' }))
    await act(async () => vi.advanceTimersByTime(1_000))

    expect(writeManifestMock).not.toHaveBeenCalled()
    unmount()
  })
})
