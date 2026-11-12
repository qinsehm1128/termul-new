import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/lib/api', () => ({
  filesystemApi: { readFile: vi.fn(), getFileInfo: vi.fn(), writeFile: vi.fn() }
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
vi.mock('sonner', () => ({ toast: { error: vi.fn(), warning: vi.fn() } }))
vi.mock('@/components/editor/EditorPanel', () => ({
  EditorPanel: ({ filePath }: { filePath: string }) => (
    <div data-testid="editor" data-path={filePath} />
  )
}))

import { toast } from 'sonner'
import { filesystemApi } from '@/lib/api'
import { logFrontendError } from '@/lib/log-api'
import { type EditorFileState, useEditorStore } from '@/stores/editor-store'
import { QuickTerminalWorkspace } from './QuickTerminalWorkspace'
import { closeQuickTab } from './quick-tab-close'
import { resetQuickEditorDrafts } from './quick-terminal-editor-drafts'
import { resetQuickWorkspaceStore, useQuickWorkspaceStore } from './quick-terminal-workspace-store'

const id = '22222222-2222-4222-8222-222222222222'
const record = {
  schemaVersion: 1 as const,
  id,
  target: { kind: 'workspace' as const },
  cwd: '/scratch',
  createdAtUtc: '2026-10-26T00:00:00Z',
  updatedAtUtc: '2026-10-26T00:00:00Z',
  origin: 'created' as const
}
const path = '/scratch/edited.ts'
const savedFile = {
  filePath: path,
  content: 'same',
  originalContent: 'same',
  isDirty: false,
  language: 'typescript',
  viewMode: 'code' as const,
  lastModified: 1,
  cursorPosition: { line: 1, col: 1 },
  scrollTop: 0,
  operationStatus: 'idle' as const
}
const dirtyFile: EditorFileState = { ...savedFile, content: 'edited', isDirty: true }
const diskFile = { success: true as const, data: { content: 'same', modifiedAt: 1, size: 4 } }

function setup(file: EditorFileState = dirtyFile) {
  useEditorStore.setState({ openFiles: new Map([[path, file]]), activeFilePath: null })
  useQuickWorkspaceStore.setState({
    layouts: {
      [id]: {
        tabs: [{ id: `editor:${path}`, type: 'editor' as const, filePath: path }],
        activeTabId: `editor:${path}`,
        lastTerminalTabId: null
      }
    }
  })
  return render(<QuickTerminalWorkspace record={record} />)
}

const tabs = (): number => useQuickWorkspaceStore.getState().layouts[id].tabs.length

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(filesystemApi.readFile).mockResolvedValue(diskFile as never)
  vi.mocked(filesystemApi.writeFile).mockResolvedValue({ success: true } as never)
  resetQuickEditorDrafts()
  resetQuickWorkspaceStore()
  useEditorStore.setState({ openFiles: new Map(), activeFilePath: null })
})
afterEach(() => cleanup())

describe('quick workspace dirty editor close', () => {
  it('asks before closing a dirty file instead of refusing to close', async () => {
    setup()
    await screen.findByTestId('editor')

    fireEvent.click(screen.getByRole('button', { name: 'Close preview' }))

    expect(await screen.findByText('Unsaved Changes')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Save' })).toBeTruthy()
    // Destructive consequence is explicit, not a generic "Discard".
    expect(screen.getByRole('button', { name: "Don't Save" })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeTruthy()
    expect(tabs()).toBe(1)
  })

  it('discards the dirty buffer and closes the tab without writing to disk', async () => {
    setup()
    await screen.findByTestId('editor')
    fireEvent.click(screen.getByRole('button', { name: 'Close preview' }))
    fireEvent.click(await screen.findByRole('button', { name: "Don't Save" }))

    await waitFor(() => expect(tabs()).toBe(0))
    expect(useEditorStore.getState().openFiles.has(path)).toBe(false)
    expect(filesystemApi.writeFile).not.toHaveBeenCalled()
    await waitFor(() => expect(screen.queryByText('Unsaved Changes')).toBeNull())
  })

  it('saves then closes the tab on Save', async () => {
    setup()
    await screen.findByTestId('editor')
    fireEvent.click(screen.getByRole('button', { name: 'Close preview' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Save' }))

    await waitFor(() => expect(tabs()).toBe(0))
    expect(filesystemApi.writeFile).toHaveBeenCalledWith(path, 'edited')
    await waitFor(() => expect(screen.queryByText('Unsaved Changes')).toBeNull())
  })

  it('keeps the tab and the dirty buffer on Cancel', async () => {
    setup()
    await screen.findByTestId('editor')
    fireEvent.click(screen.getByRole('button', { name: 'Close preview' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Cancel' }))

    await waitFor(() => expect(screen.queryByText('Unsaved Changes')).toBeNull())
    expect(tabs()).toBe(1)
    expect(useEditorStore.getState().openFiles.get(path)).toMatchObject({
      content: 'edited',
      isDirty: true
    })
  })

  it('closes a clean file without asking', async () => {
    setup(savedFile)
    await screen.findByTestId('editor')
    fireEvent.click(screen.getByRole('button', { name: 'Close preview' }))

    await waitFor(() => expect(tabs()).toBe(0))
    expect(screen.queryByText('Unsaved Changes')).toBeNull()
  })

  it('keeps the tab open and reports a failed save', async () => {
    vi.mocked(filesystemApi.writeFile).mockResolvedValue({
      success: false,
      error: 'disk full'
    } as never)
    setup()
    await screen.findByTestId('editor')
    fireEvent.click(screen.getByRole('button', { name: 'Close preview' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Save' }))

    await waitFor(() => expect(toast.error).toHaveBeenCalled())
    expect(tabs()).toBe(1)
    expect(useEditorStore.getState().openFiles.get(path)?.isDirty).toBe(true)
    // Sanitized log: no path, no file content.
    const logged = JSON.stringify(vi.mocked(logFrontendError).mock.calls)
    expect(logged).not.toContain('/scratch')
    expect(logged).not.toContain('edited')
  })

  it('routes the shared close command (keyboard shortcut / menu) through the same dialog', async () => {
    setup()
    await screen.findByTestId('editor')

    act(() => closeQuickTab())

    expect(await screen.findByText('Unsaved Changes')).toBeTruthy()
    expect(tabs()).toBe(1)
  })
})
