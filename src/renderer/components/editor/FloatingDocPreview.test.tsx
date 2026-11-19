import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { type EditorFileState, useEditorStore } from '@/stores/editor-store'
import { resetFloatingDocStoreForTests, useFloatingDocStore } from '@/stores/floating-doc-store'
import { getAllLeafPanes, useWorkspaceStore } from '@/stores/workspace-store'
import { FloatingDocPreview } from './FloatingDocPreview'

vi.mock('@/lib/tauri-runtime', () => ({
  isTauriContext: () => false
}))

vi.mock('./CodeEditor', () => ({
  CodeEditor: ({ content }: { content: string }) => <pre data-testid="code-editor">{content}</pre>
}))

vi.mock('./MarkdownEditor', () => ({
  MarkdownEditor: ({ content }: { content: string }) => (
    <div data-testid="markdown-editor">{content}</div>
  )
}))

vi.mock('./FilePreview', () => ({
  FilePreview: ({ kind }: { kind: string }) => <div data-testid={`file-preview-${kind}`} />
}))

vi.mock('./HtmlPreview', () => ({
  HtmlPreview: ({ srcDoc, name }: { srcDoc?: string; name: string }) => (
    <div data-testid="html-preview" data-srcdoc={srcDoc ?? ''} title={name} />
  )
}))

const originalSaveFile = useEditorStore.getState().saveFile

function fileState(filePath: string, overrides: Partial<EditorFileState> = {}): EditorFileState {
  return {
    filePath,
    content: '',
    originalContent: '',
    isDirty: false,
    language: '',
    lastModified: 1,
    viewMode: 'code',
    cursorPosition: { line: 1, col: 1 },
    scrollTop: 0,
    operationStatus: 'idle',
    ...overrides
  }
}

function seed(path: string, overrides: Partial<EditorFileState> = {}): void {
  useEditorStore.setState((state) => {
    const openFiles = new Map(state.openFiles)
    openFiles.set(path, fileState(path, overrides))
    return { openFiles, activeFilePath: path }
  })
}

function openInPreview(path: string, overrides: Partial<EditorFileState> = {}): void {
  seed(path, overrides)
  const state = useFloatingDocStore.getState()
  useFloatingDocStore.setState({
    docPaths: [...state.docPaths, path],
    activePath: path,
    mode: 'floating'
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
  useEditorStore.setState({ saveFile: vi.fn(async () => true) })
})

afterEach(() => {
  cleanup()
  useEditorStore.setState({ saveFile: originalSaveFile })
  useFloatingDocStore.setState({ mode: 'floating' })
})

describe('FloatingDocPreview', () => {
  it('renders nothing when no document is open', () => {
    const { container } = render(<FloatingDocPreview />)
    expect(container.firstChild).toBeNull()
  })

  it('collapses to a ball and expands on activation', () => {
    openInPreview('/repo/readme.md', { language: 'markdown' })
    useFloatingDocStore.setState({ mode: 'collapsed', ballPosition: { x: 20, y: 30 } })

    render(<FloatingDocPreview />)
    const ball = screen.getByTestId('floating-doc-ball')
    expect(ball).toBeInTheDocument()
    expect(ball).toHaveTextContent('1')

    fireEvent.click(ball)
    expect(useFloatingDocStore.getState().mode).toBe('floating')
  })

  it('videos the document tabs and switches the active one', () => {
    openInPreview('/repo/a.md', { language: 'markdown' })
    openInPreview('/repo/b.md', { language: 'markdown' })
    useFloatingDocStore.setState({ activePath: '/repo/b.md' })

    render(<FloatingDocPreview />)

    const tabs = screen.getAllByRole('tab')
    expect(tabs.map((tab) => tab.textContent)).toEqual([
      expect.stringContaining('a.md'),
      expect.stringContaining('b.md')
    ])

    fireEvent.click(tabs[0])
    expect(useFloatingDocStore.getState().activePath).toBe('/repo/a.md')
  })

  it('renders unsaved HTML from the buffer in the browser edition', () => {
    openInPreview('/repo/page.html', {
      content: '<h1>draft</h1>',
      isDirty: true
    })

    render(<FloatingDocPreview />)

    const preview = screen.getByTestId('html-preview')
    expect(preview).toHaveAttribute('data-srcdoc', '<h1>draft</h1>')
  })

  it('marks a dirty tab and asks before discarding it', () => {
    openInPreview('/repo/notes.md', { content: '# draft', isDirty: true })

    render(<FloatingDocPreview />)
    expect(screen.getByLabelText('Unsaved')).toBeInTheDocument()

    fireEvent.click(screen.getByLabelText('Close'))
    expect(screen.getByTestId('floating-doc-confirm')).toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Discard and close' }))

    expect(useFloatingDocStore.getState().docPaths).toEqual([])
    expect(useEditorStore.getState().openFiles.has('/repo/notes.md')).toBe(false)
    expect(useEditorStore.getState().saveFile).not.toHaveBeenCalled()
  })

  it('keeps the dirty document open when save-and-close fails', async () => {
    openInPreview('/repo/notes.md', { content: '# draft', isDirty: true })
    vi.mocked(useEditorStore.getState().saveFile).mockResolvedValueOnce(false)

    render(<FloatingDocPreview />)
    fireEvent.click(screen.getByLabelText('Close'))
    fireEvent.click(screen.getByRole('button', { name: 'Save and close' }))

    await waitFor(() => {
      expect(useEditorStore.getState().saveFile).toHaveBeenCalledWith('/repo/notes.md')
    })
    expect(useFloatingDocStore.getState().docPaths).toEqual(['/repo/notes.md'])
  })

  it('closes both dirty and clean documents on close-all confirm', async () => {
    openInPreview('/repo/dirty.md', { content: '# draft', isDirty: true })
    openInPreview('/repo/clean.md', { content: '# clean', isDirty: false })

    render(<FloatingDocPreview />)
    fireEvent.click(screen.getByLabelText('Close all'))
    expect(screen.getByTestId('floating-doc-confirm')).toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Discard and close' }))

    await waitFor(() => {
      expect(useFloatingDocStore.getState().docPaths).toEqual([])
    })
    expect(useEditorStore.getState().openFiles.has('/repo/dirty.md')).toBe(false)
    expect(useEditorStore.getState().openFiles.has('/repo/clean.md')).toBe(false)
  })

  it('saves a dirty document before closing it', async () => {
    openInPreview('/repo/notes.md', { content: '# draft', isDirty: true })

    render(<FloatingDocPreview />)
    fireEvent.click(screen.getByLabelText('Close'))
    fireEvent.click(screen.getByRole('button', { name: 'Save and close' }))

    expect(useEditorStore.getState().saveFile).toHaveBeenCalledWith('/repo/notes.md')
    await waitFor(() => {
      expect(useFloatingDocStore.getState().docPaths).toEqual([])
    })
  })

  it('leaves fullscreen with Escape', () => {
    openInPreview('/repo/notes.md', { language: 'markdown' })
    useFloatingDocStore.setState({ mode: 'fullscreen' })

    render(<FloatingDocPreview />)
    expect(screen.getByLabelText('Exit fullscreen')).toBeInTheDocument()

    fireEvent.keyDown(window, { key: 'Escape' })
    expect(useFloatingDocStore.getState().mode).toBe('floating')
  })

  it('moves the active document into a workspace tab', () => {
    openInPreview('/repo/notes.md', { language: 'markdown' })

    render(<FloatingDocPreview />)
    fireEvent.click(screen.getByLabelText('Open in workspace'))

    expect(editorTabPaths()).toContain('/repo/notes.md')
    expect(useFloatingDocStore.getState().docPaths).toEqual([])
  })

  it('collapses from the window back to the ball', () => {
    openInPreview('/repo/notes.md', { language: 'markdown' })

    render(<FloatingDocPreview />)
    fireEvent.click(screen.getByLabelText('Collapse to floating ball'))

    expect(useFloatingDocStore.getState().mode).toBe('collapsed')
    expect(screen.getByTestId('floating-doc-ball')).toBeInTheDocument()
  })
})
