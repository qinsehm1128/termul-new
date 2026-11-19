import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { type EditorFileState, useEditorStore } from '@/stores/editor-store'
import {
  clampBallPosition,
  FLOATING_BALL_MARGIN,
  resetFloatingDocStoreForTests,
  useFloatingDocStore
} from '@/stores/floating-doc-store'
import { getAllLeafPanes, useWorkspaceStore } from '@/stores/workspace-store'

function fileState(filePath: string, overrides: Partial<EditorFileState> = {}): EditorFileState {
  return {
    filePath,
    content: '',
    originalContent: '',
    isDirty: false,
    language: '',
    lastModified: 1,
    viewMode: 'preview',
    cursorPosition: { line: 1, col: 1 },
    scrollTop: 0,
    operationStatus: 'idle',
    ...overrides
  }
}

const originalOpenFile = useEditorStore.getState().openFile
const originalSaveFile = useEditorStore.getState().saveFile

function seedOpen(path: string, overrides: Partial<EditorFileState> = {}): void {
  useEditorStore.setState((state) => {
    const openFiles = new Map(state.openFiles)
    openFiles.set(path, fileState(path, overrides))
    return { openFiles, activeFilePath: path }
  })
}

function editorTabPaths(): string[] {
  return getAllLeafPanes(useWorkspaceStore.getState().root).flatMap((leaf) =>
    leaf.tabs.flatMap((tab) => (tab.type === 'editor' ? [tab.filePath] : []))
  )
}

beforeEach(() => {
  resetFloatingDocStoreForTests()
  useEditorStore.setState({ openFiles: new Map(), activeFilePath: null })
  useEditorStore.setState({
    openFile: async (path: string) => {
      seedOpen(path)
    }
  })
})

afterEach(() => {
  useEditorStore.setState({
    openFile: originalOpenFile,
    saveFile: originalSaveFile,
    openFiles: new Map(),
    activeFilePath: null
  })
})

describe('floating document store', () => {
  it('opens a document in the floating window', async () => {
    await useFloatingDocStore.getState().openDocument('/repo/readme.md')

    const state = useFloatingDocStore.getState()
    expect(state.docPaths).toEqual(['/repo/readme.md'])
    expect(state.activePath).toBe('/repo/readme.md')
    expect(state.mode).toBe('floating')
  })

  it('honours a fullscreen request and keeps it for later documents', async () => {
    await useFloatingDocStore.getState().openDocument('/repo/a.md', { mode: 'fullscreen' })
    expect(useFloatingDocStore.getState().mode).toBe('fullscreen')

    await useFloatingDocStore.getState().openDocument('/repo/b.md')
    const state = useFloatingDocStore.getState()
    expect(state.mode).toBe('fullscreen')
    expect(state.activePath).toBe('/repo/b.md')
    expect(state.docPaths).toEqual(['/repo/a.md', '/repo/b.md'])
  })

  it('rejects a document the editor store refused to open', async () => {
    useEditorStore.setState({ openFile: async () => undefined })
    await expect(useFloatingDocStore.getState().openDocument('/repo/missing.md')).rejects.toThrow()
    expect(useFloatingDocStore.getState().docPaths).toEqual([])
  })

  it('switches the active document only for open documents', async () => {
    await useFloatingDocStore.getState().openDocument('/repo/a.md')
    await useFloatingDocStore.getState().openDocument('/repo/b.md')

    useFloatingDocStore.getState().setActiveDoc('/repo/a.md')
    expect(useFloatingDocStore.getState().activePath).toBe('/repo/a.md')

    useFloatingDocStore.getState().setActiveDoc('/repo/unknown.md')
    expect(useFloatingDocStore.getState().activePath).toBe('/repo/a.md')
  })

  it('closes a document and lets go of its idle buffer', async () => {
    await useFloatingDocStore.getState().openDocument('/repo/a.md')
    await useFloatingDocStore.getState().openDocument('/repo/b.md')

    useFloatingDocStore.getState().closeDocument('/repo/b.md')

    const state = useFloatingDocStore.getState()
    expect(state.docPaths).toEqual(['/repo/a.md'])
    expect(state.activePath).toBe('/repo/a.md')
    expect(useEditorStore.getState().openFiles.has('/repo/b.md')).toBe(false)
  })

  it('keeps a dirty buffer after closing the preview', async () => {
    await useFloatingDocStore.getState().openDocument('/repo/draft.md')
    seedOpen('/repo/draft.md', { isDirty: true })

    useFloatingDocStore.getState().closeDocument('/repo/draft.md')

    expect(useFloatingDocStore.getState().docPaths).toEqual([])
    expect(useFloatingDocStore.getState().activePath).toBeNull()
    expect(useEditorStore.getState().openFiles.has('/repo/draft.md')).toBe(true)
  })

  it('closes every document', async () => {
    await useFloatingDocStore.getState().openDocument('/repo/a.md')
    await useFloatingDocStore.getState().openDocument('/repo/b.md')

    useFloatingDocStore.getState().closeAllDocuments()

    expect(useFloatingDocStore.getState().docPaths).toEqual([])
    expect(useEditorStore.getState().openFiles.size).toBe(0)
  })

  it('toggles between floating, collapsed and fullscreen', async () => {
    await useFloatingDocStore.getState().openDocument('/repo/a.md')

    useFloatingDocStore.getState().toggleCollapse()
    expect(useFloatingDocStore.getState().mode).toBe('collapsed')
    useFloatingDocStore.getState().toggleCollapse()
    expect(useFloatingDocStore.getState().mode).toBe('floating')

    useFloatingDocStore.getState().toggleFullscreen()
    expect(useFloatingDocStore.getState().mode).toBe('fullscreen')
    useFloatingDocStore.getState().toggleFullscreen()
    expect(useFloatingDocStore.getState().mode).toBe('floating')
  })

  it('ignores mode changes with no documents', () => {
    useFloatingDocStore.getState().toggleCollapse()
    useFloatingDocStore.getState().toggleFullscreen()
    useFloatingDocStore.getState().setMode('fullscreen')
    expect(useFloatingDocStore.getState().mode).toBe('floating')
  })

  it('clamps and persists the ball position', () => {
    useFloatingDocStore.getState().setBallPosition({ x: -50, y: 999999 })

    const position = useFloatingDocStore.getState().ballPosition
    expect(position?.x).toBe(FLOATING_BALL_MARGIN)
    expect(position?.y).toBeLessThan(999999)

    const stored = window.localStorage.getItem('floating-doc-preview/ball-position')
    expect(stored).not.toBeNull()
    expect(JSON.parse(String(stored))).toEqual(position)
  })

  it('clamps to the viewport in both directions', () => {
    const clamped = clampBallPosition({ x: 5000, y: -20 }, { width: 400, height: 300 })
    expect(clamped.x).toBe(400 - 44 - FLOATING_BALL_MARGIN)
    expect(clamped.y).toBe(FLOATING_BALL_MARGIN)
  })

  it('moves a document into a workspace tab', async () => {
    await useFloatingDocStore.getState().openDocument('/repo/a.md')
    await useFloatingDocStore.getState().openDocument('/repo/b.md')

    useFloatingDocStore.getState().moveToWorkspaceTab('/repo/a.md')

    expect(editorTabPaths()).toContain('/repo/a.md')
    const state = useFloatingDocStore.getState()
    expect(state.docPaths).toEqual(['/repo/b.md'])
    expect(state.activePath).toBe('/repo/b.md')
  })

  it('does not touch the editor store for an unknown close', () => {
    const openFile = vi.fn()
    useEditorStore.setState({ openFile })
    useFloatingDocStore.getState().closeDocument('/repo/unknown.md')
    expect(openFile).not.toHaveBeenCalled()
  })
})
