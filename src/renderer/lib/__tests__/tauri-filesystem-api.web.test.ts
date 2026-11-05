/**
 * Web-branch tests for tauri-filesystem-api.ts.
 *
 * The desktop path (`@tauri-apps/plugin-fs`) is covered by the sibling
 * `tauri-filesystem-api.test.ts` (which pins `isTauriContext()` to true).
 * This file pins it to FALSE and asserts the facade delegates the
 * server-backed methods (`createDirectory`, `createFile`, `writeFile`,
 * `readDirectory`, `readFile`, `getFileInfo`, `deletePath`, `renameFile`,
 * `copyFile`) to `webServerFilesystem` — i.e. the fetch client that hits
 * `/fs/*`. Methods without a server transport (`watchDirectory`, streaming
 * search start/cancel) return an explicit `WEB_UNSUPPORTED` result instead
 * of false success or silent `invoke()` failure.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const { mockFetch, mockIsTauriContext, mockInvoke } = vi.hoisted(() => ({
  mockFetch: vi.fn(),
  mockIsTauriContext: vi.fn(),
  mockInvoke: vi.fn()
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }))

vi.mock('../tauri-runtime', () => ({
  isTauriContext: mockIsTauriContext
}))

// The Tauri plugin-fs is imported by the facade module even in web mode (the
// desktop branch is in the same file). Mock it so the module loads without a
// real Tauri runtime. These are never called in the web branch.
vi.mock('@tauri-apps/plugin-fs', () => ({
  open: vi.fn(),
  readDir: vi.fn(),
  readTextFile: vi.fn(),
  writeTextFile: vi.fn(),
  mkdir: vi.fn(),
  remove: vi.fn(),
  rename: vi.fn(),
  copyFile: vi.fn(),
  stat: vi.fn(),
  watchImmediate: vi.fn()
}))

import { i18n } from '../../i18n'
import { _resetRemoteAccessCredentialForTests } from '../remote-access-credential'
import {
  _resetFilesystemStateForTesting,
  MAX_IMAGE_PREVIEW_SIZE,
  tauriFilesystemApi
} from '../tauri-filesystem-api'

async function blobBytes(blob: Blob): Promise<Uint8Array> {
  const reader = new FileReader()
  return new Promise((resolve, reject) => {
    reader.onload = () => resolve(new Uint8Array(reader.result as ArrayBuffer))
    reader.onerror = () => reject(reader.error)
    reader.readAsArrayBuffer(blob)
  })
}

function jsonResponse(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: status === 200 ? 'OK' : 'Error',
    json: () => Promise.resolve(body)
  } as unknown as Response
}

describe('tauriFilesystemApi (web branch)', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en')
    vi.clearAllMocks()
    _resetFilesystemStateForTesting()
    mockIsTauriContext.mockReturnValue(false)
    mockFetch.mockReset()
    vi.stubGlobal('fetch', mockFetch)
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  describe('binary raster preview transport', () => {
    beforeEach(() => {
      _resetRemoteAccessCredentialForTests()
      window.history.replaceState(null, '', '/#access_token=preview-secret')
      mockInvoke.mockReset()
    })

    afterEach(() => {
      window.history.replaceState(null, '', '/')
      _resetRemoteAccessCredentialForTests()
    })

    it('fetches arbitrary raster bytes with authenticated headers and safely encoded paths', async () => {
      const bytes = new Uint8Array([0, 255, 128, 195, 40, 13, 10])
      const path = '/p/图片 + 50% #tag&.png'
      mockFetch.mockResolvedValue(new Response(bytes, { headers: { 'content-type': 'image/png' } }))
      const controller = new AbortController()
      const result = await tauriFilesystemApi.readImage(path, controller.signal)
      expect(result.success).toBe(true)
      expect(result.data?.type).toBe('image/png')
      expect(await blobBytes(result.data!)).toEqual(bytes)
      const [url, options] = mockFetch.mock.calls[0]
      expect(url).toBe(`${window.location.origin}/fs/image?path=${encodeURIComponent(path)}`)
      expect(url).not.toContain('preview-secret')
      expect(options.headers.get('authorization')).toBe('Bearer preview-secret')
      expect(options.signal).toBe(controller.signal)
      expect(mockInvoke).not.toHaveBeenCalled()
    })

    it.each([
      'jfif',
      'tif',
      'tiff',
      'heic',
      'heif'
    ])('loads %s bytes without text decoding', async (extension) => {
      const mime =
        extension === 'jfif'
          ? 'image/jpeg'
          : extension.startsWith('tif')
            ? 'image/tiff'
            : `image/${extension}`
      mockFetch.mockResolvedValue(
        new Response(new Uint8Array([0, 255]), {
          headers: { 'content-type': mime }
        })
      )
      const result = await tauriFilesystemApi.readImage(`/p/photo.${extension}`)
      expect(result.success).toBe(true)
      expect(result.data?.size).toBe(2)
      expect(result.data?.type).toBe(mime)
    })

    it.each([
      'svg',
      'html',
      'pdf',
      'mp3',
      'mp4',
      'txt'
    ])('does not expand binary transport to %s', async (extension) => {
      expect(await tauriFilesystemApi.readImage(`/p/file.${extension}`)).toMatchObject({
        success: false,
        code: 'UNSUPPORTED_IMAGE_TYPE'
      })
      expect(mockFetch).not.toHaveBeenCalled()
      expect(mockInvoke).not.toHaveBeenCalled()
    })

    it.each([
      [401, 'UNAUTHORIZED'],
      [403, 'ACCESS_DENIED'],
      [404, 'NOT_FOUND'],
      [413, 'FILE_TOO_LARGE'],
      [415, 'UNSUPPORTED_IMAGE_FORMAT']
    ])('preserves typed server errors (HTTP %s)', async (status, code) => {
      mockFetch.mockResolvedValue(
        new Response(JSON.stringify({ success: false, error: 'Unavailable', code }), {
          status: Number(status),
          headers: { 'content-type': 'application/json' }
        })
      )
      expect(await tauriFilesystemApi.readImage('/p/file.png')).toEqual({
        success: false,
        error: 'Unavailable',
        code
      })
    })

    it('returns controlled errors for malformed error responses and network failures', async () => {
      mockFetch.mockResolvedValueOnce(new Response('<html>bad gateway</html>', { status: 502 }))
      expect(await tauriFilesystemApi.readImage('/p/file.png')).toMatchObject({
        success: false,
        code: 'IMAGE_READ_ERROR'
      })
      mockFetch.mockRejectedValueOnce(new Error('offline'))
      expect(await tauriFilesystemApi.readImage('/p/file.png')).toMatchObject({
        success: false,
        code: 'IMAGE_READ_ERROR'
      })
    })

    it.each([
      'text/html',
      'application/json',
      'image/svg+xml'
    ])('rejects %s fallback content', async (mime) => {
      mockFetch.mockResolvedValue(
        new Response('<html>not an image</html>', { headers: { 'content-type': mime } })
      )
      expect(await tauriFilesystemApi.readImage('/p/file.png')).toMatchObject({
        success: false,
        code: 'INVALID_IMAGE_RESPONSE'
      })
    })

    it('refuses oversized responses before reading their bodies', async () => {
      const cancel = vi.fn()
      const getReader = vi.fn()
      mockFetch.mockResolvedValue({
        ok: true,
        headers: new Headers({
          'content-type': 'image/png',
          'content-length': String(MAX_IMAGE_PREVIEW_SIZE + 1)
        }),
        body: { cancel, getReader }
      })
      expect(await tauriFilesystemApi.readImage('/p/file.png')).toMatchObject({
        success: false,
        code: 'FILE_TOO_LARGE'
      })
      expect(cancel).toHaveBeenCalledOnce()
      expect(getReader).not.toHaveBeenCalled()
    })

    it('enforces the cap on chunked responses that omit content-length', async () => {
      const cancel = vi.fn()
      const stream = new ReadableStream({
        start(controller) {
          controller.enqueue(new Uint8Array(MAX_IMAGE_PREVIEW_SIZE + 1))
        },
        cancel
      })
      mockFetch.mockResolvedValue(
        new Response(stream, { headers: { 'content-type': 'image/png' } })
      )
      expect(await tauriFilesystemApi.readImage('/p/file.png')).toMatchObject({
        success: false,
        code: 'FILE_TOO_LARGE'
      })
      expect(cancel).toHaveBeenCalledOnce()
    })

    it('does not begin aborted requests and reports aborted fetches', async () => {
      const controller = new AbortController()
      controller.abort()
      expect(await tauriFilesystemApi.readImage('/p/file.png', controller.signal)).toMatchObject({
        success: false,
        code: 'ABORTED'
      })
      expect(mockFetch).not.toHaveBeenCalled()
      mockFetch.mockRejectedValue(new DOMException('Aborted', 'AbortError'))
      expect(await tauriFilesystemApi.readImage('/p/file.png')).toMatchObject({
        success: false,
        code: 'ABORTED'
      })
    })

    it('uses the bounded raw-byte Tauri command without HTTP or asset scope', async () => {
      mockIsTauriContext.mockReturnValue(true)
      const bytes = new Uint8Array([0, 255, 128])
      mockInvoke.mockResolvedValue(bytes.buffer)
      const result = await tauriFilesystemApi.readImage('/outside-home/photo.png')
      expect(mockInvoke).toHaveBeenCalledExactlyOnceWith('read_image_preview', {
        path: '/outside-home/photo.png'
      })
      expect(mockFetch).not.toHaveBeenCalled()
      expect(await blobBytes(result.data!)).toEqual(bytes)
      expect(result.data?.type).toBe('image/png')
    })

    it('normalizes Tauri platform byte-array fallbacks without stringifying bytes', async () => {
      mockIsTauriContext.mockReturnValue(true)
      mockInvoke.mockResolvedValue([0, 255, 128, 195, 40])
      const result = await tauriFilesystemApi.readImage('/p/file.png')
      expect(result.success).toBe(true)
      expect(await blobBytes(result.data!)).toEqual(new Uint8Array([0, 255, 128, 195, 40]))
    })

    it('preserves structured desktop errors', async () => {
      mockIsTauriContext.mockReturnValue(true)
      mockInvoke.mockRejectedValue({ code: 'NOT_FOUND', error: 'Missing image' })
      expect(await tauriFilesystemApi.readImage('/missing.png')).toEqual({
        success: false,
        code: 'NOT_FOUND',
        error: 'Missing image'
      })
    })

    it('drops late desktop responses after cancellation', async () => {
      mockIsTauriContext.mockReturnValue(true)
      const controller = new AbortController()
      mockInvoke.mockImplementation(async () => {
        controller.abort()
        return new Uint8Array([0, 255]).buffer
      })
      expect(await tauriFilesystemApi.readImage('/p/file.png', controller.signal)).toMatchObject({
        success: false,
        code: 'ABORTED'
      })
    })
  })

  it('createDirectory delegates to webServerFilesystem (/fs/mkdir) when !isTauriContext()', async () => {
    mockFetch.mockResolvedValueOnce(jsonResponse({ success: true }))

    const result = await tauriFilesystemApi.createDirectory('/web/proj')

    expect(result.success).toBe(true)
    expect(mockFetch).toHaveBeenCalledWith(
      `${window.location.origin}/fs/mkdir`,
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({ path: '/web/proj' })
      })
    )
  })

  it('createFile delegates to webServerFilesystem (/fs/write) when !isTauriContext()', async () => {
    mockFetch.mockResolvedValueOnce(jsonResponse({ success: true }))

    const result = await tauriFilesystemApi.createFile('/web/proj/README.md', 'hi')

    expect(result.success).toBe(true)
    expect(mockFetch).toHaveBeenCalledWith(
      `${window.location.origin}/fs/write`,
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({ path: '/web/proj/README.md', content: 'hi' })
      })
    )
  })

  it('copyEntries delegates to webServerFilesystem (/fs/copy-into) when !isTauriContext()', async () => {
    mockFetch.mockResolvedValueOnce(jsonResponse({ success: true, data: ['/web/lib/a copy.ts'] }))

    const result = await tauriFilesystemApi.copyEntries(['/web/lib/a.ts'], '/web/lib')

    expect(result).toEqual({ success: true, data: ['/web/lib/a copy.ts'] })
    expect(mockFetch).toHaveBeenCalledWith(
      `${window.location.origin}/fs/copy-into`,
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({ sources: ['/web/lib/a.ts'], targetDir: '/web/lib' })
      })
    )
  })

  it('readDirectory delegates to webServerFilesystem (/fs/ls) when !isTauriContext()', async () => {
    const entries = [
      { name: 'src', path: '/web/src', type: 'directory', extension: null, size: 0, modifiedAt: 1 }
    ]
    mockFetch.mockResolvedValueOnce(jsonResponse({ success: true, data: entries }))

    const result = await tauriFilesystemApi.readDirectory('/web')

    expect(result.success).toBe(true)
    if (result.success) {
      expect(result.data).toEqual(entries)
    }
    expect(mockFetch).toHaveBeenCalledWith(
      `${window.location.origin}/fs/ls?path=${encodeURIComponent('/web')}`,
      expect.objectContaining({ method: 'GET' })
    )
  })

  it('readDirectory normalizes Windows backslash entry paths to forward slashes (web)', async () => {
    // fs_api.rs `ls` joins paths with PathBuf and returns to_string_lossy() —
    // backslash separators on a Windows server. The file-explorer store keys
    // expandedDirs/directoryContents by normalizePath (`\`→`/`) but
    // FileTreeNode reads by raw entry.path; without this normalization the
    // desktop tree can't expand subdirs at level 2+ on web.
    const entries = [
      {
        name: 'src',
        path: 'C:\\web\\src',
        type: 'directory',
        extension: null,
        size: 0,
        modifiedAt: 1
      },
      {
        name: 'a.txt',
        path: 'C:\\web\\a.txt',
        type: 'file',
        extension: 'txt',
        size: 4,
        modifiedAt: 2
      }
    ]
    mockFetch.mockResolvedValueOnce(jsonResponse({ success: true, data: entries }))

    const result = await tauriFilesystemApi.readDirectory('C:\\web')

    expect(result.success).toBe(true)
    if (result.success) {
      expect(result.data.map((e) => e.path)).toEqual(['C:/web/src', 'C:/web/a.txt'])
    }
    expect(mockFetch).toHaveBeenCalledWith(
      `${window.location.origin}/fs/ls?path=${encodeURIComponent('C:\\web')}`,
      expect.objectContaining({ method: 'GET' })
    )
  })

  it('propagates a server-side failure body (e.g. MKDIR_ERROR) from the web client', async () => {
    mockFetch.mockResolvedValueOnce(
      jsonResponse({ success: false, error: 'permission denied', code: 'MKDIR_ERROR' })
    )

    const result = await tauriFilesystemApi.createDirectory('/bad')

    expect(result.success).toBe(false)
    if (!result.success) {
      expect(result.code).toBe('MKDIR_ERROR')
      expect(result.error).toBe('permission denied')
    }
  })

  it('maps a fetch throw to NETWORK_ERROR through the web branch', async () => {
    mockFetch.mockRejectedValueOnce(new Error('offline'))

    const result = await tauriFilesystemApi.createDirectory('/x')

    expect(result.success).toBe(false)
    if (!result.success) {
      expect(result.code).toBe('NETWORK_ERROR')
    }
  })

  it('getFileInfo delegates to webServerFilesystem (/fs/info) when !isTauriContext()', async () => {
    const fileInfo = {
      path: '/web/file.txt',
      size: 100,
      modifiedAt: 1234567890,
      type: 'file',
      isReadOnly: false,
      isBinary: false
    }
    mockFetch.mockResolvedValueOnce(jsonResponse({ success: true, data: fileInfo }))

    const result = await tauriFilesystemApi.getFileInfo('/web/file.txt')

    expect(result.success).toBe(true)
    if (result.success) {
      expect(result.data).toEqual(fileInfo)
    }
    expect(mockFetch).toHaveBeenCalledWith(
      `${window.location.origin}/fs/info?path=${encodeURIComponent('/web/file.txt')}`,
      expect.objectContaining({ method: 'GET' })
    )
  })

  it('getFileInfo returns directory metadata from /fs/info on web', async () => {
    const fileInfo = {
      path: '/web/dir',
      size: 0,
      modifiedAt: 1234567890,
      type: 'directory',
      isReadOnly: false,
      isBinary: false
    }
    mockFetch.mockResolvedValueOnce(jsonResponse({ success: true, data: fileInfo }))

    const result = await tauriFilesystemApi.getFileInfo('/web/dir')

    expect(result.success).toBe(true)
    if (result.success) {
      expect(result.data.type).toBe('directory')
    }
  })

  it('getFileInfo propagates a missing-path failure (STAT_ERROR) on web', async () => {
    mockFetch.mockResolvedValueOnce(
      jsonResponse({ success: false, error: 'not found', code: 'STAT_ERROR' })
    )

    const result = await tauriFilesystemApi.getFileInfo('/web/missing')

    expect(result.success).toBe(false)
    if (!result.success) {
      expect(result.code).toBe('STAT_ERROR')
    }
  })

  it('writeFile delegates to webServerFilesystem (/fs/write) when !isTauriContext()', async () => {
    mockFetch.mockResolvedValueOnce(jsonResponse({ success: true }))

    const result = await tauriFilesystemApi.writeFile('/web/existing.txt', 'new content')

    expect(result.success).toBe(true)
    expect(mockFetch).toHaveBeenCalledWith(
      `${window.location.origin}/fs/write`,
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({ path: '/web/existing.txt', content: 'new content' })
      })
    )
  })

  it('writeFile creates a new file via /fs/write on web', async () => {
    mockFetch.mockResolvedValueOnce(jsonResponse({ success: true }))

    const result = await tauriFilesystemApi.writeFile('/web/new.txt', 'hello')

    expect(result.success).toBe(true)
    if (result.success) {
      expect(result.data).toBeUndefined()
    }
  })

  it('watchDirectory returns WEB_UNSUPPORTED on web (no false success)', async () => {
    const result = await tauriFilesystemApi.watchDirectory('/web/proj')

    expect(result.success).toBe(false)
    if (!result.success) {
      expect(result.code).toBe('WEB_UNSUPPORTED')
      expect(result.error).toContain('not available')
    }
    // No fetch/invoke should be attempted.
    expect(mockFetch).not.toHaveBeenCalled()
  })

  it('searchContentStreamStart returns WEB_UNSUPPORTED on web (no invoke)', async () => {
    const result = await tauriFilesystemApi.searchContentStreamStart(
      'id1',
      '/scope',
      '/root',
      'query'
    )

    expect(result.success).toBe(false)
    if (!result.success) {
      expect(result.code).toBe('WEB_UNSUPPORTED')
      expect(result.error).toBe('Streaming search is not available in the web client')
    }
    expect(mockFetch).not.toHaveBeenCalled()
  })

  it('localizes unsupported streaming search errors in Simplified Chinese', async () => {
    await i18n.changeLanguage('zh-CN')

    const results = await Promise.all([
      tauriFilesystemApi.searchContentStreamStart('id1', '/scope', '/root', 'query'),
      tauriFilesystemApi.searchContentStreamCancel('id1'),
      tauriFilesystemApi.searchFileNamesStreamStart('id2', '/scope', '/root', 'query'),
      tauriFilesystemApi.searchFileNamesStreamCancel('id2')
    ])

    for (const result of results) {
      expect(result.success).toBe(false)
      if (!result.success) {
        expect(result.code).toBe('WEB_UNSUPPORTED')
        expect(result.error).toBe('网页客户端暂不支持流式搜索')
      }
    }
    expect(mockFetch).not.toHaveBeenCalled()
  })

  it('searchContentStreamCancel returns WEB_UNSUPPORTED on web', async () => {
    const result = await tauriFilesystemApi.searchContentStreamCancel('id1')

    expect(result.success).toBe(false)
    if (!result.success) {
      expect(result.code).toBe('WEB_UNSUPPORTED')
    }
  })

  it('searchFileNamesStreamStart returns WEB_UNSUPPORTED on web (no invoke)', async () => {
    const result = await tauriFilesystemApi.searchFileNamesStreamStart(
      'id2',
      '/scope',
      '/root',
      'query'
    )

    expect(result.success).toBe(false)
    if (!result.success) {
      expect(result.code).toBe('WEB_UNSUPPORTED')
    }
    expect(mockFetch).not.toHaveBeenCalled()
  })

  it('searchFileNamesStreamCancel returns WEB_UNSUPPORTED on web', async () => {
    const result = await tauriFilesystemApi.searchFileNamesStreamCancel('id2')

    expect(result.success).toBe(false)
    if (!result.success) {
      expect(result.code).toBe('WEB_UNSUPPORTED')
    }
  })
})
