import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/lib/api', () => ({
  filesystemApi: {
    readFile: vi.fn(),
    getFileInfo: vi.fn()
  }
}))

import { filesystemApi } from '@/lib/api'
import { type EditorFileState, useEditorStore } from './editor-store'

const path = '/project/draft.md'
const otherPath = '/project/active.ts'
const readResult = {
  success: true as const,
  data: { content: '# Draft', encoding: 'utf-8', size: 7, modifiedAt: 10 }
}

function cachedFile(filePath: string): EditorFileState {
  return {
    filePath,
    content: 'unsaved draft',
    originalContent: 'original',
    isDirty: true,
    language: 'markdown',
    lastModified: 1,
    viewMode: 'markdown',
    cursorPosition: { line: 2, col: 3 },
    scrollTop: 42,
    operationStatus: 'idle'
  }
}

describe('editor-store openFile activation', () => {
  beforeEach(() => {
    vi.mocked(filesystemApi.readFile).mockReset().mockResolvedValue(readResult)
    vi.mocked(filesystemApi.getFileInfo).mockReset()
    useEditorStore.setState({ openFiles: new Map(), activeFilePath: null })
  })

  afterEach(() => {
    useEditorStore.setState({ openFiles: new Map(), activeFilePath: null })
  })

  it.each([
    undefined,
    {},
    { activate: true }
  ])('activates a newly loaded file with options %j', async (options) => {
    useEditorStore.setState({ activeFilePath: otherPath })

    await useEditorStore.getState().openFile(path, options)

    expect(filesystemApi.readFile).toHaveBeenCalledWith(path)
    expect(useEditorStore.getState().activeFilePath).toBe(path)
    expect(useEditorStore.getState().openFiles.get(path)).toMatchObject({
      content: '# Draft',
      originalContent: '# Draft',
      isDirty: false,
      language: 'markdown',
      viewMode: 'markdown',
      lastModified: 10
    })
  })

  it.each([
    undefined,
    {},
    { activate: true }
  ])('activates a cached file with options %j without reloading its buffer', async (options) => {
    const file = cachedFile(path)
    useEditorStore.setState({ openFiles: new Map([[path, file]]), activeFilePath: otherPath })

    await useEditorStore.getState().openFile(path, options)

    expect(useEditorStore.getState().activeFilePath).toBe(path)
    expect(useEditorStore.getState().openFiles.get(path)).toBe(file)
    expect(filesystemApi.readFile).not.toHaveBeenCalled()
  })

  it.each([
    null,
    otherPath
  ])('leaves activeFilePath %j untouched when opening a cached file in the background', async (activeFilePath) => {
    const file = cachedFile(path)
    useEditorStore.setState({ openFiles: new Map([[path, file]]), activeFilePath })
    const before = useEditorStore.getState()

    await useEditorStore.getState().openFile(path, { activate: false })

    expect(useEditorStore.getState()).toBe(before)
    expect(useEditorStore.getState().activeFilePath).toBe(activeFilePath)
    expect(useEditorStore.getState().openFiles.get(path)).toBe(file)
    expect(filesystemApi.readFile).not.toHaveBeenCalled()
  })

  it.each([
    null,
    otherPath
  ])('preserves the latest activeFilePath %j after an asynchronous background read', async (latestActivePath) => {
    let resolveRead!: (result: typeof readResult) => void
    vi.mocked(filesystemApi.readFile).mockReturnValueOnce(
      new Promise((resolve) => {
        resolveRead = resolve
      })
    )
    useEditorStore.setState({ activeFilePath: '/project/previous.ts' })

    const opening = useEditorStore.getState().openFile(path, { activate: false })
    expect(useEditorStore.getState().activeFilePath).toBe('/project/previous.ts')
    expect(useEditorStore.getState().openFiles.has(path)).toBe(false)

    useEditorStore.getState().setActiveFilePath(latestActivePath)
    resolveRead(readResult)
    await opening

    expect(useEditorStore.getState().activeFilePath).toBe(latestActivePath)
    expect(useEditorStore.getState().openFiles.get(path)).toMatchObject({
      content: '# Draft',
      originalContent: '# Draft',
      isDirty: false
    })
  })

  it.each([
    false,
    true
  ])('preserves a newer dirty buffer after an interleaved read (activate=%j)', async (activate) => {
    let resolveRead!: (result: typeof readResult) => void
    vi.mocked(filesystemApi.readFile).mockReturnValueOnce(
      new Promise((resolve) => {
        resolveRead = resolve
      })
    )
    useEditorStore.setState({ activeFilePath: otherPath })
    const opening = useEditorStore.getState().openFile(path, { activate })
    const newer = cachedFile(path)
    useEditorStore.setState({ openFiles: new Map([[path, newer]]) })
    resolveRead(readResult)
    await opening
    expect(useEditorStore.getState().openFiles.get(path)).toBe(newer)
    expect(useEditorStore.getState().activeFilePath).toBe(activate ? path : otherPath)
  })

  it('keeps activeFilePath unchanged when a background read fails', async () => {
    vi.mocked(filesystemApi.readFile).mockResolvedValueOnce({
      success: false,
      error: 'Missing file',
      code: 'READ_ERROR'
    })
    useEditorStore.setState({ activeFilePath: otherPath })

    await expect(useEditorStore.getState().openFile(path, { activate: false })).rejects.toThrow(
      'Missing file'
    )

    expect(useEditorStore.getState().activeFilePath).toBe(otherPath)
    expect(useEditorStore.getState().openFiles.has(path)).toBe(false)
  })
})
