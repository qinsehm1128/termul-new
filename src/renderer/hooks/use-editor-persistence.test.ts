import { renderHook, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { LeafNode, PaneNode, SplitNode } from '@/types/workspace.types'
import {
  persistState,
  subscribeProjectWorkspaceRestored,
  useEditorPersistence
} from './use-editor-persistence'

const { mockLoadPersistedTerminals, quickEditorPaths } = vi.hoisted(() => ({
  mockLoadPersistedTerminals: vi.fn(),
  quickEditorPaths: new Set<string>()
}))

vi.mock('@/features/quick-terminal/quick-terminal-editor-drafts', () => ({
  isQuickEditorFile: (filePath: string) => quickEditorPaths.has(filePath)
}))

const { mockPersistenceRead, mockPersistenceWriteDebounced } = vi.hoisted(() => ({
  mockPersistenceRead: vi.fn(),
  mockPersistenceWriteDebounced: vi.fn()
}))

vi.mock('@/lib/api', () => ({
  persistenceApi: {
    read: mockPersistenceRead,
    writeDebounced: mockPersistenceWriteDebounced
  }
}))

function createEditorFileState(filePath: string): {
  filePath: string
  content: string
  originalContent: string
  isDirty: boolean
  language: string
  lastModified: number
  viewMode: 'code' | 'markdown'
  cursorPosition: { line: number; col: number }
  scrollTop: number
  operationStatus: 'idle' | 'saving' | 'reloading' | 'saved'
} {
  return {
    filePath,
    content: '',
    originalContent: '',
    isDirty: false,
    language: 'typescript',
    lastModified: Date.now(),
    viewMode: 'code',
    cursorPosition: { line: 1, col: 1 },
    scrollTop: 0,
    operationStatus: 'idle'
  }
}

const mockEditorState = {
  openFiles: new Map<string, ReturnType<typeof createEditorFileState>>(),
  activeFilePath: null as string | null,
  clearAllFiles: vi.fn(),
  openFile: vi.fn().mockResolvedValue(undefined),
  updateCursorPosition: vi.fn(),
  updateScrollTop: vi.fn(),
  setViewMode: vi.fn(),
  updateContent: vi.fn(),
  setActiveFilePath: vi.fn()
}

const mockExplorerState = {
  expandedDirs: new Set<string>(),
  setAsideExpandedDirs: null as Set<string> | null,
  isVisible: true,
  setExpandedDirs: vi.fn(),
  restoreExpandedDirs: vi.fn().mockResolvedValue(undefined)
}

const mockWorkspaceState: {
  root: PaneNode
  activePaneId: string
  activeTabId: string | null
  syncEditorTabs: ReturnType<typeof vi.fn>
  remapTerminalTabs: ReturnType<typeof vi.fn>
  syncTerminalTabs: ReturnType<typeof vi.fn>
  clearEditorTabs: ReturnType<typeof vi.fn>
  resetLayout: ReturnType<typeof vi.fn>
  loadProjectWorkspace: ReturnType<typeof vi.fn>
} = {
  root: {
    type: 'leaf',
    id: 'pane-root',
    tabs: [],
    activeTabId: null
  },
  activePaneId: 'pane-root',
  activeTabId: null,
  syncEditorTabs: vi.fn(),
  remapTerminalTabs: vi.fn(),
  syncTerminalTabs: vi.fn(),
  clearEditorTabs: vi.fn(),
  resetLayout: vi.fn(),
  loadProjectWorkspace: vi.fn()
}

const mockProjectState = {
  projects: [
    { id: 'project-a', path: '/projects/a' },
    { id: 'project-b', path: '/projects/b' }
  ]
}

vi.mock('@/stores/editor-store', async () => {
  const actual =
    await vi.importActual<typeof import('@/stores/editor-store')>('@/stores/editor-store')
  return {
    getDefaultViewMode: actual.getDefaultViewMode,
    useEditorStore: {
      getState: vi.fn(() => mockEditorState),
      setState: vi.fn((state: Partial<typeof mockEditorState>) => {
        Object.assign(mockEditorState, state)
      }),
      subscribe: vi.fn(() => vi.fn())
    }
  }
})

vi.mock('@/stores/file-explorer-store', () => ({
  useFileExplorerStore: {
    getState: vi.fn(() => mockExplorerState),
    subscribe: vi.fn(() => vi.fn())
  }
}))

vi.mock('@/stores/workspace-store', async () => {
  const actual = await vi.importActual<typeof import('@/stores/workspace-store')>(
    '@/stores/workspace-store'
  )
  return {
    ...actual,
    useWorkspaceStore: {
      getState: vi.fn(() => mockWorkspaceState),
      setState: vi.fn(),
      subscribe: vi.fn(() => vi.fn())
    }
  }
})

vi.mock('@/stores/project-store', () => ({
  useProjectStore: {
    getState: vi.fn(() => mockProjectState)
  }
}))

const mockConversationState = { activeConversationId: null as string | null }

vi.mock('@/features/agent-session/stores/conversation-store', () => ({
  useConversationStore: {
    getState: vi.fn(() => mockConversationState)
  }
}))

const mockTerminalState = {
  terminals: [] as Array<{
    id: string
    name: string
    projectId: string
    shell: string
    cwd?: string
    ptyId?: string
  }>
}

vi.mock('@/stores/terminal-store', () => ({
  useTerminalStore: {
    getState: vi.fn(() => mockTerminalState)
  }
}))

vi.mock('./useTerminalAutoSave', () => ({
  loadPersistedTerminals: mockLoadPersistedTerminals
}))

// P11: mock the manifest facade + log facade + sync store so loadWorkspaceManifest
// doesn't hit real network I/O (web transport fetch in jsdom → NETWORK_ERROR).
// Default getManifest → null (no manifest → legacy editorStateKey.paneLayout path).
const {
  mockGetManifest,
  mockWriteManifest,
  mockDeleteManifest,
  mockLogFrontendError,
  mockSetManifestRestoreInProgress,
  mockSetRestoredWorkspaceScope,
  mockSyncStoreState
} = vi.hoisted(() => ({
  mockGetManifest: vi.fn().mockResolvedValue({ success: true, data: null }),
  mockWriteManifest: vi
    .fn()
    .mockResolvedValue({ success: true, data: { status: 'updated', revision: 1, updatedAt: 1 } }),
  mockDeleteManifest: vi.fn().mockResolvedValue({ success: true, data: undefined }),
  mockLogFrontendError: vi.fn().mockResolvedValue(undefined),
  mockSetManifestRestoreInProgress: vi.fn(),
  mockSetRestoredWorkspaceScope: vi.fn(),
  mockSyncStoreState: {
    setBasedRevision: vi.fn(),
    advanceBasedRevision: vi.fn(),
    setPendingConflict: vi.fn(),
    setManifestRestoreInProgress: vi.fn(),
    setRestoredWorkspaceScope: vi.fn(),
    getBasedRevision: vi.fn(() => null),
    hasPendingConflict: vi.fn(() => false),
    pendingConflict: null,
    legacyRevisionByProject: {},
    manifestRestoreInProgressByProject: {},
    restoredWorkspaceScope: null
  }
}))

vi.mock('@/lib/workspace-manifest-api', () => ({
  workspaceManifestApi: {
    getManifest: mockGetManifest,
    writeManifest: mockWriteManifest,
    deleteManifest: mockDeleteManifest
  }
}))

vi.mock('@/lib/log-api', () => ({
  logFrontendError: mockLogFrontendError
}))

vi.mock('@/stores/workspace-manifest-sync-store', () => ({
  setManifestRestoreInProgress: mockSetManifestRestoreInProgress,
  setRestoredWorkspaceScope: mockSetRestoredWorkspaceScope,
  isManifestRestoreInProgress: vi.fn(() => false),
  isManifestRestoreInProgressFor: vi.fn(() => false),
  useWorkspaceManifestSyncStore: {
    getState: vi.fn(() => mockSyncStoreState),
    setState: vi.fn(),
    subscribe: vi.fn(() => vi.fn())
  }
}))

// P11/P3: the manifest sync module is NOT mocked — the real loadWorkspaceManifest
// runs against the mocked facade (mockGetManifest) so the manifest-wins branch
// exercises the actual topology rebuild (rebuildTopologyFromManifest +
// loadProjectWorkspace). The sync-store + log facades are mocked above.

beforeEach(() => {
  quickEditorPaths.clear()
  mockPersistenceRead.mockReset()
  mockPersistenceWriteDebounced.mockReset()
  mockLoadPersistedTerminals.mockReset()
  mockLoadPersistedTerminals.mockResolvedValue(null)
  // P11: default getManifest → null (no manifest → legacy path).
  mockGetManifest.mockReset()
  mockGetManifest.mockResolvedValue({ success: true, data: null })
  mockWriteManifest.mockReset()
  // Re-arm: Vitest 4 `mockReset` clears `mockResolvedValue`, leaving the mock
  // returning `undefined` — tests that exercise the manifest-write path would
  // otherwise see a silent failure. Restore the default success payload
  // (mirrors the hoisted definition above).
  mockWriteManifest.mockResolvedValue({
    success: true,
    data: { status: 'updated', revision: 1, updatedAt: 1 }
  })
  mockDeleteManifest.mockReset()
  mockDeleteManifest.mockResolvedValue({ success: true, data: undefined })
  mockLogFrontendError.mockReset()
  mockSetManifestRestoreInProgress.mockReset()
  mockSetRestoredWorkspaceScope.mockReset()

  mockEditorState.openFiles = new Map<string, ReturnType<typeof createEditorFileState>>()
  mockEditorState.activeFilePath = null
  mockEditorState.clearAllFiles.mockReset()
  mockEditorState.clearAllFiles.mockImplementation(() => {
    mockEditorState.openFiles.clear()
    mockEditorState.activeFilePath = null
  })
  mockEditorState.openFile.mockReset()
  mockEditorState.openFile.mockImplementation(async (filePath: string) => {
    mockEditorState.openFiles.set(filePath, createEditorFileState(filePath))
  })
  mockEditorState.updateCursorPosition.mockReset()
  mockEditorState.updateScrollTop.mockReset()
  mockEditorState.setViewMode.mockReset()
  mockEditorState.updateContent.mockReset()
  mockEditorState.setActiveFilePath.mockReset()

  mockExplorerState.expandedDirs = new Set<string>()
  mockExplorerState.isVisible = true
  mockExplorerState.setExpandedDirs.mockReset()
  mockExplorerState.restoreExpandedDirs.mockReset()
  mockExplorerState.restoreExpandedDirs.mockResolvedValue(undefined)

  mockWorkspaceState.root = {
    type: 'leaf',
    id: 'pane-root',
    tabs: [],
    activeTabId: null
  }
  mockWorkspaceState.activePaneId = 'pane-root'
  mockWorkspaceState.activeTabId = null
  mockWorkspaceState.syncEditorTabs.mockReset()
  mockWorkspaceState.remapTerminalTabs.mockReset()
  mockWorkspaceState.syncTerminalTabs.mockReset()
  mockWorkspaceState.clearEditorTabs.mockReset()
  mockWorkspaceState.resetLayout.mockReset()
  mockWorkspaceState.loadProjectWorkspace.mockReset()

  mockTerminalState.terminals = []
  mockConversationState.activeConversationId = null
})

afterEach(() => {
  vi.clearAllMocks()
})

describe('workspace scoping', () => {
  it('exposes no group-scoped workspace restore', async () => {
    const persistence = await import('./use-editor-persistence')

    // A group is not a workspace of its own. While it was, the same project had
    // two tab sets keyed by how you reached it — arrive via the group and its
    // separate, initially empty workspace loaded, which is indistinguishable
    // from every tab having been closed. Re-exporting a reader for
    // `editor-state/group-<id>` is how that second workspace comes back.
    expect(Object.keys(persistence)).not.toContain('restoreProjectGroupWorkspace')
  })
})

describe('quick editor isolation (RV006)', () => {
  it('excludes current, inactive and deleted quick-only files regardless of cwd, but keeps legacy files', () => {
    const quickPaths = [
      '/projects/a/current-quick.ts',
      '/elsewhere/inactive-quick.ts',
      '/deleted-layout/draft.ts'
    ]
    const legacyPath = '/unowned/legacy.ts'
    for (const path of [...quickPaths, legacyPath]) {
      mockEditorState.openFiles.set(path, {
        ...createEditorFileState(path),
        isDirty: true,
        content: `draft for ${path}`
      })
    }
    for (const path of quickPaths) quickEditorPaths.add(path)
    mockEditorState.activeFilePath = quickPaths[0]

    persistState('project-a')

    expect(mockPersistenceWriteDebounced.mock.calls[0][1]).toMatchObject({
      openFiles: [{ filePath: legacyPath, draftContent: `draft for ${legacyPath}` }],
      activeFilePath: null
    })
    expect(mockPersistenceWriteDebounced.mock.calls[0][1].openFiles).toHaveLength(1)
  })

  it('keeps a shared dirty buffer explicitly referenced by any project leaf', () => {
    const path = '/shared/draft.ts'
    quickEditorPaths.add(path)
    mockEditorState.openFiles.set(path, {
      ...createEditorFileState(path),
      isDirty: true,
      content: 'shared unsaved content'
    })
    mockEditorState.activeFilePath = path
    mockWorkspaceState.root = {
      type: 'split',
      id: 'split-root',
      direction: 'horizontal',
      sizes: [50, 50],
      children: [
        { type: 'leaf', id: 'pane-root', tabs: [], activeTabId: null },
        {
          type: 'leaf',
          id: 'pane-inactive',
          tabs: [{ type: 'editor', id: `edit-${path}`, filePath: path }],
          activeTabId: `edit-${path}`
        }
      ]
    }

    persistState('project-a')

    expect(mockPersistenceWriteDebounced.mock.calls[0][1]).toMatchObject({
      openFiles: [{ filePath: path, isDirty: true, draftContent: 'shared unsaved content' }],
      activeFilePath: null
    })
  })

  it.each([
    ['quick-only', 'editor', '/projects/a/project.ts'],
    ['shared', 'editor', '/projects/a/project.ts'],
    ['shared', 'other-editor', '/projects/a/other.ts'],
    ['null', 'editor', '/projects/a/project.ts'],
    ['quick-only', 'terminal', null],
    ['shared', 'terminal', null],
    ['null', 'terminal', null],
    ['quick-only', 'missing-editor', null],
    ['legacy', 'terminal', '/unowned/legacy.ts']
  ] as const)('uses project selection for global %s and active project %s tab', (globalSelection, projectSelection, expected) => {
    const projectPath = '/projects/a/project.ts'
    const quickPath = '/projects/a/quick.ts'
    const legacyPath = '/unowned/legacy.ts'
    const otherProjectPath = '/projects/a/other.ts'
    for (const path of [projectPath, otherProjectPath, quickPath, legacyPath]) {
      mockEditorState.openFiles.set(path, createEditorFileState(path))
    }
    quickEditorPaths.add(quickPath)
    if (globalSelection === 'shared') quickEditorPaths.add(projectPath)
    mockEditorState.activeFilePath =
      globalSelection === 'null'
        ? null
        : globalSelection === 'shared'
          ? projectPath
          : globalSelection === 'legacy'
            ? legacyPath
            : quickPath
    mockWorkspaceState.root = {
      type: 'leaf',
      id: 'pane-root',
      tabs: [
        { type: 'editor', id: `edit-${projectPath}`, filePath: projectPath },
        { type: 'editor', id: `edit-${otherProjectPath}`, filePath: otherProjectPath },
        { type: 'terminal', id: 'term-project', terminalId: 'project' },
        { type: 'editor', id: 'edit-missing', filePath: '/missing.ts' }
      ],
      activeTabId:
        projectSelection === 'editor'
          ? `edit-${projectPath}`
          : projectSelection === 'other-editor'
            ? `edit-${otherProjectPath}`
            : projectSelection === 'terminal'
              ? 'term-project'
              : 'edit-missing'
    }

    persistState('project-a')

    expect(mockPersistenceWriteDebounced.mock.calls[0][1].activeFilePath).toBe(expected)
  })

  it.each([
    'payload-read',
    'file-open'
  ] as const)('preserves quick dirty memory inserted during project %s', async (phase) => {
    const path = '/shared/late.ts'
    quickEditorPaths.add(path)
    let release!: () => void
    const pending = new Promise<void>((resolve) => {
      release = resolve
    })
    const persisted = {
      success: true,
      data: {
        openFiles: [
          {
            filePath: path,
            cursorPosition: { line: 9, col: 9 },
            scrollTop: 999,
            viewMode: 'code',
            isDirty: true,
            lastModified: 999999,
            draftContent: 'stale project draft'
          }
        ],
        activeFilePath: null,
        expandedDirs: [],
        activeTabId: null
      }
    }
    mockPersistenceRead.mockImplementationOnce(async () => {
      if (phase === 'payload-read') await pending
      return persisted
    })
    if (phase === 'file-open')
      mockEditorState.openFile.mockImplementationOnce(async () => {
        await pending
      })
    renderHook(() => useEditorPersistence('project-a'))
    await waitFor(() =>
      expect(
        phase === 'payload-read' ? mockPersistenceRead : mockEditorState.openFile
      ).toHaveBeenCalled()
    )
    const live = {
      ...createEditorFileState(path),
      content: 'new quick draft',
      isDirty: true,
      lastModified: 1
    }
    mockEditorState.openFiles.set(path, live)
    release()
    await waitFor(() => expect(mockSetRestoredWorkspaceScope).toHaveBeenCalledWith('project-a'))
    expect(mockEditorState.openFiles.get(path)).toBe(live)
    expect(mockEditorState.updateContent).not.toHaveBeenCalledWith(path, 'stale project draft')
    expect(mockEditorState.updateCursorPosition).not.toHaveBeenCalledWith(path, 9, 9)
  })

  it.each([
    'legacy',
    'pane-layout'
  ] as const)('retains quick dirty/busy buffers through project changes without applying stale %s drafts', async (layout) => {
    const quickPath = '/projects/a/quick-only.ts'
    const sharedPath = '/shared/draft.ts'
    const savingPath = '/inactive/saving.ts'
    const reloadingPath = '/deleted/reloading.ts'
    const cleanPath = '/quick/clean.ts'
    const projectPath = '/projects/b/project.ts'
    for (const path of [quickPath, sharedPath, savingPath, reloadingPath, cleanPath]) {
      quickEditorPaths.add(path)
      mockEditorState.openFiles.set(path, createEditorFileState(path))
    }
    const quickDraft = {
      ...mockEditorState.openFiles.get(quickPath)!,
      isDirty: true,
      content: 'live quick'
    }
    const sharedDraft = {
      ...mockEditorState.openFiles.get(sharedPath)!,
      isDirty: true,
      content: 'live shared',
      lastModified: 1
    }
    mockEditorState.openFiles.set(quickPath, quickDraft)
    mockEditorState.openFiles.set(sharedPath, sharedDraft)
    mockEditorState.openFiles.get(savingPath)!.operationStatus = 'saving'
    mockEditorState.openFiles.get(reloadingPath)!.operationStatus = 'reloading'
    mockEditorState.openFiles.set('/old-project.ts', createEditorFileState('/old-project.ts'))
    const persistedFiles = [sharedPath, projectPath].map((filePath) => ({
      filePath,
      cursorPosition: { line: 9, col: 2 },
      scrollTop: 50,
      viewMode: 'code',
      isDirty: true,
      lastModified: 9999999999999,
      draftContent: 'stale persisted content'
    }))
    mockPersistenceRead.mockResolvedValueOnce({ success: true, data: null }).mockResolvedValueOnce({
      success: true,
      data: {
        openFiles: persistedFiles,
        activeFilePath: sharedPath,
        expandedDirs: [],
        activeTabId: `edit-${sharedPath}`,
        ...(layout === 'pane-layout'
          ? {
              paneLayout: {
                type: 'leaf',
                id: 'pane-b',
                tabs: [{ type: 'editor', filePath: sharedPath }],
                activeTabId: `edit-${sharedPath}`
              }
            }
          : {})
      }
    })
    const { rerender } = renderHook(({ projectId }) => useEditorPersistence(projectId), {
      initialProps: { projectId: 'project-a' }
    })
    await waitFor(() => {
      expect(mockSetRestoredWorkspaceScope).toHaveBeenCalledWith('project-a')
    })
    expect(mockEditorState.openFiles.get(quickPath)).toBe(quickDraft)
    expect(mockEditorState.openFiles.has(cleanPath)).toBe(false)
    expect(mockEditorState.openFiles.has('/old-project.ts')).toBe(false)

    rerender({ projectId: 'project-b' })

    await waitFor(() => {
      expect(mockSetRestoredWorkspaceScope).toHaveBeenCalledWith('project-b')
    })
    expect(mockEditorState.clearAllFiles).toHaveBeenCalledTimes(2)
    expect(mockEditorState.openFiles.get(quickPath)).toBe(quickDraft)
    expect(mockEditorState.openFiles.get(sharedPath)).toBe(sharedDraft)
    expect(mockEditorState.openFiles.get(savingPath)?.operationStatus).toBe('saving')
    expect(mockEditorState.openFiles.get(reloadingPath)?.operationStatus).toBe('reloading')
    expect(mockEditorState.openFile).not.toHaveBeenCalledWith(sharedPath)
    expect(mockEditorState.updateContent).not.toHaveBeenCalledWith(sharedPath, expect.anything())
    expect(mockEditorState.updateCursorPosition).not.toHaveBeenCalledWith(sharedPath, 9, 2)
    expect(mockEditorState.updateContent).toHaveBeenCalledWith(
      projectPath,
      'stale persisted content'
    )
    if (layout === 'legacy') {
      expect(mockWorkspaceState.syncEditorTabs).toHaveBeenCalledWith(
        [sharedPath, projectPath],
        `edit-${sharedPath}`
      )
    } else {
      expect(mockWorkspaceState.loadProjectWorkspace).toHaveBeenCalledWith(
        expect.objectContaining({
          tabs: [{ type: 'editor', id: `edit-${sharedPath}`, filePath: sharedPath }]
        }),
        undefined
      )
    }
  })
})

describe('useEditorPersistence', () => {
  it('restores only expanded dirs within active project root', async () => {
    mockPersistenceRead.mockResolvedValue({
      success: true,
      data: {
        openFiles: [],
        activeFilePath: null,
        expandedDirs: ['/projects/a', '/projects/a/src', '/projects/b/src', '/outside/path'],
        activeTabId: null
      }
    })

    renderHook(() => useEditorPersistence('project-a'))

    await waitFor(() => {
      expect(mockExplorerState.setExpandedDirs).toHaveBeenCalledWith(
        new Set(['/projects/a', '/projects/a/src'])
      )
      expect(mockExplorerState.restoreExpandedDirs).toHaveBeenCalledWith([
        '/projects/a',
        '/projects/a/src'
      ])
    })
  })

  it('restores expanded dirs from every root in a group workspace', async () => {
    mockPersistenceRead.mockResolvedValue({
      success: true,
      data: {
        openFiles: [],
        activeFilePath: null,
        expandedDirs: ['/projects/a/src', '/projects/b/docs', '/outside/path'],
        activeTabId: null
      }
    })

    renderHook(() =>
      useEditorPersistence('group-workspace', {
        projectIds: ['project-a', 'project-b'],
        rootPaths: ['/projects/a', '/projects/b'],
        manifestProjectId: null,
        notificationProjectId: 'project-a'
      })
    )

    await waitFor(() => {
      expect(mockPersistenceRead).toHaveBeenCalledWith('editor-state/group-workspace')
      expect(mockExplorerState.restoreExpandedDirs).toHaveBeenCalledWith([
        '/projects/a/src',
        '/projects/b/docs'
      ])
    })
  })

  it('ignores legacy fileExplorerVisible in persisted payload', async () => {
    mockExplorerState.isVisible = true

    mockPersistenceRead.mockResolvedValue({
      success: true,
      data: {
        openFiles: [],
        activeFilePath: null,
        expandedDirs: ['/projects/a/src'],
        fileExplorerVisible: false,
        activeTabId: null
      }
    })

    renderHook(() => useEditorPersistence('project-a'))

    await waitFor(() => {
      expect(mockExplorerState.restoreExpandedDirs).toHaveBeenCalledWith(['/projects/a/src'])
    })

    expect(mockExplorerState.isVisible).toBe(true)
  })

  it('keeps expanded dir persistence isolated per project', async () => {
    mockPersistenceRead
      .mockResolvedValueOnce({
        success: true,
        data: {
          openFiles: [],
          activeFilePath: null,
          expandedDirs: ['/projects/a/src'],
          activeTabId: null
        }
      })
      .mockResolvedValueOnce({
        success: true,
        data: {
          openFiles: [],
          activeFilePath: null,
          expandedDirs: ['/projects/b/docs'],
          activeTabId: null
        }
      })

    const { rerender } = renderHook(({ projectId }) => useEditorPersistence(projectId), {
      initialProps: { projectId: 'project-a' }
    })

    await waitFor(() => {
      expect(mockExplorerState.restoreExpandedDirs).toHaveBeenLastCalledWith(['/projects/a/src'])
    })

    rerender({ projectId: 'project-b' })

    await waitFor(() => {
      expect(mockExplorerState.restoreExpandedDirs).toHaveBeenLastCalledWith(['/projects/b/docs'])
    })
  })

  it('persists pane layout with mixed editor and terminal tabs', () => {
    mockEditorState.openFiles.set(
      '/projects/a/src/index.ts',
      createEditorFileState('/projects/a/src/index.ts')
    )
    mockEditorState.openFiles.set(
      '/projects/a/README.md',
      createEditorFileState('/projects/a/README.md')
    )
    mockEditorState.activeFilePath = '/projects/a/src/index.ts'
    mockExplorerState.expandedDirs = new Set(['/projects/a', '/projects/a/src'])

    mockWorkspaceState.root = {
      type: 'split',
      id: 'split-root',
      direction: 'horizontal',
      sizes: [55, 45],
      children: [
        {
          type: 'leaf',
          id: 'pane-left',
          tabs: [
            { type: 'terminal', id: 'term-old-1', terminalId: 'old-1' },
            {
              type: 'editor',
              id: 'edit-/projects/a/src/index.ts',
              filePath: '/projects/a/src/index.ts'
            }
          ],
          activeTabId: 'term-old-1'
        },
        {
          type: 'leaf',
          id: 'pane-right',
          tabs: [
            {
              type: 'editor',
              id: 'edit-/projects/a/README.md',
              filePath: '/projects/a/README.md'
            }
          ],
          activeTabId: 'edit-/projects/a/README.md'
        }
      ]
    }
    mockWorkspaceState.activePaneId = 'pane-left'

    persistState('project-a')

    expect(mockPersistenceWriteDebounced).toHaveBeenCalledTimes(1)
    expect(mockPersistenceWriteDebounced).toHaveBeenCalledWith(
      'editor-state/project-a',
      expect.objectContaining({
        activePaneId: 'pane-left',
        paneLayout: expect.objectContaining({
          type: 'split',
          id: 'split-root',
          direction: 'horizontal'
        })
      })
    )

    const payload = mockPersistenceWriteDebounced.mock.calls[0][1]
    const leftLeaf = payload.paneLayout.children[0]
    expect(leftLeaf.tabs).toEqual([
      { type: 'terminal', terminalId: 'old-1' },
      { type: 'editor', filePath: '/projects/a/src/index.ts' }
    ])
  })

  it("saves the project's set-aside expanded directories while the tree shows an outside folder", () => {
    mockExplorerState.expandedDirs = new Set(['/quick/terminal/sub'])
    mockExplorerState.setAsideExpandedDirs = new Set(['/projects/a/src'])
    try {
      persistState('project-a')
    } finally {
      mockExplorerState.expandedDirs = new Set<string>()
      mockExplorerState.setAsideExpandedDirs = null
    }

    expect(mockPersistenceWriteDebounced).toHaveBeenCalledWith(
      'editor-state/project-a',
      expect.objectContaining({ expandedDirs: ['/projects/a/src'] })
    )
  })

  it('does not persist the project pane tree while a Conversation is active', () => {
    mockConversationState.activeConversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
    persistState('project-a')
    expect(mockPersistenceWriteDebounced).not.toHaveBeenCalled()
  })

  it('restores a view only when it differs from how the file opens', async () => {
    const persistedFile = (filePath: string, viewMode: 'code' | 'markdown' | 'preview') => ({
      filePath,
      cursorPosition: { line: 1, col: 1 },
      scrollTop: 0,
      viewMode,
      isDirty: false,
      lastModified: 10
    })
    mockPersistenceRead.mockResolvedValue({
      success: true,
      data: {
        openFiles: [
          persistedFile('/projects/a/icon.svg', 'code'),
          persistedFile('/projects/a/logo.svg', 'preview'),
          persistedFile('/projects/a/README.md', 'markdown'),
          persistedFile('/projects/a/main.ts', 'code')
        ],
        activeFilePath: null,
        expandedDirs: [],
        activeTabId: null,
        paneLayout: { type: 'leaf', id: 'pane-empty', tabs: [], activeTabId: null }
      }
    })

    renderHook(() => useEditorPersistence('project-a'))

    await waitFor(() => {
      expect(mockEditorState.openFile).toHaveBeenCalledWith('/projects/a/main.ts')
    })
    expect(mockEditorState.setViewMode.mock.calls).toEqual([['/projects/a/icon.svg', 'code']])
  })

  it('notifies listeners after a project pane restore finishes', async () => {
    mockPersistenceRead.mockResolvedValue({
      success: true,
      data: {
        openFiles: [],
        activeFilePath: null,
        expandedDirs: [],
        activeTabId: null,
        paneLayout: {
          type: 'leaf',
          id: 'pane-empty',
          tabs: [],
          activeTabId: null
        }
      }
    })
    const restored: string[] = []
    const unsubscribe = subscribeProjectWorkspaceRestored((projectId) => {
      restored.push(projectId)
    })

    renderHook(() => useEditorPersistence('project-a'))

    await waitFor(() => {
      expect(restored).toEqual(['project-a'])
    })
    unsubscribe()
  })

  it('restores pane layout, remaps terminal tabs to live terminals, and prunes missing editor tabs', async () => {
    mockPersistenceRead.mockResolvedValue({
      success: true,
      data: {
        openFiles: [
          {
            filePath: '/projects/a/src/existing.ts',
            cursorPosition: { line: 1, col: 1 },
            scrollTop: 0,
            viewMode: 'code',
            isDirty: false,
            lastModified: 10
          }
        ],
        activeFilePath: '/projects/a/src/existing.ts',
        expandedDirs: ['/projects/a/src'],
        activeTabId: null,
        activePaneId: 'pane-drop',
        paneLayout: {
          type: 'split',
          id: 'split-1',
          direction: 'horizontal',
          sizes: [50, 50],
          children: [
            {
              type: 'leaf',
              id: 'pane-keep',
              tabs: [
                { type: 'terminal', terminalId: 'old-1' },
                { type: 'editor', filePath: '/projects/a/src/existing.ts' }
              ],
              activeTabId: 'term-old-1'
            },
            {
              type: 'leaf',
              id: 'pane-drop',
              tabs: [{ type: 'editor', filePath: '/projects/a/src/missing.ts' }],
              activeTabId: 'edit-/projects/a/src/missing.ts'
            }
          ]
        }
      }
    })

    mockTerminalState.terminals = [
      {
        id: 'live-1',
        name: 'Claude',
        projectId: 'project-a',
        shell: 'bash',
        cwd: '/projects/a',
        ptyId: 'pty-live-1'
      }
    ]
    mockLoadPersistedTerminals.mockResolvedValue({
      activeTerminalId: 'old-1',
      terminals: [
        {
          id: 'old-1',
          name: 'Claude',
          shell: 'bash',
          cwd: '/projects/a',
          scrollback: []
        }
      ],
      updatedAt: '2026-03-09T00:00:00.000Z'
    })

    renderHook(() => useEditorPersistence('project-a'))

    await waitFor(() => {
      expect(mockWorkspaceState.loadProjectWorkspace).toHaveBeenCalled()
    })

    const [restoredRootArg, activePaneIdArg] = mockWorkspaceState.loadProjectWorkspace.mock.calls[0]

    expect(activePaneIdArg).toBe('pane-drop')

    const restoredRoot = restoredRootArg as SplitNode
    expect(restoredRoot.type).toBe('split')

    const leftLeaf = restoredRoot.children[0] as LeafNode
    const rightLeaf = restoredRoot.children[1] as LeafNode

    expect(leftLeaf.type).toBe('leaf')
    expect(leftLeaf.id).toBe('pane-keep')
    expect(leftLeaf.tabs).toEqual([
      { type: 'terminal', id: 'term-live-1', terminalId: 'live-1' },
      {
        type: 'editor',
        id: 'edit-/projects/a/src/existing.ts',
        filePath: '/projects/a/src/existing.ts'
      }
    ])
    expect(leftLeaf.activeTabId).toBe('term-live-1')

    expect(rightLeaf.type).toBe('leaf')
    expect(rightLeaf.id).toBe('pane-drop')
    expect(rightLeaf.tabs).toEqual([])
  })

  it('keeps persisted terminal tabs during recovery-only restore when no live terminal exists', async () => {
    mockPersistenceRead.mockResolvedValue({
      success: true,
      data: {
        openFiles: [],
        activeFilePath: null,
        expandedDirs: ['/projects/a'],
        fileExplorerVisible: true,
        activeTabId: null,
        activePaneId: 'pane-recovery',
        paneLayout: {
          type: 'leaf',
          id: 'pane-recovery',
          tabs: [{ type: 'terminal', terminalId: 'persisted-only' }],
          activeTabId: 'term-persisted-only'
        }
      }
    })
    mockLoadPersistedTerminals.mockResolvedValue({
      activeTerminalId: 'persisted-only',
      terminals: [
        {
          id: 'persisted-only',
          name: 'Recovered terminal',
          shell: 'bash',
          cwd: '/projects/a',
          scrollback: []
        }
      ],
      updatedAt: '2026-03-09T00:00:00.000Z'
    })

    renderHook(() => useEditorPersistence('project-a'))

    await waitFor(() => {
      expect(mockWorkspaceState.loadProjectWorkspace).toHaveBeenCalled()
    })

    const [restoredRootArg, activePaneIdArg] = mockWorkspaceState.loadProjectWorkspace.mock.calls[0]
    expect(activePaneIdArg).toBe('pane-recovery')
    expect(restoredRootArg).toEqual({
      type: 'leaf',
      id: 'pane-recovery',
      tabs: [
        {
          type: 'terminal',
          id: 'term-persisted-only',
          terminalId: 'persisted-only'
        }
      ],
      activeTabId: 'term-persisted-only'
    })
  })

  it('ignores stale restore results after switching projects', async () => {
    const projectARead = {
      resolve: undefined as
        | ((value: {
            success: true
            data: {
              openFiles: never[]
              activeFilePath: null
              expandedDirs: string[]
              fileExplorerVisible: boolean
              activeTabId: null
              activePaneId: string
              paneLayout: {
                type: 'leaf'
                id: string
                tabs: { type: 'editor'; filePath: string }[]
                activeTabId: string
              }
            }
          }) => void)
        | undefined
    }

    mockPersistenceRead
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            projectARead.resolve = resolve
          })
      )
      .mockResolvedValueOnce({ success: true, data: null })
      .mockResolvedValueOnce({
        success: true,
        data: {
          openFiles: [],
          activeFilePath: null,
          expandedDirs: ['/projects/b'],
          fileExplorerVisible: true,
          activeTabId: null,
          activePaneId: 'pane-b',
          paneLayout: {
            type: 'leaf',
            id: 'pane-b',
            tabs: [{ type: 'editor', filePath: '/projects/b/src/index.ts' }],
            activeTabId: 'edit-/projects/b/src/index.ts'
          }
        }
      })

    const { rerender } = renderHook(
      ({ currentProjectId }) => useEditorPersistence(currentProjectId),
      {
        initialProps: { currentProjectId: 'project-a' }
      }
    )

    rerender({ currentProjectId: 'project-b' })

    await waitFor(() => {
      expect(mockWorkspaceState.resetLayout).toHaveBeenCalledTimes(1)
    })

    projectARead.resolve?.({
      success: true,
      data: {
        openFiles: [],
        activeFilePath: null,
        expandedDirs: ['/projects/a'],
        fileExplorerVisible: true,
        activeTabId: null,
        activePaneId: 'pane-a',
        paneLayout: {
          type: 'leaf',
          id: 'pane-a',
          tabs: [{ type: 'editor', filePath: '/projects/a/src/index.ts' }],
          activeTabId: 'edit-/projects/a/src/index.ts'
        }
      }
    })

    await waitFor(() => {
      expect(mockPersistenceRead).toHaveBeenCalledTimes(2)
    })

    expect(mockWorkspaceState.loadProjectWorkspace).not.toHaveBeenCalled()
  })

  it('restores legacy pane layout entries that use editorFilePaths', async () => {
    mockPersistenceRead.mockResolvedValue({
      success: true,
      data: {
        openFiles: [
          {
            filePath: '/projects/a/src/legacy.ts',
            cursorPosition: { line: 1, col: 1 },
            scrollTop: 0,
            viewMode: 'code',
            isDirty: false,
            lastModified: 10
          }
        ],
        activeFilePath: '/projects/a/src/legacy.ts',
        expandedDirs: ['/projects/a/src'],
        activeTabId: null,
        activePaneId: 'pane-legacy',
        paneLayout: {
          type: 'leaf',
          id: 'pane-legacy',
          editorFilePaths: ['/projects/a/src/legacy.ts'],
          activeTabId: 'edit-/projects/a/src/legacy.ts'
        }
      }
    })

    renderHook(() => useEditorPersistence('project-a'))

    await waitFor(() => {
      expect(mockWorkspaceState.loadProjectWorkspace).toHaveBeenCalled()
    })

    const [restoredRootArg, activePaneIdArg] = mockWorkspaceState.loadProjectWorkspace.mock.calls[0]

    expect(activePaneIdArg).toBe('pane-legacy')
    expect(restoredRootArg).toEqual({
      type: 'leaf',
      id: 'pane-legacy',
      tabs: [
        {
          type: 'editor',
          id: 'edit-/projects/a/src/legacy.ts',
          filePath: '/projects/a/src/legacy.ts'
        }
      ],
      activeTabId: 'edit-/projects/a/src/legacy.ts'
    })
  })

  it('resets pane layout when destination project has no persisted state', async () => {
    mockWorkspaceState.root = {
      type: 'split',
      id: 'split-old',
      direction: 'horizontal',
      sizes: [50, 50],
      children: [
        {
          type: 'leaf',
          id: 'pane-left',
          tabs: [{ type: 'terminal', id: 'term-old-1', terminalId: 'old-1' }],
          activeTabId: 'term-old-1'
        },
        {
          type: 'leaf',
          id: 'pane-right',
          tabs: [
            {
              type: 'editor',
              id: 'edit-/projects/a/src/leftover.ts',
              filePath: '/projects/a/src/leftover.ts'
            }
          ],
          activeTabId: 'edit-/projects/a/src/leftover.ts'
        }
      ]
    }
    mockWorkspaceState.activePaneId = 'pane-left'

    mockPersistenceRead.mockResolvedValue({
      success: true,
      data: null
    })

    renderHook(() => useEditorPersistence('project-b'))

    await waitFor(() => {
      expect(mockWorkspaceState.resetLayout).toHaveBeenCalledTimes(1)
    })
  })

  // Stage-5 cutover: preserved project manifests are inspection-only. Their editor evidence may
  // be opened, but the Conversation-independent project pane tree is never restored as live state.
  it('inspects a legacy manifest but restores the renderer-local paneLayout instead', async () => {
    mockPersistenceRead.mockResolvedValue({
      success: true,
      data: {
        openFiles: [],
        activeFilePath: null,
        expandedDirs: [],
        activeTabId: null,
        paneLayout: {
          type: 'leaf',
          id: 'legacy-leaf',
          tabs: [{ type: 'editor', filePath: '/projects/a/legacy.ts' }],
          activeTabId: 'edit-/projects/a/legacy.ts'
        }
      }
    })
    mockGetManifest.mockResolvedValue({
      success: true,
      data: {
        projectId: 'project-a',
        revision: 3,
        updatedAt: 1,
        topology: {
          type: 'leaf',
          id: 'manifest-leaf-A',
          terminalIds: [],
          editorIds: ['edit-/manifest/file.ts'],
          activeTabId: null
        },
        activePaneId: 'manifest-leaf-A',
        focusedSessionId: null,
        terminals: [],
        editors: [{ editorId: 'edit-/manifest/file.ts', filePath: '/manifest/file.ts' }]
      }
    })

    renderHook(() => useEditorPersistence('project-a'))

    await waitFor(() => {
      expect(mockGetManifest).toHaveBeenCalledWith('project-a')
    })

    await waitFor(() => {
      expect(mockWorkspaceState.loadProjectWorkspace).toHaveBeenCalledTimes(1)
    })
    const [restoredRootArg] = mockWorkspaceState.loadProjectWorkspace.mock.calls[0]
    expect((restoredRootArg as { id: string }).id).toBe('legacy-leaf')
    expect(mockLoadPersistedTerminals).toHaveBeenCalled()
    expect(mockWorkspaceState.resetLayout).not.toHaveBeenCalled()

    // Preserved manifest editor paths remain inspectable, without applying its pane topology.
    expect(mockEditorState.openFile).toHaveBeenCalledWith('/manifest/file.ts')
  })

  // P11: when no manifest exists (data: null), the legacy paneLayout fallback runs.
  it('falls back to the legacy paneLayout path when no manifest exists', async () => {
    mockPersistenceRead.mockResolvedValue({
      success: true,
      data: {
        openFiles: [],
        activeFilePath: null,
        expandedDirs: [],
        activeTabId: null,
        paneLayout: {
          type: 'leaf',
          id: 'legacy-leaf',
          tabs: [{ type: 'editor', filePath: '/projects/a/legacy.ts' }],
          activeTabId: 'edit-/projects/a/legacy.ts'
        }
      }
    })
    mockGetManifest.mockResolvedValue({ success: true, data: null })
    mockLoadPersistedTerminals.mockResolvedValue(null)

    renderHook(() => useEditorPersistence('project-a'))

    await waitFor(() => {
      expect(mockGetManifest).toHaveBeenCalledWith('project-a')
      // The legacy path runs — loadPersistedTerminals + loadProjectWorkspace.
      expect(mockLoadPersistedTerminals).toHaveBeenCalledWith('project-a')
      expect(mockWorkspaceState.loadProjectWorkspace).toHaveBeenCalledTimes(1)
    })
  })

  /**
   * A group scope passes `manifestProjectId: null` (WorkspaceLayout only bridges
   * the host manifest for a single project). Keying the restore guard on that
   * meant the guard was never raised in group mode — and the guard is what the
   * pane area reads to stop rendering the OUTGOING project's tabs while the
   * destination restores. The scope being restored always has a project id;
   * that is what the guard has to be keyed on.
   */
  it('raises the restore guard in a group scope, where manifestProjectId is null', async () => {
    mockPersistenceRead.mockResolvedValue({
      success: true,
      data: { openFiles: [], activeFilePath: null, expandedDirs: [], activeTabId: null }
    })
    mockGetManifest.mockResolvedValue({ success: true, data: null })

    renderHook(() =>
      useEditorPersistence('project-a', {
        projectIds: ['project-a', 'project-b'],
        rootPaths: [],
        manifestProjectId: null,
        notificationProjectId: 'project-a'
      })
    )

    await waitFor(() => {
      expect(mockSetManifestRestoreInProgress).toHaveBeenCalledWith('project-a', true)
    })
    await waitFor(() => {
      expect(mockSetManifestRestoreInProgress).toHaveBeenCalledWith('project-a', false)
    })
  })

  // P9: the restore flow sets setManifestRestoreInProgress(projectId, true)
  // before the manifest load and (projectId, false) in the finally.
  it('sets manifestRestoreInProgress true on entry and false in finally', async () => {
    mockPersistenceRead.mockResolvedValue({
      success: true,
      data: {
        openFiles: [],
        activeFilePath: null,
        expandedDirs: [],
        activeTabId: null
      }
    })
    mockGetManifest.mockResolvedValue({ success: true, data: null })

    renderHook(() => useEditorPersistence('project-a'))

    await waitFor(() => {
      expect(mockGetManifest).toHaveBeenCalled()
    })

    // true is set before the load, false in the finally.
    expect(mockSetManifestRestoreInProgress).toHaveBeenCalledWith('project-a', true)
    expect(mockSetManifestRestoreInProgress).toHaveBeenCalledWith('project-a', false)

    // Verify call order: true before false.
    const calls = mockSetManifestRestoreInProgress.mock.calls
    const trueIndex = calls.findIndex(
      (call: unknown[]) => call[0] === 'project-a' && call[1] === true
    )
    const falseIndex = calls.findIndex(
      (call: unknown[]) => call[0] === 'project-a' && call[1] === false
    )
    expect(trueIndex).toBeGreaterThanOrEqual(0)
    expect(falseIndex).toBeGreaterThanOrEqual(0)
    expect(trueIndex).toBeLessThan(falseIndex)
  })

  // The pane area gates on "does the tree on screen belong to the active
  // project", which only this hook can answer.
  it('claims the workspace scope once the restore has landed', async () => {
    mockPersistenceRead.mockResolvedValue({
      success: true,
      data: { openFiles: [], activeFilePath: null, expandedDirs: [], activeTabId: null }
    })
    mockGetManifest.mockResolvedValue({ success: true, data: null })

    renderHook(() => useEditorPersistence('project-a'))

    await waitFor(() => {
      expect(mockSetRestoredWorkspaceScope).toHaveBeenCalledWith('project-a')
    })
  })

  // A run overtaken by a run for a DIFFERENT project must stay quiet: the tree
  // it was building never reached the screen, and claiming it would drop the
  // skeleton back onto the wrong project's tabs.
  it('does not claim the workspace scope for a restore another project superseded', async () => {
    let releaseFirstRead = (): void => {}
    const firstRead = new Promise<void>((resolve) => {
      releaseFirstRead = resolve
    })
    const emptyState = {
      success: true,
      data: { openFiles: [], activeFilePath: null, expandedDirs: [], activeTabId: null }
    }
    mockPersistenceRead
      .mockImplementationOnce(async () => {
        await firstRead
        return emptyState
      })
      .mockResolvedValue(emptyState)
    mockGetManifest.mockResolvedValue({ success: true, data: null })

    const { rerender } = renderHook(({ projectId }) => useEditorPersistence(projectId), {
      initialProps: { projectId: 'project-a' }
    })
    await waitFor(() => {
      expect(mockSetManifestRestoreInProgress).toHaveBeenCalledWith('project-a', true)
    })

    // project-a is still parked on its read when project-b takes over.
    rerender({ projectId: 'project-b' })
    await waitFor(() => {
      expect(mockSetRestoredWorkspaceScope).toHaveBeenCalledWith('project-b')
    })

    releaseFirstRead()
    await waitFor(() => {
      expect(mockSetManifestRestoreInProgress).toHaveBeenCalledWith('project-a', false)
    })
    expect(mockSetRestoredWorkspaceScope).not.toHaveBeenCalledWith('project-a')
  })
})
