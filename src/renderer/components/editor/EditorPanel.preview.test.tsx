import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { type EditorFileState, useEditorStore } from '@/stores/editor-store'
import { EditorPanel } from './EditorPanel'

vi.mock('@/lib/tauri-runtime', () => ({
  isTauriContext: () => true
}))

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: (path: string) => `asset://localhost${path}`
}))

vi.mock('@/hooks/use-toc-settings', () => ({
  useTocSettings: vi.fn()
}))

vi.mock('./CodeEditor', () => ({
  CodeEditor: ({ content }: { content: string }) => <pre data-testid="code-editor">{content}</pre>
}))

vi.mock('./MarkdownEditor', () => ({
  MarkdownEditor: () => <div data-testid="markdown-editor" />
}))

function seed(filePath: string, overrides: Partial<EditorFileState>): void {
  const file: EditorFileState = {
    filePath,
    content: '',
    originalContent: '',
    isDirty: false,
    language: '',
    lastModified: 5,
    viewMode: 'preview',
    cursorPosition: { line: 1, col: 1 },
    scrollTop: 0,
    operationStatus: 'idle',
    ...overrides
  }
  useEditorStore.setState({ openFiles: new Map([[filePath, file]]), activeFilePath: filePath })
}

describe('EditorPanel previews', () => {
  afterEach(() => {
    cleanup()
    useEditorStore.setState({ openFiles: new Map(), activeFilePath: null })
  })

  it('shows an image from disk, versioned by its modification time', () => {
    seed('/p/logo.png', {})
    render(<EditorPanel filePath="/p/logo.png" isVisible />)

    expect(document.querySelector('img')).toHaveAttribute('src', 'asset://localhost/p/logo.png?v=5')
    expect(screen.queryByTestId('code-editor')).not.toBeInTheDocument()
  })

  it('renders svg and switches between preview and source', () => {
    seed('/p/icon.svg', { content: '<svg/>', language: 'xml' })
    render(<EditorPanel filePath="/p/icon.svg" isVisible />)

    expect(document.querySelector('img')?.getAttribute('src')).toMatch(/^data:image\/svg\+xml/)
    expect(screen.queryByRole('button', { name: /Contents/ })).not.toBeInTheDocument()

    fireEvent.click(screen.getByTitle('Switch to source mode'))
    expect(screen.getByTestId('code-editor')).toHaveTextContent('<svg/>')

    fireEvent.click(screen.getByTitle('Switch to preview'))
    expect(document.querySelector('img')).toBeInTheDocument()
  })

  it('renders html from the saved file in a sandbox and switches to its source', () => {
    seed('/p/report.html', { content: '<h1>Report</h1>', language: 'html' })
    render(<EditorPanel filePath="/p/report.html" isVisible />)

    const frame = screen.getByTitle('Rendered report.html')
    expect(frame).toHaveAttribute('src', 'asset://localhost/p/report.html?v=5')
    // Scripts run, but without the app's origin: no IPC, no storage, no file reads.
    expect(frame).toHaveAttribute('sandbox', 'allow-scripts')
    expect(screen.queryByText(/Unsaved edits appear after saving/)).not.toBeInTheDocument()

    fireEvent.click(screen.getByTitle('Switch to source mode'))
    expect(screen.getByTestId('code-editor')).toHaveTextContent('<h1>Report</h1>')
    fireEvent.click(screen.getByTitle('Switch to preview'))
    expect(screen.getByTitle('Rendered report.html')).toBeInTheDocument()
  })

  it('says when the rendered html lags behind unsaved edits', () => {
    seed('/p/report.html', { content: '<h1>Draft</h1>', language: 'html', isDirty: true })
    render(<EditorPanel filePath="/p/report.html" isVisible />)

    expect(screen.getByText(/Unsaved edits appear after saving/)).toBeInTheDocument()
  })

  it('keeps the markdown toggle and outline', () => {
    seed('/p/README.md', { language: 'markdown', viewMode: 'markdown' })
    render(<EditorPanel filePath="/p/README.md" isVisible />)

    expect(screen.getByTestId('markdown-editor')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /Contents/ })).toBeInTheDocument()
    fireEvent.click(screen.getByTitle('Switch to source mode'))
    expect(screen.getByTestId('code-editor')).toBeInTheDocument()
    expect(screen.getByTitle('Switch to WYSIWYG mode')).toBeInTheDocument()
  })
})
