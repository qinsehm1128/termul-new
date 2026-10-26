import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { useEffect, useRef, useState } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/lib/api', () => ({
  filesystemApi: { readFile: vi.fn(), getFileInfo: vi.fn() }
}))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn(async () => undefined) }))
vi.mock('@/lib/persistence-api', () => ({
  persistenceApi: {
    read: vi.fn(async () => ({ success: true, data: null })),
    writeDebounced: vi.fn(async () => ({ success: true })),
    flushPendingWrites: vi.fn(async () => ({ success: true }))
  }
}))
vi.mock('./QuickTerminalView', () => ({ QuickTerminalView: () => null }))
vi.mock('sonner', () => ({ toast: { error: vi.fn() } }))

// Model the actual 300ms buffer boundary using the production flusher registry.
// Like CodeMirror, this local document survives prop changes unless keyed by file identity.
vi.mock('@/components/editor/EditorPanel', () => ({
  EditorPanel: ({ filePath }: { filePath: string }) => {
    const [document, setDocument] = useState(
      () => useEditorStore.getState().openFiles.get(filePath)?.content ?? 'same'
    )
    const current = useRef({ filePath, document })
    current.current = { filePath, document }
    const pending = useRef<ReturnType<typeof setTimeout> | null>(null)
    useEffect(() => {
      const flush = (): void => {
        if (!pending.current) return
        clearTimeout(pending.current)
        pending.current = null
        useEditorStore.getState().updateContent(current.current.filePath, current.current.document)
      }
      registerEditorContentFlusher(filePath, flush)
      return () => {
        unregisterEditorContentFlusher(filePath)
        if (pending.current) clearTimeout(pending.current)
      }
    }, [filePath])
    return (
      <textarea
        aria-label="Live editor"
        data-path={filePath}
        value={document}
        onChange={(event) => {
          const value = event.target.value
          setDocument(value)
          current.current.document = value
          if (pending.current) clearTimeout(pending.current)
          pending.current = setTimeout(() => {
            pending.current = null
            useEditorStore
              .getState()
              .updateContent(current.current.filePath, current.current.document)
          }, 300)
        }}
      />
    )
  }
}))

import { filesystemApi } from '@/lib/api'
import {
  registerEditorContentFlusher,
  unregisterEditorContentFlusher
} from '@/lib/editor-content-flush'
import { logFrontendError } from '@/lib/log-api'
import { type EditorFileState, useEditorStore } from '@/stores/editor-store'
import { QuickTerminalWorkspace } from './QuickTerminalWorkspace'
import { closeQuickTab } from './quick-tab-close'
import { resetQuickEditorDrafts } from './quick-terminal-editor-drafts'
import { resetQuickWorkspaceStore, useQuickWorkspaceStore } from './quick-terminal-workspace-store'

const id = '11111111-1111-4111-8111-111111111111'
const record = {
  schemaVersion: 1 as const,
  id,
  target: { kind: 'workspace' as const },
  cwd: '/scratch',
  createdAtUtc: '2026-10-26T00:00:00Z',
  updatedAtUtc: '2026-10-26T00:00:00Z',
  origin: 'created' as const
}
const a = '/scratch/a.ts'
const b = '/scratch/b.ts'
const file = (filePath: string): EditorFileState => ({
  filePath,
  content: 'same',
  originalContent: 'same',
  isDirty: false,
  language: 'typescript',
  viewMode: 'code',
  lastModified: 1,
  cursorPosition: { line: 1, col: 1 },
  scrollTop: 0,
  operationStatus: 'idle'
})
const diskFile = { success: true as const, data: { content: 'same', modifiedAt: 1, size: 4 } }

function setup(paths = [a, b], cached = true) {
  useEditorStore.setState({
    openFiles: new Map(cached ? paths.map((path) => [path, file(path)]) : []),
    activeFilePath: null
  })
  useQuickWorkspaceStore.setState({
    layouts: {
      [id]: {
        tabs: paths.map((filePath) => ({ id: filePath, type: 'editor' as const, filePath })),
        activeTabId: paths[0] ?? null,
        lastTerminalTabId: null
      }
    }
  })
  return render(<QuickTerminalWorkspace record={record} />)
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(filesystemApi.readFile).mockResolvedValue(diskFile as never)
  resetQuickEditorDrafts()
  resetQuickWorkspaceStore()
  useEditorStore.setState({ openFiles: new Map(), activeFilePath: null })
})
afterEach(() => cleanup())

describe('quick editor ownership safety', () => {
  it('flushes A before switching to identical B and never sends its debounce to B', async () => {
    setup()
    const outgoing = await screen.findByRole('textbox', { name: 'Live editor' })
    await waitFor(() => expect(useEditorStore.getState().activeFilePath).toBe(a))
    fireEvent.change(outgoing, { target: { value: 'A unsaved' } })
    expect(useEditorStore.getState().openFiles.get(a)?.isDirty).toBe(false)
    fireEvent.click(screen.getByRole('tab', { name: 'b.ts' }))
    await waitFor(() => expect(screen.getByRole('textbox')).not.toBe(outgoing))
    expect(useEditorStore.getState().openFiles.get(a)?.content).toBe('A unsaved')
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('same')
    await act(() => new Promise((resolve) => setTimeout(resolve, 350)))
    expect(useEditorStore.getState().openFiles.get(b)).toMatchObject({
      content: 'same',
      isDirty: false
    })
  })

  it('flushes immediate type-close before deciding dirty state and retains the tab', async () => {
    setup([a])
    const editor = await screen.findByRole('textbox')
    fireEvent.change(editor, { target: { value: 'not yet debounced' } })
    expect(useEditorStore.getState().openFiles.get(a)?.isDirty).toBe(false)
    fireEvent.click(screen.getByRole('button', { name: 'Close preview' }))
    await waitFor(() => expect(useEditorStore.getState().openFiles.get(a)?.isDirty).toBe(true))
    expect(useEditorStore.getState().openFiles.get(a)?.content).toBe('not yet debounced')
    expect(screen.getByRole('textbox')).toBe(editor)
    expect(useQuickWorkspaceStore.getState().layouts[id].tabs).toHaveLength(1)
  })

  it('uses the routed close owner with the same immediate-edit protection', async () => {
    setup([a])
    const editor = await screen.findByRole('textbox')
    fireEvent.change(editor, { target: { value: 'routed close draft' } })
    act(() => closeQuickTab())
    await waitFor(() => expect(useEditorStore.getState().openFiles.get(a)?.isDirty).toBe(true))
    expect(useQuickWorkspaceStore.getState().layouts[id].tabs).toHaveLength(1)
  })

  it('flushes on navigation before releasing the live editor buffer', async () => {
    const view = setup([a])
    const editor = await screen.findByRole('textbox')
    fireEvent.change(editor, { target: { value: 'navigation draft' } })
    view.unmount()
    await waitFor(() =>
      expect(useEditorStore.getState().openFiles.get(a)).toMatchObject({
        content: 'navigation draft',
        isDirty: true
      })
    )
  })

  it('does not activate a delayed restored file after navigating away', async () => {
    let finish!: (result: typeof diskFile) => void
    vi.mocked(filesystemApi.readFile).mockReturnValue(
      new Promise((resolve) => {
        finish = resolve
      }) as never
    )
    const view = setup([a], false)
    await waitFor(() => expect(filesystemApi.readFile).toHaveBeenCalledWith(a))
    view.unmount()
    useEditorStore.getState().setActiveFilePath('/project/visible.ts')
    await act(async () => {
      finish(diskFile)
      await Promise.resolve()
    })
    await waitFor(() => expect(useEditorStore.getState().openFiles.has(a)).toBe(true))
    expect(useEditorStore.getState().activeFilePath).toBe('/project/visible.ts')
  })

  it('does not activate an old delayed read after selecting another quick file', async () => {
    let finish!: (result: typeof diskFile) => void
    vi.mocked(filesystemApi.readFile).mockImplementation((path) =>
      path === a
        ? (new Promise((resolve) => {
            finish = resolve
          }) as never)
        : (Promise.resolve(diskFile) as never)
    )
    setup([a, b], false)
    await waitFor(() => expect(filesystemApi.readFile).toHaveBeenCalledWith(a))
    fireEvent.click(screen.getByRole('tab', { name: 'b.ts' }))
    await waitFor(() => expect(useEditorStore.getState().activeFilePath).toBe(b))
    await act(async () => {
      finish(diskFile)
      await Promise.resolve()
    })
    expect(useEditorStore.getState().activeFilePath).toBe(b)
  })

  it('catches missing file reads, logs sanitized failure, and supports retry and close', async () => {
    vi.mocked(filesystemApi.readFile).mockRejectedValueOnce(new Error('private /secret/path'))
    setup([a], false)
    await screen.findByRole('alert')
    expect(useEditorStore.getState().activeFilePath).toBeNull()
    expect(logFrontendError).toHaveBeenCalledWith(
      expect.objectContaining({ message: 'operation=editor-load code=EDITOR_FAILED' })
    )
    expect(JSON.stringify(vi.mocked(logFrontendError).mock.calls)).not.toContain('/secret/path')
    fireEvent.click(screen.getByRole('button', { name: 'Retry loading file' }))
    await waitFor(() => expect(useEditorStore.getState().activeFilePath).toBe(a))
    expect(screen.queryByRole('alert')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Close preview' }))
    await waitFor(() => expect(useQuickWorkspaceStore.getState().layouts[id].tabs).toHaveLength(0))
  })

  it('allows closing a failed restored file without retrying it', async () => {
    vi.mocked(filesystemApi.readFile).mockRejectedValueOnce(new Error('missing'))
    setup([a], false)
    await screen.findByRole('alert')
    fireEvent.click(screen.getByRole('alert').querySelectorAll('button')[1])
    await waitFor(() => expect(useQuickWorkspaceStore.getState().layouts[id].tabs).toHaveLength(0))
    expect(filesystemApi.readFile).toHaveBeenCalledTimes(1)
  })
})
