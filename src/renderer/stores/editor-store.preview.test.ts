import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/lib/api', () => ({
  filesystemApi: {
    readFile: vi.fn(),
    getFileInfo: vi.fn()
  }
}))

vi.mock('@/lib/tauri-runtime', () => ({
  isTauriContext: () => true
}))

import { filesystemApi } from '@/lib/api'
import { useEditorStore } from './editor-store'

function fileInfo(modifiedAt: number) {
  return {
    success: true as const,
    data: {
      path: '/p/logo.png',
      size: 10,
      modifiedAt,
      type: 'file' as const,
      isReadOnly: false,
      isBinary: true
    }
  }
}

describe('editor-store previewable files', () => {
  beforeEach(() => {
    vi.mocked(filesystemApi.readFile).mockReset()
    vi.mocked(filesystemApi.getFileInfo).mockReset()
  })

  afterEach(() => {
    useEditorStore.setState({ openFiles: new Map(), activeFilePath: null })
  })

  it('opens an image as a preview without reading it as text', async () => {
    vi.mocked(filesystemApi.getFileInfo).mockResolvedValue(fileInfo(7))

    await useEditorStore.getState().openFile('/p/logo.png')

    expect(filesystemApi.readFile).not.toHaveBeenCalled()
    const file = useEditorStore.getState().openFiles.get('/p/logo.png')
    expect(file).toMatchObject({ content: '', viewMode: 'preview', lastModified: 7 })
    expect(useEditorStore.getState().activeFilePath).toBe('/p/logo.png')
  })

  it('fails to open an image that is gone', async () => {
    vi.mocked(filesystemApi.getFileInfo).mockResolvedValue({
      success: false,
      error: 'No such file',
      code: 'STAT_ERROR'
    })

    await expect(useEditorStore.getState().openFile('/p/logo.png')).rejects.toThrow('No such file')
    expect(useEditorStore.getState().openFiles.size).toBe(0)
  })

  it('keeps svg source and starts on the preview', async () => {
    vi.mocked(filesystemApi.readFile).mockResolvedValue({
      success: true,
      data: { content: '<svg/>', encoding: 'utf-8', size: 6, modifiedAt: 3 }
    })

    await useEditorStore.getState().openFile('/p/icon.svg')

    expect(useEditorStore.getState().openFiles.get('/p/icon.svg')).toMatchObject({
      content: '<svg/>',
      viewMode: 'preview'
    })
  })

  it('reloads a changed image by moving its modification time', async () => {
    vi.mocked(filesystemApi.getFileInfo).mockResolvedValueOnce(fileInfo(7))
    await useEditorStore.getState().openFile('/p/logo.png')

    vi.mocked(filesystemApi.getFileInfo).mockResolvedValueOnce(fileInfo(9))
    await useEditorStore.getState().reloadFile('/p/logo.png')

    expect(filesystemApi.readFile).not.toHaveBeenCalled()
    expect(useEditorStore.getState().openFiles.get('/p/logo.png')?.lastModified).toBe(9)
  })
})
