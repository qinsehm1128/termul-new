import { readFileSync } from 'node:fs'
import type { IpcResult } from '@shared/types/ipc.types'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const readImage = vi.hoisted(() => vi.fn())
const runtime = vi.hoisted(() => ({ desktop: false }))
vi.mock('./tauri-runtime', () => ({ isTauriContext: () => runtime.desktop }))
vi.mock('./filesystem-api', () => ({ filesystemApi: { readImage } }))

import { loadImagePreview } from './file-preview-api'

const createObjectURL = vi.fn()
const NativeFileReader = globalThis.FileReader
const revokeObjectURL = vi.fn()
const readAsDataURL = vi.fn()
const abortReader = vi.fn()
let reader: MockFileReader
class MockFileReader {
  result: string | ArrayBuffer | null = null
  error: DOMException | null = null
  onload: (() => void) | null = null
  onerror: (() => void) | null = null
  onabort: (() => void) | null = null
  constructor() {
    reader = this
  }
  readAsDataURL = readAsDataURL
  abort = abortReader
}

beforeEach(() => {
  runtime.desktop = false
  readAsDataURL.mockReset()
  abortReader.mockReset()
  vi.stubGlobal('FileReader', MockFileReader)
  readImage.mockReset()
  createObjectURL.mockReset().mockReturnValue('blob:preview')
  revokeObjectURL.mockReset()
  vi.stubGlobal(
    'URL',
    class extends URL {
      static createObjectURL = createObjectURL
      static revokeObjectURL = revokeObjectURL
    }
  )
})
afterEach(() => vi.unstubAllGlobals())

describe('desktop image preview CSP and cancellation', () => {
  it('preserves arbitrary binary bytes with the native DOM FileReader', async () => {
    runtime.desktop = true
    vi.stubGlobal('FileReader', NativeFileReader)
    readImage.mockResolvedValue({
      success: true,
      data: new Blob([new Uint8Array([0, 255, 128])], { type: 'image/png' })
    })
    const result = await loadImagePreview('/p/image.png')
    expect(result).toMatchObject({ success: true, data: { url: 'data:image/png;base64,AP+A' } })
    result.data?.dispose()
    expect(createObjectURL).not.toHaveBeenCalled()
    expect(revokeObjectURL).not.toHaveBeenCalled()
  })

  it('discards a data URL when cancelled after encoding but before publication', async () => {
    runtime.desktop = true
    readImage.mockResolvedValue({ success: true, data: new Blob(['image'], { type: 'image/png' }) })
    const controller = new AbortController()
    const pending = loadImagePreview('/p/image.png', controller.signal)
    await vi.waitFor(() => expect(readAsDataURL).toHaveBeenCalledTimes(1))
    reader.result = 'data:image/png;base64,aW1hZ2U='
    reader.onload?.()
    controller.abort()
    expect(await pending).toMatchObject({ success: false, code: 'ABORTED' })
    expect(createObjectURL).not.toHaveBeenCalled()
    expect(revokeObjectURL).not.toHaveBeenCalled()
  })

  it('uses the validated MIME-specific data URL allowed by the current img-src', async () => {
    runtime.desktop = true
    const blob = new Blob([new Uint8Array([0, 255, 128])], { type: 'image/png' })
    readImage.mockResolvedValue({ success: true, data: blob })
    const controller = new AbortController()
    const pending = loadImagePreview('/p/animation.apng', controller.signal)
    await vi.waitFor(() => expect(readAsDataURL).toHaveBeenCalledExactlyOnceWith(blob))
    reader.result = 'data:image/png;base64,AP+A'
    reader.onload?.()
    const result = await pending
    expect(result).toMatchObject({ success: true, data: { url: reader.result } })
    const config = JSON.parse(readFileSync('src-tauri/tauri.conf.json', 'utf-8'))
    const imgSrc = config.app.security.csp
      .split(';')
      .map((directive: string) => directive.trim().split(/\s+/))
      .find((tokens: string[]) => tokens[0] === 'img-src')
    expect(imgSrc).toContain(result.data?.url.split(':', 1)[0].concat(':'))
    expect(createObjectURL).not.toHaveBeenCalled()
    result.data?.dispose()
    result.data?.dispose()
    controller.abort()
    expect(revokeObjectURL).not.toHaveBeenCalled()
    expect(abortReader).not.toHaveBeenCalled()
    expect(reader.onload).toBeNull()
  })

  it('aborts encoding and cannot publish a late data URL after cancellation', async () => {
    runtime.desktop = true
    readImage.mockResolvedValue({ success: true, data: new Blob(['image'], { type: 'image/png' }) })
    const controller = new AbortController()
    const pending = loadImagePreview('/p/image.png', controller.signal)
    await vi.waitFor(() => expect(readAsDataURL).toHaveBeenCalledTimes(1))
    controller.abort()
    expect(await pending).toMatchObject({ success: false, code: 'ABORTED' })
    expect(abortReader).toHaveBeenCalledTimes(1)
    expect(reader.onload).toBeNull()
    expect(reader.onerror).toBeNull()
    expect(reader.onabort).toBeNull()
    expect(createObjectURL).not.toHaveBeenCalled()
    expect(revokeObjectURL).not.toHaveBeenCalled()
  })

  it.each([
    'error',
    'invalid-result',
    'throw',
    'reader-abort'
  ])('returns a controlled failure for FileReader %s', async (failure) => {
    runtime.desktop = true
    readImage.mockResolvedValue({ success: true, data: new Blob(['image'], { type: 'image/png' }) })
    if (failure === 'throw') {
      readAsDataURL.mockImplementation(() => {
        throw new Error('Encoding unavailable')
      })
    }
    const pending = loadImagePreview('/p/image.png')
    await vi.waitFor(() => expect(readAsDataURL).toHaveBeenCalledTimes(1))
    if (failure === 'error') {
      reader.error = new DOMException('Encoding failed', 'NotReadableError')
      reader.onerror?.()
    } else if (failure === 'invalid-result') {
      reader.onload?.()
    } else if (failure === 'reader-abort') {
      reader.onabort?.()
    }
    expect(await pending).toMatchObject({
      success: false,
      code: failure === 'reader-abort' ? 'ABORTED' : 'IMAGE_PREVIEW_ERROR'
    })
    expect(reader.onload).toBeNull()
    expect(createObjectURL).not.toHaveBeenCalled()
    expect(revokeObjectURL).not.toHaveBeenCalled()
  })
})

describe('image preview object URL ownership', () => {
  it('allocates only after successful binary loading and disposes once', async () => {
    const blob = new Blob([new Uint8Array([0, 255, 128])], { type: 'image/png' })
    readImage.mockResolvedValue({ success: true, data: blob })
    const controller = new AbortController()
    const result = await loadImagePreview('/p/image.png', controller.signal)
    expect(readImage).toHaveBeenCalledWith('/p/image.png', controller.signal)
    expect(createObjectURL).toHaveBeenCalledWith(blob)
    expect(result.success).toBe(true)
    expect(result.data?.url).toBe('blob:preview')
    result.data?.dispose()
    result.data?.dispose()
    controller.abort()
    expect(revokeObjectURL).toHaveBeenCalledExactlyOnceWith('blob:preview')
  })

  it('revokes a resolved preview when its request is aborted', async () => {
    readImage.mockResolvedValue({ success: true, data: new Blob(['image']) })
    const controller = new AbortController()
    const result = await loadImagePreview('/p/image.png', controller.signal)
    controller.abort()
    expect(revokeObjectURL).toHaveBeenCalledExactlyOnceWith('blob:preview')
    result.data?.dispose()
    expect(revokeObjectURL).toHaveBeenCalledTimes(1)
  })

  it('does not load a cancelled request or create a URL', async () => {
    const controller = new AbortController()
    controller.abort()
    expect(await loadImagePreview('/p/image.png', controller.signal)).toMatchObject({
      success: false,
      code: 'ABORTED'
    })
    expect(readImage).not.toHaveBeenCalled()
    expect(createObjectURL).not.toHaveBeenCalled()
  })

  it('does not allocate for a late desktop response after cancellation', async () => {
    runtime.desktop = true
    let resolve!: (result: IpcResult<Blob>) => void
    readImage.mockReturnValue(
      new Promise<IpcResult<Blob>>((done) => {
        resolve = done
      })
    )
    const controller = new AbortController()
    const pending = loadImagePreview('/p/image.png', controller.signal)
    controller.abort()
    resolve({ success: true, data: new Blob(['image']) })
    expect(await pending).toMatchObject({ success: false, code: 'ABORTED' })
    expect(createObjectURL).not.toHaveBeenCalled()
  })

  it('keeps independent request URLs so disposing stale results cannot revoke newer previews', async () => {
    readImage.mockResolvedValue({ success: true, data: new Blob(['image']) })
    createObjectURL.mockReturnValueOnce('blob:old').mockReturnValueOnce('blob:new')
    const oldResult = await loadImagePreview('/p/image.png')
    const newResult = await loadImagePreview('/p/image.png')
    oldResult.data?.dispose()
    expect(revokeObjectURL.mock.calls).toEqual([['blob:old']])
    expect(newResult.data?.url).toBe('blob:new')
    newResult.data?.dispose()
    expect(revokeObjectURL.mock.calls).toEqual([['blob:old'], ['blob:new']])
  })

  it('returns transport errors without allocating URLs', async () => {
    const failure = { success: false, error: 'Not found', code: 'NOT_FOUND' }
    readImage.mockResolvedValue(failure)
    expect(await loadImagePreview('/p/missing.png')).toBe(failure)
    expect(createObjectURL).not.toHaveBeenCalled()
  })

  it('returns a controlled error when URL allocation fails', async () => {
    readImage.mockResolvedValue({ success: true, data: new Blob(['image']) })
    createObjectURL.mockImplementation(() => {
      throw new Error('URL unavailable')
    })
    expect(await loadImagePreview('/p/image.png')).toMatchObject({
      success: false,
      code: 'IMAGE_PREVIEW_ERROR',
      error: expect.stringContaining('URL unavailable')
    })
  })
})
