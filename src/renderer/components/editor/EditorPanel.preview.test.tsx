import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { type EditorFileState, useEditorStore } from '@/stores/editor-store'
import { EditorPanel } from './EditorPanel'

const mocks = vi.hoisted(() => ({
  desktop: true,
  isMac: true,
  failSource: false,
  reveal: vi.fn(),
  clipboard: vi.fn(),
  toastSuccess: vi.fn(),
  toastError: vi.fn(),
  loadImage: vi.fn(async (path: string) => ({
    success: true as const,
    data: { url: `blob:${path}`, dispose: vi.fn() }
  })),
  logError: vi.fn().mockResolvedValue(undefined)
}))
vi.mock('@/lib/file-preview-api', () => ({ loadImagePreview: mocks.loadImage }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: mocks.logError }))
vi.mock('@/lib/tauri-runtime', () => ({
  isTauriContext: () => mocks.desktop
}))
vi.mock('@/lib/platform', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/platform')>()
  return {
    ...actual,
    get isMac() {
      return mocks.isMac
    }
  }
})
vi.mock('@/lib/tauri-opener-api', () => ({
  openerApi: {
    revealInFileManager: mocks.reveal,
    openWithExternalApp: vi.fn(),
    openUrlWithSystemBrowser: vi.fn()
  }
}))
vi.mock('@/lib/clipboard-api', () => ({
  clipboardApi: {
    writeText: mocks.clipboard,
    readText: vi.fn(),
    hasImage: vi.fn()
  }
}))
vi.mock('sonner', () => ({
  toast: {
    success: mocks.toastSuccess,
    error: mocks.toastError,
    warning: vi.fn(),
    info: vi.fn()
  }
}))

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: (path: string) => {
    if (mocks.failSource) throw new Error('Preview source failed')
    return `asset://localhost${path}`
  }
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
  beforeEach(() => {
    mocks.desktop = true
    mocks.isMac = true
    mocks.reveal.mockReset().mockResolvedValue({ success: true })
    mocks.clipboard.mockReset().mockResolvedValue({ success: true })
    mocks.toastSuccess.mockReset()
    mocks.toastError.mockReset()
  })

  afterEach(() => {
    cleanup()
    mocks.desktop = true
    mocks.failSource = false
    mocks.logError.mockClear()
    mocks.loadImage.mockClear()
    vi.restoreAllMocks()
    useEditorStore.setState({ openFiles: new Map(), activeFilePath: null })
  })

  it('shows an image from the owned binary loader', async () => {
    seed('/p/logo.png', {})
    render(<EditorPanel filePath="/p/logo.png" isVisible />)

    await waitFor(() =>
      expect(document.querySelector('img')).toHaveAttribute('src', 'blob:/p/logo.png')
    )
    expect(mocks.loadImage).toHaveBeenCalledWith('/p/logo.png', expect.any(AbortSignal))
    expect(screen.queryByTestId('code-editor')).not.toBeInTheDocument()
  })

  it.each([
    'jfif',
    'tif',
    'tiff',
    'heic',
    'heif',
    'HEIC'
  ])('routes %s to a read-only raster preview on desktop', async (extension) => {
    const path = `/p/photo.${extension}`
    seed(path, {})
    render(<EditorPanel filePath={path} isVisible />)
    await waitFor(() => expect(document.querySelector('img')).toBeInTheDocument())
    expect(mocks.loadImage).toHaveBeenCalledWith(path, expect.any(AbortSignal))
    expect(screen.queryByTestId('code-editor')).not.toBeInTheDocument()
    expect(screen.queryByTitle('Switch to source mode')).not.toBeInTheDocument()
  })

  it.each([
    'png',
    'jpg',
    'jfif',
    'tif',
    'tiff',
    'heic',
    'heif'
  ])('routes %s to raster preview on web rather than a text editor', async (extension) => {
    mocks.desktop = false
    const path = `/p/photo.${extension}`
    seed(path, {})
    render(<EditorPanel filePath={path} isVisible />)
    await waitFor(() => expect(document.querySelector('img')).toBeInTheDocument())
    expect(mocks.loadImage).toHaveBeenCalledWith(path, expect.any(AbortSignal))
    expect(screen.queryByTestId('code-editor')).not.toBeInTheDocument()
    expect(document.querySelector('img')?.getAttribute('src')).not.toContain('asset://')
  })

  it('keeps web HTML preview disabled', () => {
    mocks.desktop = false
    seed('/p/report.html', { language: 'html', content: '<h1>Report</h1>' })
    render(<EditorPanel filePath="/p/report.html" isVisible />)
    expect(screen.getByTestId('code-editor')).toHaveTextContent('<h1>Report</h1>')
    expect(document.querySelector('iframe')).not.toBeInTheDocument()
  })

  it('logs source resolution failures and retries without trapping the editor pane', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {})
    mocks.failSource = true
    seed('/p/sound.mp3', {})
    render(<EditorPanel filePath="/p/sound.mp3" isVisible />)
    expect(screen.getByRole('alert')).toHaveTextContent('Unable to display this file')
    expect(mocks.logError).toHaveBeenCalledWith(
      expect.objectContaining({
        source: 'ErrorBoundary:editorPane',
        message: 'Preview source failed'
      })
    )
    mocks.failSource = false
    fireEvent.click(screen.getByRole('button', { name: 'Retry preview' }))
    expect(document.querySelector('audio')).toHaveAttribute(
      'src',
      'asset://localhost/p/sound.mp3?v=5'
    )
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('resets a source boundary failure when another file is opened', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {})
    mocks.failSource = true
    seed('/p/sound.mp3', {})
    const { rerender } = render(<EditorPanel filePath="/p/sound.mp3" isVisible />)
    expect(screen.getByRole('alert')).toBeInTheDocument()
    mocks.failSource = false
    seed('/p/next.mp3', {})
    rerender(<EditorPanel filePath="/p/next.mp3" isVisible />)
    expect(document.querySelector('audio')).toHaveAttribute(
      'src',
      'asset://localhost/p/next.mp3?v=5'
    )
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
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

  const surfaces: Array<[string, string, Partial<EditorFileState>]> = [
    ['plain text', '/p/notes.txt', { content: 'hello', language: 'plaintext' }],
    ['markdown', '/p/README.md', { content: '# hi', language: 'markdown', viewMode: 'markdown' }],
    ['html', '/p/report.html', { content: '<h1>Report</h1>', language: 'html' }],
    ['svg', '/p/icon.svg', { content: '<svg/>', language: 'xml' }],
    ['raster media', '/p/photo.png', {}],
    ['audio media', '/p/sound.mp3', {}]
  ]

  it.each(
    surfaces
  )('offers the desktop file-location entry on %s panels and reveals the exact path', async (_label, path, overrides) => {
    seed(path, overrides)
    render(<EditorPanel filePath={path} isVisible />)

    const button = screen.getByRole('button', { name: 'Show in Finder' })
    expect(screen.queryByRole('button', { name: 'Copy Path' })).not.toBeInTheDocument()

    fireEvent.click(button)

    await waitFor(() => expect(mocks.reveal).toHaveBeenCalledWith(path))
    expect(mocks.reveal).toHaveBeenCalledTimes(1)
    expect(mocks.clipboard).not.toHaveBeenCalled()
  })

  it('uses the non-macOS desktop label and still reveals the exact path', async () => {
    mocks.isMac = false
    seed('/p/notes.txt', { content: 'hello', language: 'plaintext' })
    render(<EditorPanel filePath="/p/notes.txt" isVisible />)

    expect(screen.queryByRole('button', { name: 'Show in Finder' })).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Show in File Manager' }))

    await waitFor(() => expect(mocks.reveal).toHaveBeenCalledWith('/p/notes.txt'))
  })

  it('only copies the path in the browser edition', async () => {
    mocks.desktop = false
    seed('/p/notes.txt', { content: 'hello', language: 'plaintext' })
    render(<EditorPanel filePath="/p/notes.txt" isVisible />)

    expect(screen.queryByRole('button', { name: 'Show in Finder' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Show in File Manager' })).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Copy Path' }))

    await waitFor(() => expect(mocks.clipboard).toHaveBeenCalledWith('/p/notes.txt'))
    expect(mocks.reveal).not.toHaveBeenCalled()
    await waitFor(() => expect(mocks.toastSuccess).toHaveBeenCalledWith('Path copied'))
  })

  it('surfaces a reveal failure with a fixed toast and a safe code log', async () => {
    mocks.reveal.mockResolvedValueOnce({
      success: false,
      error: 'ENOENT: /Users/qs/private/secret-notes.md'
    })
    seed('/p/notes.txt', { content: 'hello', language: 'plaintext' })
    render(<EditorPanel filePath="/p/notes.txt" isVisible />)

    fireEvent.click(screen.getByRole('button', { name: 'Show in Finder' }))

    await waitFor(() =>
      expect(mocks.toastError).toHaveBeenCalledWith(
        'Could not show the original file in the file manager'
      )
    )
    expect(mocks.logError).toHaveBeenCalledWith({
      level: 'warn',
      source: 'editor.file-location',
      message: expect.stringContaining('operation=reveal code=FILE_LOCATION_FAILED')
    })
    expect(JSON.stringify(mocks.logError.mock.calls)).not.toContain(
      '/Users/qs/private/secret-notes.md'
    )
    expect(JSON.stringify(mocks.toastError.mock.calls)).not.toContain(
      '/Users/qs/private/secret-notes.md'
    )
  })

  it('turns a thrown reveal error into the same safe failure without leaking details', async () => {
    mocks.reveal.mockRejectedValueOnce(new Error('native failure at /Users/qs/private/secret.md'))
    seed('/p/notes.txt', { content: 'hello', language: 'plaintext' })
    render(<EditorPanel filePath="/p/notes.txt" isVisible />)

    fireEvent.click(screen.getByRole('button', { name: 'Show in Finder' }))

    await waitFor(() => expect(mocks.toastError).toHaveBeenCalledTimes(1))
    expect(mocks.logError).toHaveBeenCalledWith(
      expect.objectContaining({
        source: 'editor.file-location',
        message: expect.stringContaining('code=FILE_LOCATION_FAILED')
      })
    )
    expect(JSON.stringify(mocks.logError.mock.calls)).not.toContain('/Users/qs/private/secret.md')
  })

  it('surfaces a browser copy failure without exposing the clipboard error', async () => {
    mocks.desktop = false
    mocks.clipboard.mockResolvedValueOnce({ success: false, error: 'denied /Users/qs/private' })
    seed('/p/notes.txt', { content: 'hello', language: 'plaintext' })
    render(<EditorPanel filePath="/p/notes.txt" isVisible />)

    fireEvent.click(screen.getByRole('button', { name: 'Copy Path' }))

    await waitFor(() =>
      expect(mocks.toastError).toHaveBeenCalledWith('Could not copy the file path')
    )
    expect(mocks.logError).toHaveBeenCalledWith(
      expect.objectContaining({
        source: 'editor.file-location',
        message: expect.stringContaining('operation=copy_path code=FILE_LOCATION_FAILED')
      })
    )
    expect(JSON.stringify(mocks.logError.mock.calls)).not.toContain('/Users/qs/private')
  })
})
