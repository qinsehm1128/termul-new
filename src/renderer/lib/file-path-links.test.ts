import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  buildTerminalPathLinks,
  extractPathCandidate,
  openFilePathFromTerminal,
  resolveFilePathCandidate,
  stripLineColumnSuffix,
  trimWrappedPath
} from './file-path-links'

const mocks = vi.hoisted(() => ({
  getFileInfo: vi.fn(),
  openFile: vi.fn(),
  updateCursorPosition: vi.fn(),
  addEditorTab: vi.fn(),
  toastError: vi.fn(),
  searchStart: vi.fn(),
  searchCancel: vi.fn(),
  onBatch: vi.fn(),
  onDone: vi.fn(),
  logError: vi.fn(),
  getHomeDirectory: vi.fn()
}))

vi.mock('@/lib/api', () => ({
  systemApi: { getHomeDirectory: mocks.getHomeDirectory },
  filesystemApi: {
    getFileInfo: mocks.getFileInfo,
    searchFileNamesStreamStart: mocks.searchStart,
    searchFileNamesStreamCancel: mocks.searchCancel,
    onSearchFileNamesBatch: mocks.onBatch,
    onSearchFileNamesDone: mocks.onDone
  }
}))
vi.mock('@/lib/log-api', () => ({ logFrontendError: mocks.logError }))
vi.mock('@/stores/editor-store', () => ({
  useEditorStore: {
    getState: () => ({
      openFile: mocks.openFile,
      updateCursorPosition: mocks.updateCursorPosition
    })
  }
}))

vi.mock('@/stores/workspace-store', () => ({
  useWorkspaceStore: {
    getState: () => ({ addEditorTab: mocks.addEditorTab })
  }
}))

vi.mock('sonner', () => ({
  toast: {
    error: mocks.toastError
  }
}))

describe('file-path-links parsing', () => {
  it('does not treat apostrophes in contractions as path quote boundaries', () => {
    expect(
      buildTerminalPathLinks("can't open ./src/App.tsx because it's missing", 1, vi.fn()).map(
        ({ text }) => text
      )
    ).toEqual(['./src/App.tsx'])
  })
  it('keeps quoted space paths and suffixes contiguous', () => {
    const text = 'open "scripts/my task.ts":12:3 and `docs/my guide.md:4`'
    expect(buildTerminalPathLinks(text, 1, vi.fn()).map((link) => link.text)).toEqual([
      '"scripts/my task.ts":12:3',
      'docs/my guide.md:4'
    ])
    expect(extractPathCandidate('"scripts/my task.ts":12:3')).toBe('scripts/my task.ts')
    expect(extractPathCandidate('`docs/my guide.md:4`')).toBe('docs/my guide.md')
  })

  it('rejects URL fragments, non-hierarchical schemes, and standalone fragments', () => {
    expect(
      buildTerminalPathLinks(
        'https://example.com/#scripts/x.ts mailto:a/b.ts #docs/x.md "https://x/y/z.ts"',
        1,
        vi.fn()
      )
    ).toEqual([])
  })
  it('trims wrapped paths', () => {
    expect(trimWrappedPath('`src/foo.ts`')).toBe('src/foo.ts')
    expect(trimWrappedPath('"src/foo.ts"')).toBe('src/foo.ts')
    expect(trimWrappedPath('(src/foo.ts)')).toBe('src/foo.ts')
  })

  it('strips only one wrapping layer per call', () => {
    expect(trimWrappedPath('("src/foo.ts")')).toBe('"src/foo.ts"')
  })

  it('strips line and column suffixes', () => {
    expect(stripLineColumnSuffix('src/foo.ts:12')).toBe('src/foo.ts')
    expect(stripLineColumnSuffix('src/foo.ts:12:3')).toBe('src/foo.ts')
    expect(stripLineColumnSuffix('C:/repo/src/foo.ts:4:9')).toBe('C:/repo/src/foo.ts')
  })

  it('extracts wrapped paths with line and column suffixes', () => {
    expect(extractPathCandidate('(src/renderer/App.tsx:42:7)')).toBe('src/renderer/App.tsx')
    expect(extractPathCandidate('`./src/main.ts:3`')).toBe('./src/main.ts')
  })

  it('keeps plain file paths unchanged', () => {
    expect(extractPathCandidate('src/renderer/App.tsx')).toBe('src/renderer/App.tsx')
    expect(extractPathCandidate('./src/renderer/App.tsx')).toBe('./src/renderer/App.tsx')
  })

  it('does not extract file links from URL host/path fragments', () => {
    const links = buildTerminalPathLinks(
      'See https://example.com/src/App.tsx for details',
      1,
      vi.fn()
    )

    expect(links).toEqual([])
  })

  it('does not extract file links from file URLs', () => {
    const links = buildTerminalPathLinks('file:///c:/repo/src/App.tsx', 1, vi.fn())

    expect(links).toEqual([])
  })

  it('does not extract file links from wrapped URLs', () => {
    const links = buildTerminalPathLinks('(https://example.com/src/App.tsx)', 1, vi.fn())

    expect(links).toEqual([])
  })

  it('keeps labeled absolute paths outside URLs', () => {
    const links = buildTerminalPathLinks('Path: /repo/src/App.tsx', 1, vi.fn())

    expect(links).toHaveLength(1)
    expect(links[0]?.text).toBe('/repo/src/App.tsx')
  })

  it('keeps UNC-style file links outside URLs', () => {
    const links = buildTerminalPathLinks('\\\\server\\share\\workspace\\src\\App.tsx', 1, vi.fn())

    expect(links).toHaveLength(1)
    expect(links[0]?.text).toBe('\\\\server\\share\\workspace\\src\\App.tsx')
  })
})

describe('file-path-links resolution', () => {
  beforeEach(() => {
    mocks.getFileInfo.mockReset()
    mocks.openFile.mockReset()
    mocks.updateCursorPosition.mockReset()
    mocks.addEditorTab.mockReset()
    mocks.toastError.mockReset()
    mocks.getHomeDirectory.mockReset().mockResolvedValue({ success: true, data: '/Users/tester' })
    mocks.onBatch.mockReset().mockReturnValue(vi.fn())
    mocks.onDone.mockReset().mockReturnValue(vi.fn())
    mocks.searchCancel.mockReset().mockResolvedValue({ success: true })
    mocks.searchStart.mockReset().mockImplementation(async (searchId: string) => {
      const done = mocks.onDone.mock.calls.at(-1)?.[0]
      done?.({ searchId, truncated: false, totalFiles: 0 })
      return { success: true }
    })
  })

  function mockSearch(paths: string[], truncated = false): void {
    mocks.searchStart.mockImplementation(async (searchId: string) => {
      const batch = mocks.onBatch.mock.calls.at(-1)?.[0]
      const done = mocks.onDone.mock.calls.at(-1)?.[0]
      batch({
        searchId,
        files: paths.map((path) => ({ path, relativePath: path, name: path.split('/').at(-1) }))
      })
      done({ searchId, truncated, totalFiles: paths.length })
      return { success: true }
    })
    mocks.getFileInfo.mockImplementation(async (path: string) =>
      paths.includes(path)
        ? { success: true, data: { type: 'file' } }
        : { success: false, error: 'missing' }
    )
  }

  it('finds the unique exact suffix nested below the workspace root after direct misses', async () => {
    const path = '/repo/apps/backend/scripts/normalize-experience.ts'
    mockSearch([path, '/repo/apps/backend/scripts/not-normalize-experience.ts'])
    await expect(
      resolveFilePathCandidate('scripts/normalize-experience.ts:12:3', {
        cwd: '/repo/packages/client',
        workspaceCwd: '/repo',
        projectRoot: '/repo'
      })
    ).resolves.toEqual({ ok: true, path })
    expect(mocks.searchStart).toHaveBeenCalledTimes(1)
    expect(mocks.searchStart).toHaveBeenCalledWith(
      expect.any(String),
      '/repo',
      '/repo',
      'normalize-experience.ts',
      false
    )
  })

  it('keeps conversation workspace cwd as an exact root before searching', async () => {
    mocks.getFileInfo.mockImplementation(async (path: string) =>
      path === '/worktree/scripts/x.ts'
        ? { success: true, data: { type: 'file' } }
        : { success: false }
    )
    await expect(
      resolveFilePathCandidate('scripts/x.ts', {
        cwd: '/repo/packages/backend',
        workspaceCwd: '/worktree',
        projectRoot: '/repo'
      })
    ).resolves.toEqual({ ok: true, path: '/worktree/scripts/x.ts' })
    expect(mocks.searchStart).not.toHaveBeenCalled()
  })

  it('rejects multiple exact suffixes instead of opening an arbitrary first match', async () => {
    mockSearch(['/repo/apps/a/docs/x.md', '/repo/apps/b/docs/x.md'])
    await expect(openFilePathFromTerminal('docs/x.md', { cwd: '/repo' })).resolves.toMatchObject({
      ok: false,
      reason: 'ambiguous'
    })
    expect(mocks.openFile).not.toHaveBeenCalled()
    expect(mocks.searchCancel).toHaveBeenCalledTimes(1)
  })

  it('requires suffix uniqueness across all disjoint allowed roots', async () => {
    mockSearch(['/worktree/apps/scripts/x.ts', '/repo/apps/scripts/x.ts'])
    await expect(
      resolveFilePathCandidate('scripts/x.ts', {
        cwd: '/worktree',
        projectRoot: '/repo'
      })
    ).resolves.toEqual({ ok: false, reason: 'ambiguous' })
    expect(mocks.searchStart).toHaveBeenCalledTimes(2)
  })

  it('does not treat a filename substring as an exact path suffix', async () => {
    mockSearch(['/repo/apps/not-scripts/x.ts', '/repo/apps/scripts/other-x.ts'])
    await expect(resolveFilePathCandidate('scripts/x.ts', { cwd: '/repo' })).resolves.toEqual({
      ok: false,
      reason: 'not-found'
    })
  })

  it('rejects suffix hits outside the allowed root and canonical traversal', async () => {
    mockSearch(['/outside/scripts/x.ts', '/repo/../outside/scripts/x.ts'])
    await expect(resolveFilePathCandidate('scripts/x.ts', { cwd: '/repo' })).resolves.toEqual({
      ok: false,
      reason: 'not-found'
    })
    expect(mocks.getFileInfo).toHaveBeenCalledTimes(1)
  })

  it.each([
    '/custom/account',
    '/custom'
  ])('does not search actual platform home or its ancestor: %s', async (cwd) => {
    mocks.getHomeDirectory.mockResolvedValue({ success: true, data: '/custom/account' })
    mocks.getFileInfo.mockResolvedValue({ success: false })
    await resolveFilePathCandidate('scripts/x.ts', { cwd })
    expect(mocks.searchStart).not.toHaveBeenCalled()
  })

  it('does not search when the actual home boundary lookup fails', async () => {
    mocks.getHomeDirectory.mockResolvedValue({ success: false })
    mocks.getFileInfo.mockResolvedValue({ success: false })
    await resolveFilePathCandidate('scripts/x.ts', { cwd: '/repo' })
    expect(mocks.searchStart).not.toHaveBeenCalled()
  })

  it.each([
    '/Users/qs',
    '/home/qs',
    '/root',
    '/var/root',
    '/private/var/root',
    'C:/Users/qs',
    '/',
    'C:/'
  ])('does not search home or filesystem root %s', async (cwd) => {
    mocks.getFileInfo.mockResolvedValue({ success: false })
    await resolveFilePathCandidate('scripts/x.ts', { cwd })
    expect(mocks.searchStart).not.toHaveBeenCalled()
  })

  it.each([
    '/repo/scripts/x.ts',
    '../scripts/x.ts',
    'apps/../scripts/x.ts'
  ])('does not reinterpret explicit absolute/parent paths as suffixes: %s', async (candidate) => {
    mocks.getFileInfo.mockResolvedValue({ success: false })
    await resolveFilePathCandidate(candidate, { cwd: '/repo/apps', projectRoot: '/repo' })
    expect(mocks.searchStart).not.toHaveBeenCalled()
  })

  it('fails closed on truncated results even with one matching hit', async () => {
    mockSearch(['/repo/apps/scripts/x.ts'], true)
    await expect(resolveFilePathCandidate('scripts/x.ts', { cwd: '/repo' })).resolves.toEqual({
      ok: false,
      reason: 'not-found'
    })
  })

  it('cancels oversized result sets and detaches stream listeners', async () => {
    mockSearch(Array.from({ length: 257 }, (_, index) => `/repo/app${index}/scripts/x.ts`))
    await expect(resolveFilePathCandidate('scripts/x.ts', { cwd: '/repo' })).resolves.toEqual({
      ok: false,
      reason: 'not-found'
    })
    expect(mocks.searchCancel).toHaveBeenCalledTimes(1)
    expect(mocks.onBatch.mock.results[0].value).toHaveBeenCalledTimes(1)
    expect(mocks.onDone.mock.results[0].value).toHaveBeenCalledTimes(1)
  })

  it('times out incomplete searches, cancels, and never logs file text', async () => {
    vi.useFakeTimers()
    try {
      mocks.getFileInfo.mockResolvedValue({ success: false })
      mocks.searchStart.mockResolvedValue({ success: true })
      mocks.logError.mockClear()
      const pending = resolveFilePathCandidate('scripts/private-file.ts', { cwd: '/repo' })
      await vi.advanceTimersByTimeAsync(2501)
      await expect(pending).resolves.toEqual({ ok: false, reason: 'not-found' })
      expect(mocks.searchCancel).toHaveBeenCalledTimes(1)
      expect(JSON.stringify(mocks.logError.mock.calls)).not.toContain('private-file')
    } finally {
      vi.useRealTimers()
    }
  })

  it('opens quoted space paths found by suffix and retains outside-quote cursor location', async () => {
    mockSearch(['/repo/apps/backend/scripts/my task.ts'])
    await expect(
      openFilePathFromTerminal('"scripts/my task.ts":12:3', { cwd: '/repo' })
    ).resolves.toEqual({ ok: true })
    expect(mocks.updateCursorPosition).toHaveBeenCalledWith(
      '/repo/apps/backend/scripts/my task.ts',
      12,
      3
    )
  })

  it('contains thrown stat failures and logs no raw backend text', async () => {
    mocks.getFileInfo.mockRejectedValue(new Error('private file contents'))
    mocks.logError.mockClear()
    await expect(resolveFilePathCandidate('scripts/x.ts', { cwd: '/repo' })).resolves.toEqual({
      ok: false,
      reason: 'not-found'
    })
    expect(JSON.stringify(mocks.logError.mock.calls)).not.toContain('private file contents')
  })

  it('resolves relative paths against terminal cwd', async () => {
    mocks.getFileInfo.mockResolvedValue({
      success: true,
      data: {
        path: '/repo/src/renderer/App.tsx',
        size: 100,
        modifiedAt: 1,
        type: 'file',
        isReadOnly: false,
        isBinary: false
      }
    })

    const result = await resolveFilePathCandidate('src/renderer/App.tsx', {
      cwd: '/repo',
      projectRoot: '/fallback'
    })

    expect(result).toEqual({
      ok: true,
      path: '/repo/src/renderer/App.tsx'
    })
    expect(mocks.getFileInfo).toHaveBeenCalledTimes(2)
    expect(mocks.getFileInfo).toHaveBeenNthCalledWith(1, '/repo/src/renderer/App.tsx')
    expect(mocks.getFileInfo).toHaveBeenNthCalledWith(2, '/fallback/src/renderer/App.tsx')
  })

  it('returns not-file when getFileInfo reports a directory', async () => {
    mocks.getFileInfo.mockResolvedValue({
      success: true,
      data: {
        path: '/repo/src',
        size: 100,
        modifiedAt: 1,
        type: 'directory',
        isReadOnly: false,
        isBinary: false
      }
    })

    const result = await resolveFilePathCandidate('src', {
      cwd: '/repo'
    })

    expect(result).toEqual({ ok: false, reason: 'not-file' })
  })

  it('returns missing-context when relative path has no cwd or project root', async () => {
    const result = await resolveFilePathCandidate('src/App.tsx', {})

    expect(result).toEqual({ ok: false, reason: 'missing-context' })
    expect(mocks.getFileInfo).not.toHaveBeenCalled()
  })

  it('falls back to project root for relative paths when cwd resolution fails', async () => {
    mocks.getFileInfo
      .mockResolvedValueOnce({ success: false, error: 'not found' })
      .mockResolvedValueOnce({
        success: true,
        data: {
          path: '/repo/src/App.tsx',
          size: 100,
          modifiedAt: 1,
          type: 'file',
          isReadOnly: false,
          isBinary: false
        }
      })

    const result = await resolveFilePathCandidate('src/App.tsx', {
      cwd: '/tmp/shell',
      projectRoot: '/repo'
    })

    expect(result).toEqual({ ok: true, path: '/repo/src/App.tsx' })
    expect(mocks.getFileInfo).toHaveBeenCalledTimes(2)
    expect(mocks.getFileInfo).toHaveBeenNthCalledWith(1, '/tmp/shell/src/App.tsx')
    expect(mocks.getFileInfo).toHaveBeenNthCalledWith(2, '/repo/src/App.tsx')
  })

  it('resolves paths with line and column suffixes using the plain file path', async () => {
    mocks.getFileInfo.mockResolvedValue({
      success: true,
      data: {
        path: '/repo/src/App.tsx',
        size: 100,
        modifiedAt: 1,
        type: 'file',
        isReadOnly: false,
        isBinary: false
      }
    })

    const result = await resolveFilePathCandidate('src/App.tsx:12:3', {
      cwd: '/repo',
      projectRoot: '/repo'
    })

    expect(result).toEqual({ ok: true, path: '/repo/src/App.tsx' })
    expect(mocks.getFileInfo).toHaveBeenCalledTimes(1)
    expect(mocks.getFileInfo).toHaveBeenCalledWith('/repo/src/App.tsx')
  })

  it('keeps first candidate precedence when parallel checks find multiple files', async () => {
    mocks.getFileInfo.mockResolvedValue({
      success: true,
      data: {
        path: '/repo/src/App.tsx',
        size: 100,
        modifiedAt: 1,
        type: 'file',
        isReadOnly: false,
        isBinary: false
      }
    })

    const result = await resolveFilePathCandidate('src/App.tsx', {
      cwd: '/tmp/shell',
      projectRoot: '/repo'
    })

    expect(result).toEqual({ ok: true, path: '/tmp/shell/src/App.tsx' })
    expect(mocks.getFileInfo).toHaveBeenCalledTimes(2)
    expect(mocks.getFileInfo).toHaveBeenNthCalledWith(1, '/tmp/shell/src/App.tsx')
    expect(mocks.getFileInfo).toHaveBeenNthCalledWith(2, '/repo/src/App.tsx')
  })

  it('rejects relative paths that escape the allowed roots', async () => {
    const result = await resolveFilePathCandidate('../../outside.ts', {
      cwd: '/repo/apps/se',
      projectRoot: '/repo/apps/se'
    })

    expect(result).toEqual({ ok: false, reason: 'not-found' })
    expect(mocks.getFileInfo).not.toHaveBeenCalled()
  })

  it('rejects absolute paths outside the allowed roots', async () => {
    const result = await resolveFilePathCandidate('/etc/hosts', {
      cwd: '/repo/apps/se',
      projectRoot: '/repo/apps/se'
    })

    expect(result).toEqual({ ok: false, reason: 'not-found' })
    expect(mocks.getFileInfo).not.toHaveBeenCalled()
  })

  it('resolves UNC paths within the allowed root', async () => {
    mocks.getFileInfo.mockResolvedValue({
      success: true,
      data: {
        path: '//server/share/workspace/src/App.tsx',
        size: 100,
        modifiedAt: 1,
        type: 'file',
        isReadOnly: false,
        isBinary: false
      }
    })

    const result = await resolveFilePathCandidate('\\\\server\\share\\workspace\\src\\App.tsx', {
      cwd: '\\\\server\\share\\workspace',
      projectRoot: '\\\\server\\share\\workspace'
    })

    expect(result).toEqual({ ok: true, path: '//server/share/workspace/src/App.tsx' })
    expect(mocks.getFileInfo).toHaveBeenCalledWith('//server/share/workspace/src/App.tsx')
  })

  it('rejects UNC paths outside the allowed root', async () => {
    const result = await resolveFilePathCandidate('\\\\server\\other-share\\secret.txt', {
      cwd: '\\\\server\\share\\workspace',
      projectRoot: '\\\\server\\share\\workspace'
    })

    expect(result).toEqual({ ok: false, reason: 'not-found' })
    expect(mocks.getFileInfo).not.toHaveBeenCalled()
  })

  it('opens the resolved file and adds an editor tab', async () => {
    mocks.getFileInfo.mockResolvedValue({
      success: true,
      data: {
        path: '/repo/src/renderer/App.tsx',
        size: 100,
        modifiedAt: 1,
        type: 'file',
        isReadOnly: false,
        isBinary: false
      }
    })

    const opened = await openFilePathFromTerminal('src/renderer/App.tsx', {
      cwd: '/repo'
    })

    expect(opened).toEqual({ ok: true })
    expect(mocks.openFile).toHaveBeenCalledWith('/repo/src/renderer/App.tsx')
    expect(mocks.updateCursorPosition).not.toHaveBeenCalled()
    expect(mocks.addEditorTab).toHaveBeenCalledWith('/repo/src/renderer/App.tsx')
    expect(mocks.toastError).not.toHaveBeenCalled()
  })

  it('returns not-found details and does not open missing files', async () => {
    mocks.getFileInfo.mockResolvedValue({ success: false, error: 'not found' })

    const opened = await openFilePathFromTerminal('missing.ts', {
      projectRoot: '/repo'
    })

    expect(opened).toEqual({
      ok: false,
      reason: 'not-found',
      message: 'File not found: missing.ts'
    })
    expect(mocks.getFileInfo).toHaveBeenCalledTimes(1)
    expect(mocks.getFileInfo).toHaveBeenCalledWith('/repo/missing.ts')
    expect(mocks.openFile).not.toHaveBeenCalled()
    expect(mocks.updateCursorPosition).not.toHaveBeenCalled()
    expect(mocks.addEditorTab).not.toHaveBeenCalled()
    expect(mocks.toastError).not.toHaveBeenCalled()
  })

  it('returns a not-file message and does not open when resolved target is not a file', async () => {
    mocks.getFileInfo.mockResolvedValue({
      success: true,
      data: {
        path: '/repo/src',
        size: 0,
        modifiedAt: 1,
        type: 'directory',
        isReadOnly: false,
        isBinary: false
      }
    })

    const opened = await openFilePathFromTerminal('src', { cwd: '/repo' })

    expect(opened).toEqual({
      ok: false,
      reason: 'not-file',
      message: 'Path is a directory, not a file: src'
    })
    expect(mocks.openFile).not.toHaveBeenCalled()
    expect(mocks.updateCursorPosition).not.toHaveBeenCalled()
    expect(mocks.addEditorTab).not.toHaveBeenCalled()
    expect(mocks.toastError).not.toHaveBeenCalled()
  })

  it('returns a missing-context message and does not open when context is missing', async () => {
    const opened = await openFilePathFromTerminal('src/App.tsx', {})

    expect(opened).toEqual({
      ok: false,
      reason: 'missing-context',
      message: 'No project or working directory found; set a project/cwd to open paths: src/App.tsx'
    })
    expect(mocks.getFileInfo).not.toHaveBeenCalled()
    expect(mocks.openFile).not.toHaveBeenCalled()
    expect(mocks.updateCursorPosition).not.toHaveBeenCalled()
    expect(mocks.addEditorTab).not.toHaveBeenCalled()
    expect(mocks.toastError).not.toHaveBeenCalled()
  })

  it('updates cursor position from line and column suffixes after opening', async () => {
    mocks.getFileInfo.mockResolvedValue({
      success: true,
      data: {
        path: '/repo/src/App.tsx',
        size: 100,
        modifiedAt: 1,
        type: 'file',
        isReadOnly: false,
        isBinary: false
      }
    })

    const opened = await openFilePathFromTerminal('src/App.tsx:12:3', {
      cwd: '/repo'
    })

    expect(opened).toEqual({ ok: true })
    expect(mocks.openFile).toHaveBeenCalledWith('/repo/src/App.tsx')
    expect(mocks.updateCursorPosition).toHaveBeenCalledWith('/repo/src/App.tsx', 12, 3)
    expect(mocks.addEditorTab).toHaveBeenCalledWith('/repo/src/App.tsx')
  })

  it('defaults column to 1 when only line suffix is provided', async () => {
    mocks.getFileInfo.mockResolvedValue({
      success: true,
      data: {
        path: '/repo/src/App.tsx',
        size: 100,
        modifiedAt: 1,
        type: 'file',
        isReadOnly: false,
        isBinary: false
      }
    })

    const opened = await openFilePathFromTerminal('src/App.tsx:9', {
      cwd: '/repo'
    })

    expect(opened).toEqual({ ok: true })
    expect(mocks.updateCursorPosition).toHaveBeenCalledWith('/repo/src/App.tsx', 9, 1)
  })

  it('returns open-failed when editor open throws', async () => {
    mocks.getFileInfo.mockResolvedValue({
      success: true,
      data: {
        path: '/repo/src/App.tsx',
        size: 100,
        modifiedAt: 1,
        type: 'file',
        isReadOnly: false,
        isBinary: false
      }
    })
    mocks.openFile.mockRejectedValue(new Error('boom'))

    const opened = await openFilePathFromTerminal('src/App.tsx:2:5', {
      cwd: '/repo'
    })

    expect(opened).toEqual({
      ok: false,
      reason: 'open-failed',
      message: 'Failed to open file: src/App.tsx (boom)'
    })
    expect(mocks.updateCursorPosition).not.toHaveBeenCalled()
    expect(mocks.addEditorTab).not.toHaveBeenCalled()
  })
})
