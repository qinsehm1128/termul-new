import type { DirectoryEntry } from '@shared/types/filesystem.types'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const mockEntries: DirectoryEntry[] = [
  {
    name: 'src',
    path: '/project/src',
    type: 'directory',
    extension: null,
    size: 0,
    modifiedAt: 1000
  },
  {
    name: 'index.ts',
    path: '/project/index.ts',
    type: 'file',
    extension: '.ts',
    size: 100,
    modifiedAt: 1000
  }
]

const { mockApi } = vi.hoisted(() => ({
  mockApi: {
    filesystem: {
      readDirectory: vi.fn(),
      renameFile: vi.fn(),
      watchDirectory: vi.fn(),
      unwatchDirectory: vi.fn(),
      searchContentStreamCancel: vi.fn(),
      searchFileNamesStreamCancel: vi.fn(),
      onSearchContentBatch: vi.fn(() => () => {}),
      onSearchContentDone: vi.fn(() => () => {}),
      onSearchFileNamesBatch: vi.fn(() => () => {}),
      onSearchFileNamesDone: vi.fn(() => () => {})
    }
  }
}))

vi.mock('@/lib/api', () => ({
  filesystemApi: mockApi.filesystem
}))

import { i18n } from '@/i18n'
import { isRejectedMove, useFileExplorerStore } from './file-explorer-store'

beforeEach(async () => {
  await i18n.changeLanguage('en')
  mockApi.filesystem.readDirectory
    .mockReset()
    .mockResolvedValue({ success: true, data: mockEntries })
  mockApi.filesystem.renameFile.mockReset().mockResolvedValue({ success: true, data: undefined })
  mockApi.filesystem.watchDirectory.mockReset().mockResolvedValue({ success: true })
  mockApi.filesystem.unwatchDirectory.mockReset().mockResolvedValue({ success: true })

  useFileExplorerStore.setState({
    roots: [],
    rootPath: null,
    expandedDirs: new Set<string>(),
    directoryContents: new Map<string, DirectoryEntry[]>(),
    selectedPaths: new Set<string>(),
    lastClickedPath: null,
    clipboard: null,
    isVisible: true,
    loadingDirs: new Set<string>(),
    rootLoadError: null,
    rootLoadErrors: new Map()
  })
})

describe('file-explorer-store', () => {
  describe('setRootPath', () => {
    it('should set root path and reset state', () => {
      const store = useFileExplorerStore.getState()
      store.setRootPath('/project')

      const state = useFileExplorerStore.getState()
      expect(state.rootPath).toBe('/project')
      expect(state.expandedDirs.size).toBe(0)
      expect(state.directoryContents.size).toBe(0)
      expect(state.selectedPaths.size).toBe(0)
      expect(state.rootLoadError).toBeNull()
    })

    it('should normalize backslashes in rootPath', () => {
      const store = useFileExplorerStore.getState()
      store.setRootPath('C:\\Users\\test\\project')

      expect(useFileExplorerStore.getState().rootPath).toBe('C:/Users/test/project')
    })

    it('should set rootPath to null', () => {
      const store = useFileExplorerStore.getState()
      store.setRootPath('/something')
      store.setRootPath(null)

      expect(useFileExplorerStore.getState().rootPath).toBeNull()
    })

    it('should unwatch previously expanded directories', () => {
      useFileExplorerStore.setState({
        expandedDirs: new Set(['/project', '/project/src'])
      })

      useFileExplorerStore.getState().setRootPath('/other')

      expect(mockApi.filesystem.unwatchDirectory).toHaveBeenCalledWith('/project')
      expect(mockApi.filesystem.unwatchDirectory).toHaveBeenCalledWith('/project/src')
    })
  })

  describe('multi-root project groups', () => {
    it('registers ordered roots and focuses the preferred root', () => {
      useFileExplorerStore.getState().setRoots(
        [
          { projectId: 'web', name: 'Web', path: '/workspace/web' },
          { projectId: 'api', name: 'API', path: '/workspace/api' }
        ],
        '/workspace/api'
      )

      const state = useFileExplorerStore.getState()
      expect(state.roots.map((root) => root.projectId)).toEqual(['web', 'api'])
      expect(state.rootPath).toBe('/workspace/api')
      expect(state.scopeRoot).toBe('/workspace/api')
    })

    it('changes the focused root without clearing loaded trees', () => {
      useFileExplorerStore.getState().setRoots([
        { projectId: 'web', name: 'Web', path: '/workspace/web' },
        { projectId: 'api', name: 'API', path: '/workspace/api' }
      ])
      useFileExplorerStore.setState({
        directoryContents: new Map([
          ['/workspace/web', mockEntries],
          ['/workspace/api', []]
        ])
      })

      useFileExplorerStore.getState().setFocusedRoot('/workspace/api')

      const state = useFileExplorerStore.getState()
      expect(state.rootPath).toBe('/workspace/api')
      expect(state.directoryContents.size).toBe(2)
    })

    it('refreshes every root and expanded child', async () => {
      useFileExplorerStore.getState().setRoots([
        { projectId: 'web', name: 'Web', path: '/workspace/web' },
        { projectId: 'api', name: 'API', path: '/workspace/api' }
      ])
      useFileExplorerStore.setState({
        expandedDirs: new Set(['/workspace/web', '/workspace/web/src', '/workspace/api'])
      })

      await useFileExplorerStore.getState().refreshTree()

      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledWith('/workspace/web')
      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledWith('/workspace/web/src')
      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledWith('/workspace/api')
    })
  })

  describe('toggleDirectory - expand', () => {
    it('should expand a directory and store its contents', async () => {
      await useFileExplorerStore.getState().toggleDirectory('/project')

      const state = useFileExplorerStore.getState()
      expect(state.expandedDirs.has('/project')).toBe(true)
      expect(state.directoryContents.get('/project')).toEqual(mockEntries)
    })

    it('should call readDirectory and watchDirectory on expand', async () => {
      await useFileExplorerStore.getState().toggleDirectory('/project')

      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledWith('/project')
      expect(mockApi.filesystem.watchDirectory).toHaveBeenCalledWith('/project')
    })

    it('should normalize backslash paths on expand', async () => {
      await useFileExplorerStore.getState().toggleDirectory('C:\\Users\\project')

      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledWith('C:/Users/project')
      expect(useFileExplorerStore.getState().expandedDirs.has('C:/Users/project')).toBe(true)
    })

    it('should clear loadingDirs after expand completes', async () => {
      await useFileExplorerStore.getState().toggleDirectory('/project')

      expect(useFileExplorerStore.getState().loadingDirs.size).toBe(0)
    })

    it('should clear loadingDirs even when readDirectory fails', async () => {
      mockApi.filesystem.readDirectory.mockRejectedValueOnce(new Error('fail'))

      try {
        await useFileExplorerStore.getState().toggleDirectory('/project')
      } catch {
        // Expected to throw
      }

      expect(useFileExplorerStore.getState().loadingDirs.size).toBe(0)
    })

    it('should not expand if already loading', async () => {
      useFileExplorerStore.setState({
        loadingDirs: new Set(['/project'])
      })

      await useFileExplorerStore.getState().toggleDirectory('/project')

      expect(mockApi.filesystem.readDirectory).not.toHaveBeenCalled()
    })

    it('should not watch when readDirectory returns success: false', async () => {
      mockApi.filesystem.readDirectory.mockResolvedValueOnce({
        success: false,
        error: 'denied',
        code: 'PATH_INVALID'
      })

      await useFileExplorerStore.getState().toggleDirectory('/project')

      expect(mockApi.filesystem.watchDirectory).not.toHaveBeenCalled()
      expect(useFileExplorerStore.getState().expandedDirs.size).toBe(0)
    })

    it('should set rootLoadError for root failure when readDirectory returns success: false', async () => {
      useFileExplorerStore.getState().setRootPath('/project')
      mockApi.filesystem.readDirectory.mockResolvedValueOnce({
        success: false,
        error: 'denied',
        code: 'PATH_INVALID'
      })

      await useFileExplorerStore.getState().toggleDirectory('/project')

      expect(useFileExplorerStore.getState().rootLoadError).toEqual({
        message: 'denied',
        code: 'PATH_INVALID'
      })
    })

    it('should set rootLoadError for root failure when readDirectory throws', async () => {
      useFileExplorerStore.getState().setRootPath('/project')
      mockApi.filesystem.readDirectory.mockRejectedValueOnce(new Error('boom'))

      await useFileExplorerStore.getState().toggleDirectory('/project')

      expect(useFileExplorerStore.getState().rootLoadError).toEqual({
        message: 'boom',
        code: 'UNKNOWN_ERROR'
      })
    })

    it('uses the English root-load fallback when the thrown value has no message', async () => {
      useFileExplorerStore.getState().setRootPath('/project')
      mockApi.filesystem.readDirectory.mockRejectedValueOnce(null)

      await useFileExplorerStore.getState().toggleDirectory('/project')

      expect(useFileExplorerStore.getState().rootLoadError).toEqual({
        message: 'Failed to load project files',
        code: 'UNKNOWN_ERROR'
      })
    })

    it('localizes the root-load fallback when the thrown value has no message', async () => {
      await i18n.changeLanguage('zh-CN')
      useFileExplorerStore.getState().setRootPath('/project')
      mockApi.filesystem.readDirectory.mockRejectedValueOnce(null)

      await useFileExplorerStore.getState().toggleDirectory('/project')

      expect(useFileExplorerStore.getState().rootLoadError).toEqual({
        message: '加载项目文件失败',
        code: 'UNKNOWN_ERROR'
      })
    })

    it('should clear rootLoadError after successful root reload', async () => {
      useFileExplorerStore.setState({
        rootPath: '/project',
        rootLoadError: { message: 'failed', code: 'UNKNOWN_ERROR' }
      })

      await useFileExplorerStore.getState().toggleDirectory('/project')

      expect(useFileExplorerStore.getState().rootLoadError).toBeNull()
      expect(useFileExplorerStore.getState().directoryContents.get('/project')).toEqual(mockEntries)
    })
  })

  describe('toggleDirectory - collapse', () => {
    beforeEach(async () => {
      // Expand root and a child so we have nested state
      await useFileExplorerStore.getState().toggleDirectory('/project')
      vi.clearAllMocks()

      // Manually add a child expanded dir
      const state = useFileExplorerStore.getState()
      const newExpanded = new Set(state.expandedDirs)
      newExpanded.add('/project/src')
      const newContents = new Map(state.directoryContents)
      newContents.set('/project/src', [])
      useFileExplorerStore.setState({
        expandedDirs: newExpanded,
        directoryContents: newContents
      })
    })

    it('should collapse a directory and defer content removal until finalize', async () => {
      await useFileExplorerStore.getState().toggleDirectory('/project')

      let state = useFileExplorerStore.getState()
      expect(state.expandedDirs.has('/project')).toBe(false)
      expect(state.directoryContents.has('/project')).toBe(true)
      expect(state.pendingCollapses.has('/project')).toBe(true)

      useFileExplorerStore.getState().finalizeDirectoryCollapse('/project')

      state = useFileExplorerStore.getState()
      expect(state.directoryContents.has('/project')).toBe(false)
      expect(state.pendingCollapses.has('/project')).toBe(false)
    })

    it('should also collapse child directories', async () => {
      await useFileExplorerStore.getState().toggleDirectory('/project')

      const state = useFileExplorerStore.getState()
      expect(state.expandedDirs.has('/project/src')).toBe(false)

      useFileExplorerStore.getState().finalizeDirectoryCollapse('/project')
      expect(useFileExplorerStore.getState().directoryContents.has('/project/src')).toBe(false)
    })

    it('should unwatch collapsed directory and its children after finalize', async () => {
      await useFileExplorerStore.getState().toggleDirectory('/project')
      vi.clearAllMocks()

      useFileExplorerStore.getState().finalizeDirectoryCollapse('/project')

      expect(mockApi.filesystem.unwatchDirectory).toHaveBeenCalledWith('/project')
      expect(mockApi.filesystem.unwatchDirectory).toHaveBeenCalledWith('/project/src')
    })

    it('should match child directories with normalized paths on Windows', async () => {
      // Set up state with Windows-style paths (already normalized by store)
      useFileExplorerStore.setState({
        expandedDirs: new Set(['C:/Users/project', 'C:/Users/project/src']),
        directoryContents: new Map([
          ['C:/Users/project', mockEntries],
          ['C:/Users/project/src', []]
        ])
      })

      // Collapse using backslash path — should normalize and match children
      await useFileExplorerStore.getState().toggleDirectory('C:\\Users\\project')

      const state = useFileExplorerStore.getState()
      expect(state.expandedDirs.has('C:/Users/project')).toBe(false)
      expect(state.expandedDirs.has('C:/Users/project/src')).toBe(false)
    })
  })

  describe('refreshDirectory', () => {
    it('should update contents for a directory', async () => {
      const newEntries: DirectoryEntry[] = [
        {
          name: 'new.ts',
          path: '/project/new.ts',
          type: 'file',
          extension: '.ts',
          size: 50,
          modifiedAt: 2000
        }
      ]
      mockApi.filesystem.readDirectory.mockResolvedValueOnce({ success: true, data: newEntries })

      useFileExplorerStore.setState({
        directoryContents: new Map([['/project', mockEntries]])
      })

      await useFileExplorerStore.getState().refreshDirectory('/project')

      expect(useFileExplorerStore.getState().directoryContents.get('/project')).toEqual(newEntries)
    })

    it('should normalize path before refresh', async () => {
      await useFileExplorerStore.getState().refreshDirectory('C:\\Users\\project')

      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledWith('C:/Users/project')
    })

    it('should not throw when refresh fails', async () => {
      mockApi.filesystem.readDirectory.mockRejectedValueOnce(new Error('fail'))

      await expect(
        useFileExplorerStore.getState().refreshDirectory('/project')
      ).resolves.not.toThrow()
    })
  })

  describe('refreshTree (GH-540)', () => {
    it('re-reads the root and every expanded directory', async () => {
      useFileExplorerStore.setState({
        rootPath: '/project',
        directoryContents: new Map<string, DirectoryEntry[]>([
          ['/project', mockEntries],
          ['/project/src', mockEntries],
          ['/project/docs', mockEntries]
        ]),
        expandedDirs: new Set(['/project/src', '/project/docs'])
      })

      await useFileExplorerStore.getState().refreshTree()

      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledWith('/project')
      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledWith('/project/src')
      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledWith('/project/docs')
      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledTimes(3)
    })

    it('preserves expanded state and selection', async () => {
      useFileExplorerStore.setState({
        rootPath: '/project',
        directoryContents: new Map<string, DirectoryEntry[]>([['/project', mockEntries]]),
        expandedDirs: new Set(['/project/src']),
        selectedPaths: new Set(['/project/index.ts']),
        lastClickedPath: '/project/index.ts'
      })

      await useFileExplorerStore.getState().refreshTree()

      const state = useFileExplorerStore.getState()
      expect(state.expandedDirs).toEqual(new Set(['/project/src']))
      expect(state.selectedPaths).toEqual(new Set(['/project/index.ts']))
      expect(state.lastClickedPath).toBe('/project/index.ts')
    })

    it('refreshes only the root when nothing else is expanded (no duplicates)', async () => {
      useFileExplorerStore.setState({
        rootPath: '/project',
        directoryContents: new Map<string, DirectoryEntry[]>([['/project', mockEntries]]),
        expandedDirs: new Set(['/project'])
      })

      await useFileExplorerStore.getState().refreshTree()

      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledTimes(1)
      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledWith('/project')
    })

    it('is a no-op when no project root is set', async () => {
      await useFileExplorerStore.getState().refreshTree()

      expect(mockApi.filesystem.readDirectory).not.toHaveBeenCalled()
    })

    it('skips directories that were collapsed while the refresh is running', async () => {
      useFileExplorerStore.setState({
        rootPath: '/project',
        directoryContents: new Map<string, DirectoryEntry[]>([
          ['/project', mockEntries],
          ['/project/src', mockEntries],
          ['/project/docs', mockEntries]
        ]),
        expandedDirs: new Set(['/project/src', '/project/docs'])
      })

      mockApi.filesystem.readDirectory.mockImplementation(async (path: string) => {
        if (path === '/project') {
          // User collapses /project/src while the refresh is in flight.
          useFileExplorerStore.setState({ expandedDirs: new Set(['/project/docs']) })
        }
        return { success: true, data: mockEntries }
      })

      await useFileExplorerStore.getState().refreshTree()

      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledWith('/project')
      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledWith('/project/docs')
      expect(mockApi.filesystem.readDirectory).not.toHaveBeenCalledWith('/project/src')
    })

    it('stops refreshing when the project root changes mid-refresh', async () => {
      useFileExplorerStore.setState({
        rootPath: '/project',
        directoryContents: new Map<string, DirectoryEntry[]>([['/project', mockEntries]]),
        expandedDirs: new Set(['/project/src', '/project/docs'])
      })

      mockApi.filesystem.readDirectory.mockImplementation(async (path: string) => {
        if (path === '/project') {
          useFileExplorerStore.setState({ rootPath: '/other-project' })
        }
        return { success: true, data: mockEntries }
      })

      await useFileExplorerStore.getState().refreshTree()

      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledTimes(1)
      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledWith('/project')
    })

    it('ignores concurrent refreshTree calls while one is in flight', async () => {
      useFileExplorerStore.setState({
        rootPath: '/project',
        directoryContents: new Map<string, DirectoryEntry[]>([['/project', mockEntries]]),
        expandedDirs: new Set<string>()
      })

      let releaseRead: ((value: { success: boolean; data: DirectoryEntry[] }) => void) | undefined
      const firstRead = new Promise<{ success: boolean; data: DirectoryEntry[] }>((resolve) => {
        releaseRead = resolve
      })
      mockApi.filesystem.readDirectory.mockReturnValueOnce(firstRead)

      const first = useFileExplorerStore.getState().refreshTree()
      expect(useFileExplorerStore.getState().refreshingTree).toBe(true)

      // Second call while the first is still awaiting must be a no-op.
      await useFileExplorerStore.getState().refreshTree()
      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledTimes(1)

      releaseRead?.({ success: true, data: mockEntries })
      await first
      expect(useFileExplorerStore.getState().refreshingTree).toBe(false)
    })
  })

  describe('restoreExpandedDirs', () => {
    it('should restore only directories within root and skip missing paths', async () => {
      useFileExplorerStore.getState().setRootPath('/project')

      mockApi.filesystem.readDirectory.mockImplementation(async (path: string) => {
        if (path === '/project/src') {
          return { success: true, data: [] }
        }

        if (path === '/project/missing') {
          throw new Error('missing')
        }

        return { success: true, data: mockEntries }
      })

      await useFileExplorerStore
        .getState()
        .restoreExpandedDirs(['/project', '/project/src', '/project/missing', '/other/src'])

      const state = useFileExplorerStore.getState()
      expect(state.expandedDirs.has('/project/src')).toBe(true)
      expect(state.expandedDirs.has('/other/src')).toBe(false)
      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledWith('/project/src')
      expect(mockApi.filesystem.readDirectory).toHaveBeenCalledWith('/project/missing')
    })
  })

  describe('collapseAll', () => {
    it('should collapse everything except root', async () => {
      // Set up expanded tree
      useFileExplorerStore.setState({
        rootPath: '/project',
        expandedDirs: new Set(['/project', '/project/src', '/project/src/lib']),
        directoryContents: new Map([
          ['/project', mockEntries],
          ['/project/src', []],
          ['/project/src/lib', []]
        ])
      })

      useFileExplorerStore.getState().collapseAll()

      const state = useFileExplorerStore.getState()
      expect(state.expandedDirs.size).toBe(1)
      expect(state.expandedDirs.has('/project')).toBe(true)
      expect(state.directoryContents.size).toBe(1)
      expect(state.directoryContents.has('/project')).toBe(true)
    })

    it('should unwatch non-root directories', () => {
      useFileExplorerStore.setState({
        rootPath: '/project',
        expandedDirs: new Set(['/project', '/project/src'])
      })

      useFileExplorerStore.getState().collapseAll()

      expect(mockApi.filesystem.unwatchDirectory).toHaveBeenCalledWith('/project/src')
      expect(mockApi.filesystem.unwatchDirectory).not.toHaveBeenCalledWith('/project')
    })
  })

  describe('selectPath', () => {
    it('should set selected path', () => {
      useFileExplorerStore.getState().selectPath('/project/file.ts')
      expect(useFileExplorerStore.getState().selectedPaths.has('/project/file.ts')).toBe(true)
    })

    it('should clear selected path with null', () => {
      useFileExplorerStore.getState().selectPath('/project/file.ts')
      useFileExplorerStore.getState().selectPath(null)
      expect(useFileExplorerStore.getState().selectedPaths.size).toBe(0)
    })
  })

  describe('toggleVisibility', () => {
    it('should toggle isVisible', () => {
      expect(useFileExplorerStore.getState().isVisible).toBe(true)
      useFileExplorerStore.getState().toggleVisibility()
      expect(useFileExplorerStore.getState().isVisible).toBe(false)
      useFileExplorerStore.getState().toggleVisibility()
      expect(useFileExplorerStore.getState().isVisible).toBe(true)
    })
  })

  describe('setDirectoryContents / removeDirectoryContents', () => {
    it('should set and remove directory contents', () => {
      useFileExplorerStore.getState().setDirectoryContents('/dir', mockEntries)
      expect(useFileExplorerStore.getState().directoryContents.get('/dir')).toEqual(mockEntries)

      useFileExplorerStore.getState().removeDirectoryContents('/dir')
      expect(useFileExplorerStore.getState().directoryContents.has('/dir')).toBe(false)
    })
  })
  describe('searchInRoot - error code reset', () => {
    it('clears stale searchErrorCode when called with an empty query (trimmed.length < 2)', async () => {
      useFileExplorerStore.setState({
        rootPath: '/project',
        searchError: 'prior error',
        searchErrorCode: 'QUERY_TOO_LONG'
      })

      await useFileExplorerStore.getState().searchInRoot('', 0)

      const state = useFileExplorerStore.getState()
      expect(state.searchError).toBeNull()
      expect(state.searchErrorCode).toBeNull()
    })

    it('clears stale searchErrorCode when called with a single-char query', async () => {
      useFileExplorerStore.setState({
        rootPath: '/project',
        searchError: 'prior error',
        searchErrorCode: 'QUERY_TOO_LONG'
      })

      await useFileExplorerStore.getState().searchInRoot('a', 0)

      const state = useFileExplorerStore.getState()
      expect(state.searchError).toBeNull()
      expect(state.searchErrorCode).toBeNull()
    })

    it('clears stale searchErrorCode when no project is selected', async () => {
      useFileExplorerStore.setState({
        rootPath: null,
        scopeRoot: null,
        searchError: 'prior error',
        searchErrorCode: 'RG_STREAM_FAILED'
      })

      await useFileExplorerStore.getState().searchInRoot('term', 0)

      const state = useFileExplorerStore.getState()
      expect(state.searchError).toBe('No project selected')
      expect(state.searchErrorCode).toBeNull()
    })
  })
})

describe('isRejectedMove', () => {
  it('should reject a directory dropped onto itself', () => {
    expect(isRejectedMove('/project/src', '/project/src')).toBe(true)
  })

  it('should reject a directory dropped into its own subtree', () => {
    // rename() would recurse the tree into its own child and destroy it.
    expect(isRejectedMove('/project/src', '/project/src/components')).toBe(true)
  })

  it('should reject a drop back into the current parent', () => {
    expect(isRejectedMove('/project/src/index.ts', '/project/src')).toBe(true)
  })

  it('should allow a genuine move', () => {
    expect(isRejectedMove('/project/src/index.ts', '/project/lib')).toBe(false)
  })

  it('should not confuse a sibling with a prefix for a descendant', () => {
    // '/project/src-old' starts with '/project/src' as a raw string but is not
    // inside it.
    expect(isRejectedMove('/project/src', '/project/src-old')).toBe(false)
  })
})

describe('moveEntries', () => {
  it('should rename each entry into the target directory', async () => {
    await useFileExplorerStore
      .getState()
      .moveEntries(['/project/index.ts', '/project/readme.md'], '/project/src')

    expect(mockApi.filesystem.renameFile).toHaveBeenCalledWith(
      '/project/index.ts',
      '/project/src/index.ts'
    )
    expect(mockApi.filesystem.renameFile).toHaveBeenCalledWith(
      '/project/readme.md',
      '/project/src/readme.md'
    )
  })

  it('should skip rejected drops without touching the filesystem', async () => {
    await useFileExplorerStore.getState().moveEntries(['/project/src'], '/project/src/components')

    expect(mockApi.filesystem.renameFile).not.toHaveBeenCalled()
  })

  it('should refresh the source directory as well as the target', async () => {
    mockApi.filesystem.readDirectory.mockClear()

    await useFileExplorerStore.getState().moveEntries(['/project/src/a.ts'], '/project/lib')

    // Cut+paste only refreshes the destination; a drop leaves a hole in the
    // source directory the user is still looking at.
    const refreshed = mockApi.filesystem.readDirectory.mock.calls.map((call) => call[0])
    expect(refreshed).toContain('/project/src')
    expect(refreshed).toContain('/project/lib')
  })

  it('should keep moving after one entry fails', async () => {
    mockApi.filesystem.renameFile
      .mockResolvedValueOnce({ success: false, error: 'EACCES' })
      .mockResolvedValueOnce({ success: true, data: undefined })

    await useFileExplorerStore
      .getState()
      .moveEntries(['/project/locked.ts', '/project/free.ts'], '/project/src')

    // One unwritable entry must not strand a multi-select drop half-moved.
    expect(mockApi.filesystem.renameFile).toHaveBeenCalledWith(
      '/project/free.ts',
      '/project/src/free.ts'
    )
  })

  it('should not refresh anything when every drop was rejected', async () => {
    mockApi.filesystem.readDirectory.mockClear()

    await useFileExplorerStore.getState().moveEntries(['/project/src'], '/project/src')

    expect(mockApi.filesystem.readDirectory).not.toHaveBeenCalled()
  })
})
