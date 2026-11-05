import type { IpcResult } from '@shared/types/ipc.types'
import { filesystemApi } from './filesystem-api'
import { isTauriContext } from './tauri-runtime'

/** The adapter has already enforced the raster MIME allowlist and 20 MiB cap. */
function readImageDataUrl(blob: Blob, signal?: AbortSignal): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    const cleanup = (): void => {
      signal?.removeEventListener('abort', abort)
      reader.onload = null
      reader.onerror = null
      reader.onabort = null
    }
    const fail = (error: unknown): void => {
      cleanup()
      reject(error)
    }
    const abort = (): void => {
      cleanup()
      reader.abort()
      reject(new DOMException('Image preview cancelled', 'AbortError'))
    }
    reader.onload = (): void => {
      const url = reader.result
      cleanup()
      if (typeof url === 'string') resolve(url)
      else reject(new Error('Image preview returned no data URL'))
    }
    reader.onerror = (): void => fail(reader.error ?? new Error('Image preview encoding failed'))
    reader.onabort = (): void => fail(new DOMException('Image preview cancelled', 'AbortError'))
    signal?.addEventListener('abort', abort, { once: true })
    if (signal?.aborted) {
      abort()
      return
    }
    try {
      reader.readAsDataURL(blob)
    } catch (error) {
      fail(error)
    }
  })
}

export interface ImagePreviewSource {
  /** An owned web object URL or desktop raster data URL; never a remote/asset URL. */
  url: string
  /** Release on replacement, decode error or unmount; safe to call repeatedly. */
  dispose(): void
}

/**
 * Load raster bytes without the text-file pipeline. Each call owns its source:
 * callers dispose stale results and drop their references on replacement/unmount.
 * Cancellation aborts desktop encoding and revokes any resolved web object URL.
 * Desktop data strings are released by dereference/GC rather than URL revocation.
 */
export async function loadImagePreview(
  filePath: string,
  signal?: AbortSignal
): Promise<IpcResult<ImagePreviewSource>> {
  if (signal?.aborted) {
    return { success: false, error: 'Image preview cancelled', code: 'ABORTED' }
  }
  const result = await filesystemApi.readImage(filePath, signal)
  if (!result.success) return result
  if (signal?.aborted) {
    return { success: false, error: 'Image preview cancelled', code: 'ABORTED' }
  }
  if (!result.data) {
    return { success: false, error: 'Image preview returned no data', code: 'IMAGE_READ_ERROR' }
  }

  try {
    // Desktop img-src permits data: but not blob:. Web retains revocable object
    // URLs. Native FileReader preserves the validated MIME without byte guessing.
    const desktop = isTauriContext()
    const url = desktop
      ? await readImageDataUrl(result.data, signal)
      : URL.createObjectURL(result.data)
    let disposed = false
    const dispose = (): void => {
      if (disposed) return
      disposed = true
      signal?.removeEventListener('abort', dispose)
      // Data strings are released when callers drop their source reference.
      if (!desktop) URL.revokeObjectURL(url)
    }
    signal?.addEventListener('abort', dispose, { once: true })
    // Covers a signal that became aborted while the URL was being allocated.
    if (signal?.aborted) {
      dispose()
      return { success: false, error: 'Image preview cancelled', code: 'ABORTED' }
    }
    return { success: true, data: { url, dispose } }
  } catch (error) {
    if (signal?.aborted || (error instanceof DOMException && error.name === 'AbortError')) {
      return { success: false, error: 'Image preview cancelled', code: 'ABORTED' }
    }
    return { success: false, error: String(error), code: 'IMAGE_PREVIEW_ERROR' }
  }
}
