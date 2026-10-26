import { act, cleanup, renderHook, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { type EditorFileState, useEditorStore } from '@/stores/editor-store'
import {
  isQuickEditorFile,
  loadQuickEditorFile,
  resetQuickEditorDrafts,
  useQuickEditorDraftPersistence
} from './quick-terminal-editor-drafts'
import { resetQuickWorkspaceStore, useQuickWorkspaceStore } from './quick-terminal-workspace-store'

const persistence = vi.hoisted(() => ({
  read: vi.fn(),
  writeDebounced: vi.fn(),
  flushPendingWrites: vi.fn()
}))
const files = vi.hoisted(() => ({ readFile: vi.fn() }))
const log = vi.hoisted(() => vi.fn(async () => ({ success: true })))
const flush = vi.hoisted(() => vi.fn(async (_path: string): Promise<void> => {}))
vi.mock('@/lib/persistence-api', () => ({ persistenceApi: persistence }))
vi.mock('@/lib/api', () => ({ filesystemApi: files }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: log }))
vi.mock('@/lib/editor-content-flush', () => ({ flushEditorContent: flush }))

function file(path: string, content = 'disk', dirty = false): EditorFileState {
  return {
    filePath: path,
    content,
    originalContent: 'disk',
    isDirty: dirty,
    language: 'typescript',
    cursorPosition: { line: 3, col: 2 },
    scrollTop: 10,
    viewMode: 'code',
    lastModified: 1,
    operationStatus: 'idle'
  }
}
function layout(id: string, paths: string[]): void {
  const tabs = paths.map((path) => ({
    id: `file:${path}`,
    type: 'editor' as const,
    filePath: path
  }))
  useQuickWorkspaceStore.setState((state) => ({
    layouts: {
      ...state.layouts,
      [id]: { tabs, activeTabId: tabs[0]?.id ?? null, lastTerminalTabId: null }
    }
  }))
}
function saved(path: string, content = 'saved draft'): unknown {
  return {
    version: 1,
    files: [
      {
        filePath: path,
        content,
        cursorPosition: { line: 2, col: 4 },
        scrollTop: 23,
        viewMode: 'code'
      }
    ]
  }
}
const tick = async (): Promise<void> => {
  await act(async () => {
    await Promise.resolve()
    await Promise.resolve()
  })
}

beforeEach(() => {
  resetQuickEditorDrafts()
  resetQuickWorkspaceStore()
  useEditorStore.setState({ openFiles: new Map(), activeFilePath: '/project/active.ts' })
  vi.clearAllMocks()
  persistence.read.mockResolvedValue({ success: true, data: null })
  persistence.writeDebounced.mockResolvedValue({ success: true })
  persistence.flushPendingWrites.mockResolvedValue({ success: true })
  files.readFile.mockResolvedValue({ success: true, data: { content: 'disk', modifiedAt: 1 } })
  flush.mockResolvedValue(undefined)
})
afterEach(() => {
  cleanup()
  resetQuickEditorDrafts()
})

describe('quick editor drafts', () => {
  it('restores persisted text and view state without global activation', async () => {
    layout('one', ['/quick/a.ts'])
    persistence.read.mockResolvedValue({ success: true, data: saved('/quick/a.ts') })
    await loadQuickEditorFile('one', '/quick/a.ts')
    expect(useEditorStore.getState().activeFilePath).toBe('/project/active.ts')
    expect(useEditorStore.getState().openFiles.get('/quick/a.ts')).toMatchObject({
      content: 'saved draft',
      isDirty: true,
      cursorPosition: { line: 2, col: 4 },
      scrollTop: 23
    })
    expect(persistence.read).toHaveBeenCalledWith('quick-terminal/editor-drafts/one')
    expect(isQuickEditorFile('/quick/a.ts')).toBe(true)
    expect(isQuickEditorFile('/quick/not-owned.ts')).toBe(false)
  })
  it('does not replace an existing dirty buffer with stale saved text', async () => {
    useEditorStore.setState({ openFiles: new Map([['/a.ts', file('/a.ts', 'new memory', true)]]) })
    persistence.read.mockResolvedValue({ success: true, data: saved('/a.ts') })
    await loadQuickEditorFile('one', '/a.ts')
    expect(useEditorStore.getState().openFiles.get('/a.ts')?.content).toBe('new memory')
    expect(persistence.writeDebounced).toHaveBeenLastCalledWith(
      'quick-terminal/editor-drafts/one',
      expect.objectContaining({
        files: [expect.objectContaining({ content: 'new memory' })]
      })
    )
  })
  it('does not resurrect an unread stale draft after undo returns a dirty buffer to clean', async () => {
    useEditorStore.setState({ openFiles: new Map([['/a.ts', file('/a.ts', 'unsaved', true)]]) })
    let resolve!: (value: unknown) => void
    persistence.read.mockReturnValue(
      new Promise((done) => {
        resolve = done
      })
    )
    const loading = loadQuickEditorFile('one', '/a.ts')
    useEditorStore.setState({ openFiles: new Map([['/a.ts', file('/a.ts')]]) })
    resolve({ success: true, data: saved('/a.ts') })
    await loading
    expect(useEditorStore.getState().openFiles.get('/a.ts')).toMatchObject({
      content: 'disk',
      isDirty: false
    })
    expect(persistence.writeDebounced).toHaveBeenLastCalledWith(
      'quick-terminal/editor-drafts/one',
      { version: 1, files: [] }
    )
  })
  it('keeps a newer edit made while draft persistence is loading', async () => {
    let resolve!: (value: unknown) => void
    persistence.read.mockReturnValue(
      new Promise((done) => {
        resolve = done
      })
    )
    const loading = loadQuickEditorFile('one', '/a.ts')
    useEditorStore.setState({
      openFiles: new Map([['/a.ts', file('/a.ts', 'typed while loading', true)]])
    })
    resolve({ success: true, data: saved('/a.ts') })
    await loading
    expect(useEditorStore.getState().openFiles.get('/a.ts')?.content).toBe('typed while loading')
  })
  it('saves independent entry drafts and retains dirty ownership after navigation/deletion', async () => {
    layout('one', ['/a.ts'])
    layout('two', ['/b.ts'])
    const first = renderHook(() => useQuickEditorDraftPersistence('one'))
    const second = renderHook(() => useQuickEditorDraftPersistence('two'))
    await tick()
    act(() =>
      useEditorStore.setState({
        openFiles: new Map([
          ['/a.ts', file('/a.ts', 'entry one', true)],
          ['/b.ts', file('/b.ts', 'entry two', true)]
        ])
      })
    )
    expect(persistence.writeDebounced).toHaveBeenCalledWith(
      'quick-terminal/editor-drafts/one',
      expect.objectContaining({ files: [expect.objectContaining({ content: 'entry one' })] })
    )
    expect(persistence.writeDebounced).toHaveBeenCalledWith(
      'quick-terminal/editor-drafts/two',
      expect.objectContaining({ files: [expect.objectContaining({ content: 'entry two' })] })
    )
    first.unmount()
    second.unmount()
    await tick()
    useQuickWorkspaceStore.setState({ layouts: {} })
    useEditorStore.setState({ openFiles: new Map() }) // project navigation must not erase retained drafts
    expect(isQuickEditorFile('/a.ts')).toBe(true)
    expect(isQuickEditorFile('/b.ts')).toBe(true)
    await loadQuickEditorFile('one', '/a.ts')
    expect(useEditorStore.getState().openFiles.get('/a.ts')?.content).toBe('entry one')
    expect(persistence.flushPendingWrites).toHaveBeenCalled()
  })
  it('persists an asynchronous editor flush on navigation, even after unmount', async () => {
    layout('one', ['/a.ts'])
    useEditorStore.setState({ openFiles: new Map([['/a.ts', file('/a.ts')]]) })
    let resolve!: () => void
    flush.mockImplementation(async () => {
      await new Promise<void>((done) => {
        resolve = done
      })
      useEditorStore.getState().updateContent('/a.ts', 'final debounced text')
    })
    const hook = renderHook(() => useQuickEditorDraftPersistence('one'))
    await tick()
    hook.unmount()
    expect(flush).toHaveBeenCalledWith('/a.ts')
    resolve()
    await tick()
    expect(persistence.writeDebounced).toHaveBeenLastCalledWith(
      'quick-terminal/editor-drafts/one',
      expect.objectContaining({
        files: [expect.objectContaining({ content: 'final debounced text' })]
      })
    )
    expect(persistence.flushPendingWrites).toHaveBeenCalled()
  })
  it('clears saved/discarded clean drafts without dropping dirty detached buffers', async () => {
    layout('one', ['/a.ts'])
    const hook = renderHook(() => useQuickEditorDraftPersistence('one'))
    await tick()
    act(() =>
      useEditorStore.setState({ openFiles: new Map([['/a.ts', file('/a.ts', 'dirty', true)]]) })
    )
    useQuickWorkspaceStore.setState({ layouts: {} })
    expect(isQuickEditorFile('/a.ts')).toBe(true)
    act(() => useEditorStore.setState({ openFiles: new Map([['/a.ts', file('/a.ts')]]) }))
    expect(persistence.writeDebounced).toHaveBeenLastCalledWith(
      'quick-terminal/editor-drafts/one',
      { version: 1, files: [] }
    )
    // The clean buffer is still quick-owned until released; dropping its draft
    // must not make it eligible for unrelated project persistence.
    expect(isQuickEditorFile('/a.ts')).toBe(true)
    hook.unmount()
  })
  it('restores a dirty entry from its persisted payload after a simulated reload', async () => {
    layout('one', ['/a.ts'])
    const hook = renderHook(() => useQuickEditorDraftPersistence('one'))
    await tick()
    act(() =>
      useEditorStore.setState({
        openFiles: new Map([['/a.ts', file('/a.ts', 'survives reload', true)]])
      })
    )
    hook.unmount()
    await tick()
    const calls = persistence.writeDebounced.mock.calls
    const payload = calls[calls.length - 1]?.[1]
    expect(payload).toMatchObject({
      files: [expect.objectContaining({ content: 'survives reload' })]
    })
    resetQuickEditorDrafts()
    useEditorStore.setState({ openFiles: new Map(), activeFilePath: '/new-project.ts' })
    persistence.read.mockResolvedValue({ success: true, data: payload })
    await loadQuickEditorFile('one', '/a.ts')
    expect(useEditorStore.getState().openFiles.get('/a.ts')?.content).toBe('survives reload')
    expect(useEditorStore.getState().activeFilePath).toBe('/new-project.ts')
  })
  it('does not steal activation when delayed disk reading completes after navigation', async () => {
    let resolve!: (value: unknown) => void
    files.readFile.mockReturnValue(
      new Promise((done) => {
        resolve = done
      })
    )
    const loading = loadQuickEditorFile('one', '/a.ts')
    await tick()
    useEditorStore.setState({ activeFilePath: '/another-owner.ts' })
    resolve({ success: true, data: { content: 'disk', modifiedAt: 1 } })
    await loading
    expect(useEditorStore.getState().activeFilePath).toBe('/another-owner.ts')
  })
  it('does not restore a stale draft over a newer buffer edited and saved during disk read', async () => {
    persistence.read.mockResolvedValue({ success: true, data: saved('/a.ts') })
    let resolve!: (value: unknown) => void
    files.readFile.mockReturnValue(
      new Promise((done) => {
        resolve = done
      })
    )
    const loading = loadQuickEditorFile('one', '/a.ts')
    await tick()
    useEditorStore.setState({ openFiles: new Map([['/a.ts', file('/a.ts', 'new', true)]]) })
    useEditorStore.setState({
      openFiles: new Map([['/a.ts', { ...file('/a.ts', 'new'), originalContent: 'new' }]])
    })
    resolve({ success: true, data: { content: 'disk', modifiedAt: 1 } })
    await loading
    expect(useEditorStore.getState().openFiles.get('/a.ts')?.content).toBe('new')
  })
  it('keeps missing-file drafts recoverable and rejects until disk can be loaded', async () => {
    persistence.read.mockResolvedValue({ success: true, data: saved('/missing.ts') })
    files.readFile.mockResolvedValue({
      success: false,
      error: 'File not found',
      code: 'FILE_NOT_FOUND'
    })
    await expect(loadQuickEditorFile('one', '/missing.ts')).rejects.toThrow()
    expect(isQuickEditorFile('/missing.ts')).toBe(true)
    expect(persistence.writeDebounced).not.toHaveBeenCalled()
    files.readFile.mockResolvedValue({ success: true, data: { content: 'disk', modifiedAt: 1 } })
    await loadQuickEditorFile('one', '/missing.ts')
    expect(useEditorStore.getState().openFiles.get('/missing.ts')?.content).toBe('saved draft')
  })
  it('does not queue destructive cleanup when persistence read fails and allows retry', async () => {
    persistence.read.mockResolvedValueOnce({
      success: false,
      code: 'IO_ERROR',
      error: 'secret path/content'
    })
    await expect(loadQuickEditorFile('one', '/a.ts')).rejects.toThrow(
      'Quick editor drafts unavailable'
    )
    expect(persistence.writeDebounced).not.toHaveBeenCalled()
    persistence.read.mockResolvedValueOnce({ success: true, data: saved('/a.ts') })
    await loadQuickEditorFile('one', '/a.ts')
    expect(useEditorStore.getState().openFiles.get('/a.ts')?.isDirty).toBe(true)
    expect(JSON.stringify(log.mock.calls)).not.toContain('secret')
  })
  it('logs failed writes and flushes without exposing file contents', async () => {
    layout('one', ['/private.ts'])
    persistence.writeDebounced.mockResolvedValue({ success: false, error: 'secret content' })
    persistence.flushPendingWrites.mockRejectedValue(new Error('secret content'))
    const hook = renderHook(() => useQuickEditorDraftPersistence('one'))
    await tick()
    act(() =>
      useEditorStore.setState({
        openFiles: new Map([['/private.ts', file('/private.ts', 'secret content', true)]])
      })
    )
    await tick()
    hook.unmount()
    await waitFor(() =>
      expect(log).toHaveBeenCalledWith(
        expect.objectContaining({ message: expect.stringContaining('operation=flush') })
      )
    )
    expect(log).toHaveBeenCalledWith(
      expect.objectContaining({ message: expect.stringContaining('operation=write') })
    )
    expect(JSON.stringify(log.mock.calls)).not.toContain('secret')
    expect(JSON.stringify(log.mock.calls)).not.toContain('/private.ts')
  })
})
