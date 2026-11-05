import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/lib/api', () => ({
  filesystemApi: {
    readFile: vi.fn(),
    getFileInfo: vi.fn()
  }
}))

const runtime = vi.hoisted(() => ({ desktop: true }))
vi.mock('@/lib/tauri-runtime', () => ({
  isTauriContext: () => runtime.desktop
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
    runtime.desktop = true
    useEditorStore.getState().clearAllFiles()
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

  it.each([
    true,
    false
  ])('keeps unusual raster formats metadata-only (desktop=%s)', async (desktop) => {
    runtime.desktop = desktop
    vi.mocked(filesystemApi.getFileInfo).mockResolvedValue(fileInfo(7))
    for (const extension of ['jfif', 'tif', 'tiff', 'heic', 'heif', 'HEIC']) {
      const path = `/p/photo.${extension}`
      await useEditorStore.getState().openFile(path)
      expect(useEditorStore.getState().openFiles.get(path)).toMatchObject({
        content: '',
        originalContent: '',
        viewMode: 'preview',
        isDirty: false
      })
    }
    expect(filesystemApi.readFile).not.toHaveBeenCalled()
  })

  it('can retry opening a preview after metadata lookup failed', async () => {
    vi.mocked(filesystemApi.getFileInfo)
      .mockResolvedValueOnce({ success: false, error: 'Offline', code: 'STAT_ERROR' })
      .mockResolvedValueOnce(fileInfo(8))
    await expect(useEditorStore.getState().openFile('/p/logo.png')).rejects.toThrow('Offline')
    await useEditorStore.getState().openFile('/p/logo.png')
    expect(useEditorStore.getState().openFiles.get('/p/logo.png')?.lastModified).toBe(8)
    expect(filesystemApi.readFile).not.toHaveBeenCalled()
  })

  it('does not resurrect a preview when its pending open is canceled', async () => {
    let resolve!: (value: ReturnType<typeof fileInfo>) => void
    vi.mocked(filesystemApi.getFileInfo).mockReturnValueOnce(
      new Promise((done) => {
        resolve = done
      })
    )
    const pending = useEditorStore.getState().openFile('/p/logo.png')
    useEditorStore.getState().closeFile('/p/logo.png')
    resolve(fileInfo(7))
    await pending
    expect(useEditorStore.getState().openFiles.size).toBe(0)
    expect(useEditorStore.getState().activeFilePath).toBeNull()
  })

  it('does not restore a pending preview after clearing the editor', async () => {
    let resolve!: (value: ReturnType<typeof fileInfo>) => void
    vi.mocked(filesystemApi.getFileInfo).mockReturnValueOnce(
      new Promise((done) => {
        resolve = done
      })
    )
    const pending = useEditorStore.getState().openFile('/p/logo.png')
    useEditorStore.getState().clearAllFiles()
    resolve(fileInfo(7))
    await pending
    expect(useEditorStore.getState().openFiles.size).toBe(0)
  })

  it('keeps the latest reload when metadata responses arrive in reverse order', async () => {
    vi.mocked(filesystemApi.getFileInfo).mockResolvedValueOnce(fileInfo(7))
    await useEditorStore.getState().openFile('/p/logo.png')
    let resolveFirst!: (value: ReturnType<typeof fileInfo>) => void
    vi.mocked(filesystemApi.getFileInfo)
      .mockReturnValueOnce(
        new Promise((done) => {
          resolveFirst = done
        })
      )
      .mockResolvedValueOnce(fileInfo(10))
    const first = useEditorStore.getState().reloadFile('/p/logo.png')
    await useEditorStore.getState().reloadFile('/p/logo.png')
    resolveFirst(fileInfo(8))
    await first
    expect(useEditorStore.getState().openFiles.get('/p/logo.png')).toMatchObject({
      lastModified: 10,
      operationStatus: 'idle'
    })
  })

  it('ignores an old reload after closing and reopening the same preview path', async () => {
    vi.mocked(filesystemApi.getFileInfo).mockResolvedValueOnce(fileInfo(7))
    await useEditorStore.getState().openFile('/p/logo.png')
    let resolveOld!: (value: ReturnType<typeof fileInfo>) => void
    vi.mocked(filesystemApi.getFileInfo).mockReturnValueOnce(
      new Promise((done) => {
        resolveOld = done
      })
    )
    const oldReload = useEditorStore.getState().reloadFile('/p/logo.png')
    useEditorStore.getState().closeFile('/p/logo.png')
    vi.mocked(filesystemApi.getFileInfo).mockResolvedValueOnce(fileInfo(12))
    await useEditorStore.getState().openFile('/p/logo.png')
    resolveOld(fileInfo(8))
    await oldReload
    expect(useEditorStore.getState().openFiles.get('/p/logo.png')).toMatchObject({
      lastModified: 12,
      operationStatus: 'idle'
    })
  })

  it('leaves a failed reload retryable without discarding the current preview', async () => {
    vi.mocked(filesystemApi.getFileInfo).mockResolvedValueOnce(fileInfo(7))
    await useEditorStore.getState().openFile('/p/logo.png')
    vi.mocked(filesystemApi.getFileInfo).mockRejectedValueOnce(new Error('Temporary network error'))
    await useEditorStore.getState().reloadFile('/p/logo.png')
    expect(useEditorStore.getState().openFiles.get('/p/logo.png')).toMatchObject({
      lastModified: 7,
      operationStatus: 'idle'
    })
    vi.mocked(filesystemApi.getFileInfo).mockResolvedValueOnce(fileInfo(9))
    await useEditorStore.getState().reloadFile('/p/logo.png')
    expect(useEditorStore.getState().openFiles.get('/p/logo.png')?.lastModified).toBe(9)
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
