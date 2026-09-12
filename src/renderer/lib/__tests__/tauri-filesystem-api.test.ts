/**
 * Unit tests for tauri-filesystem-api.ts
 * Tests the tauriFilesystemApi implementation using Tauri plugin-fs
 */

import type { FileInfo } from '@tauri-apps/plugin-fs'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// These tests exercise the DESKTOP path (`@tauri-apps/plugin-fs`). The renderer
// facade branches to the web-server client when `!isTauriContext()`, and jsdom
// has no `__TAURI_INTERNALS__` so that branch would fire by default. Pin the
// context to true here so the desktop path is under test; the web branch is
// covered by `web-server-api.test.ts` + `tauri-filesystem-api.web.test.ts`.
const { mockIsTauriContext } = vi.hoisted(() => ({
  mockIsTauriContext: vi.fn(() => true)
}))

vi.mock('../tauri-runtime', () => ({
  isTauriContext: mockIsTauriContext
}))

// The watcher lifecycle no longer goes through the plugin's `watchImmediate`
// wrapper. That wrapper hides the resource id behind a closure whose only
// release path is the synchronous `plugin:resources|close`, which drops the
// `FsEventWatcher` — and joins its FSEvents thread — on the macOS UI thread.
// Creation calls `plugin:fs|watch` for the id; release goes through the app's
// own async `release_fs_watcher`.
const { mockInvoke, watchChannels, ridSeq } = vi.hoisted(() => ({
  mockInvoke: vi.fn(),
  watchChannels: [] as Array<{ onmessage: ((event: unknown) => void) | null }>,
  ridSeq: { next: 1 }
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: mockInvoke,
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null
    constructor() {
      watchChannels.push(this)
    }
  }
}))

function watchCalls(): Array<{ paths: string[] }> {
  return mockInvoke.mock.calls
    .filter((call) => call[0] === 'plugin:fs|watch')
    .map((call) => call[1] as { paths: string[] })
}

function releasedRids(): number[] {
  return mockInvoke.mock.calls
    .filter((call) => call[0] === 'release_fs_watcher')
    .map((call) => (call[1] as { rid: number }).rid)
}

/** Feed an event through the live watcher's channel. */
function emitWatchEvent(event: unknown): void {
  watchChannels.at(-1)?.onmessage?.(event)
}

const defaultStat: FileInfo = {
  isFile: true,
  isDirectory: false,
  isSymlink: false,
  size: 1024,
  mtime: new Date(),
  atime: null,
  birthtime: null,
  readonly: false,
  fileAttributes: null,
  dev: null,
  ino: null,
  mode: null,
  nlink: null,
  uid: null,
  gid: null,
  rdev: null,
  blksize: null,
  blocks: null
}

// Mock @tauri-apps/plugin-fs BEFORE importing
vi.mock('@tauri-apps/plugin-fs', () => ({
  open: vi.fn(async () => ({
    read: vi.fn(async () => 0),
    close: vi.fn(async () => {})
  })),
  readDir: vi.fn(async () => []),
  readFile: vi.fn(async () => new Uint8Array()),
  readTextFile: vi.fn(async () => ''),
  writeTextFile: vi.fn(async () => {}),
  mkdir: vi.fn(async () => {}),
  remove: vi.fn(async () => {}),
  rename: vi.fn(async () => {}),
  copyFile: vi.fn(async () => {}),
  stat: vi.fn(async () => defaultStat)
}))

import type { DirEntry } from '@tauri-apps/plugin-fs'
import {
  copyFile,
  mkdir,
  open,
  readDir,
  readFile,
  readTextFile,
  remove,
  rename,
  stat,
  writeTextFile
} from '@tauri-apps/plugin-fs'
import { i18n } from '../../i18n'
import {
  _liveWatcherCountForTesting,
  _resetFilesystemStateForTesting,
  MAX_FILE_SIZE,
  tauriFilesystemApi
} from '../tauri-filesystem-api'

function makeFileInfo(overrides: Partial<FileInfo>): FileInfo {
  return {
    ...defaultStat,
    ...overrides
  }
}

describe('tauriFilesystemApi', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en')
    vi.clearAllMocks()
    _resetFilesystemStateForTesting()

    // Restore default mocks after clearing
    vi.mocked(open).mockResolvedValue({
      read: vi.fn(async () => 0),
      close: vi.fn(async () => {})
    } as never)
    vi.mocked(readDir).mockResolvedValue([])
    vi.mocked(readFile).mockResolvedValue(new Uint8Array())
    vi.mocked(readTextFile).mockResolvedValue('')
    vi.mocked(writeTextFile).mockResolvedValue(undefined)
    vi.mocked(mkdir).mockResolvedValue(undefined)
    vi.mocked(remove).mockResolvedValue(undefined)
    vi.mocked(rename).mockResolvedValue(undefined)
    vi.mocked(copyFile).mockResolvedValue(undefined)
    vi.mocked(stat).mockResolvedValue(defaultStat)
    watchChannels.length = 0
    ridSeq.next = 1
    mockInvoke.mockImplementation(async (command: string) =>
      command === 'plugin:fs|watch' ? ridSeq.next++ : undefined
    )
  })

  afterEach(() => {
    vi.clearAllMocks()
  })

  describe('readDirectory', () => {
    it('should successfully read directory entries with folders-first sorting', async () => {
      const mockEntries: DirEntry[] = [
        {
          name: 'file1.txt',
          isDirectory: false,
          isFile: true,
          isSymlink: false
        },
        { name: 'dir1', isDirectory: true, isFile: false, isSymlink: false }
      ]
      vi.mocked(readDir).mockResolvedValue(mockEntries)

      const result = await tauriFilesystemApi.readDirectory('/test')

      expect(result.success).toBe(true)
      if (result.success) {
        expect(result.data).toHaveLength(2)
        // Directories come first with folders-first sorting
        expect(result.data![0].name).toBe('dir1')
        expect(result.data![0].type).toBe('directory')
        expect(result.data![1].name).toBe('file1.txt')
        expect(result.data![1].type).toBe('file')
      }
      // Patch H (byte-identical desktop guarantee): the desktop branch must
      // actually call the @tauri-apps/plugin-fs `readDir` mock — not silently
      // fall through to a web/server client. The mock fires once with the
      // raw directory path.
      expect(vi.mocked(readDir)).toHaveBeenCalledTimes(1)
      expect(vi.mocked(readDir)).toHaveBeenCalledWith('/test')
    })

    it('should flag ALWAYS_IGNORE patterns as ignored but still include them', async () => {
      const mockEntries: DirEntry[] = [
        {
          name: 'file1.txt',
          isDirectory: false,
          isFile: true,
          isSymlink: false
        },
        { name: 'src', isDirectory: true, isFile: false, isSymlink: false },
        {
          name: 'node_modules',
          isDirectory: true,
          isFile: false,
          isSymlink: false
        },
        { name: '.git', isDirectory: true, isFile: false, isSymlink: false },
        { name: 'dist', isDirectory: true, isFile: false, isSymlink: false }
      ]
      vi.mocked(readDir).mockResolvedValue(mockEntries)

      const result = await tauriFilesystemApi.readDirectory('/test')

      expect(result.success).toBe(true)
      if (result.success) {
        expect(result.data).toHaveLength(5)
        const byName = Object.fromEntries(result.data!.map((e) => [e.name, e]))
        // Ignored entries are flagged, not removed
        expect(byName.node_modules.ignored).toBe(true)
        expect(byName['.git'].ignored).toBe(true)
        expect(byName.dist.ignored).toBe(true)
        // Non-ignored entries are not flagged
        expect(byName.src.ignored).toBe(false)
        expect(byName['file1.txt'].ignored).toBe(false)
        // Non-ignored directory sorts before ignored directories
        expect(result.data![0].name).toBe('src')
        // Ignored directories still precede files
        expect(result.data!.map((e) => e.name)).toEqual([
          'src',
          '.git',
          'dist',
          'node_modules',
          'file1.txt'
        ])
      }
    })

    it('should handle errors', async () => {
      vi.mocked(readDir).mockRejectedValue(new Error('Permission denied'))

      const result = await tauriFilesystemApi.readDirectory('/test')

      expect(result.success).toBe(false)
      if (!result.success) {
        expect(result.code).toBe('READ_DIR_ERROR')
      }
    })
  })

  describe('readFile', () => {
    it('should successfully read file content', async () => {
      const testDate = new Date('2024-01-01')
      vi.mocked(readTextFile).mockResolvedValue('Hello, World!')
      vi.mocked(stat).mockResolvedValue(makeFileInfo({ size: 13, mtime: testDate }))

      const result = await tauriFilesystemApi.readFile('/test/file.txt')

      expect(result.success).toBe(true)
      if (result.success) {
        expect(result.data.content).toBe('Hello, World!')
        expect(result.data.size).toBe(13)
        expect(result.data.encoding).toBe('utf-8')
      }
    })

    /**
     * Pins the ceiling shared with the server.
     *
     * `src-tauri/src/web/fs_api.rs` declares the same value for web/remote
     * clients and nothing links the two, so a change on either side silently
     * lets one surface open a file the other refuses.
     */
    it('caps the editor at the size the server also enforces', () => {
      expect(MAX_FILE_SIZE).toBe(3 * 1024 * 1024)
      // Also update MAX_FILE_SIZE in src-tauri/src/web/fs_api.rs.
    })

    it('should reject files larger than MAX_FILE_SIZE', async () => {
      vi.mocked(stat).mockResolvedValue(makeFileInfo({ size: MAX_FILE_SIZE + 1 }))

      const result = await tauriFilesystemApi.readFile('/test/large.bin')

      expect(result.success).toBe(false)
      if (!result.success) {
        expect(result.code).toBe('FILE_TOO_LARGE')
        expect(result.error).toBe(
          `File too large (${MAX_FILE_SIZE + 1} bytes, max ${MAX_FILE_SIZE})`
        )
      }
      expect(readTextFile).not.toHaveBeenCalled()
    })

    it('reads a file that sits just under the cap', async () => {
      // The boundary matters in both directions: a cap raised in name only
      // would still refuse the files it was raised for.
      vi.mocked(stat).mockResolvedValue(makeFileInfo({ size: MAX_FILE_SIZE }))
      vi.mocked(readTextFile).mockResolvedValue('under the cap')

      const result = await tauriFilesystemApi.readFile('/test/big-but-ok.txt')

      expect(result.success).toBe(true)
    })

    it('should reject binary files by inspecting read content (no separate sample read)', async () => {
      vi.mocked(stat).mockResolvedValue(makeFileInfo({ size: 11 }))
      // Contains a null byte within the first 512 chars -> isBinaryFile() true
      vi.mocked(readTextFile).mockResolvedValue('Hello\u0000World')

      const result = await tauriFilesystemApi.readFile('/test/binary.bin')

      expect(result.success).toBe(false)
      if (!result.success) {
        expect(result.code).toBe('BINARY_FILE')
        expect(result.error).toBe('Binary file cannot be displayed')
      }
      // Must NOT open a separate file handle for binary sampling
      expect(open).not.toHaveBeenCalled()
    })

    it('should localize file validation errors in Simplified Chinese', async () => {
      await i18n.changeLanguage('zh-CN')
      vi.mocked(stat).mockResolvedValue(makeFileInfo({ size: MAX_FILE_SIZE + 1 }))

      const largeResult = await tauriFilesystemApi.readFile('/test/large.bin')

      expect(largeResult.success).toBe(false)
      if (!largeResult.success) {
        expect(largeResult.code).toBe('FILE_TOO_LARGE')
        expect(largeResult.error).toBe(
          `文件过大（${MAX_FILE_SIZE + 1} 字节，上限 ${MAX_FILE_SIZE} 字节）`
        )
      }

      vi.mocked(stat).mockResolvedValue(makeFileInfo({ size: 11 }))
      vi.mocked(readTextFile).mockResolvedValue('Hello\u0000World')

      const binaryResult = await tauriFilesystemApi.readFile('/test/binary.bin')

      expect(binaryResult.success).toBe(false)
      if (!binaryResult.success) {
        expect(binaryResult.code).toBe('BINARY_FILE')
        expect(binaryResult.error).toBe('无法显示二进制文件')
      }
    })

    it('should handle errors', async () => {
      vi.mocked(stat).mockRejectedValue(new Error('File not found'))

      const result = await tauriFilesystemApi.readFile('/test/nonexistent.txt')

      expect(result.success).toBe(false)
      if (!result.success) {
        expect(result.code).toBe('READ_ERROR')
      }
    })
  })

  describe('searchContent', () => {
    it('should localize the unavailable search backend error', async () => {
      await i18n.changeLanguage('zh-CN')

      const result = await tauriFilesystemApi.searchContent('/scope', '/root', 'query')

      expect(result.success).toBe(false)
      if (!result.success) {
        expect(result.code).toBe('SEARCH_BACKEND_UNAVAILABLE')
        expect(result.error).toBe('搜索后端不可用（ripgrep 命令执行失败）')
      }
    })
  })

  describe('getFileInfo', () => {
    it('should return file metadata', async () => {
      const testDate = new Date('2024-01-01T00:00:00Z')
      const close = vi.fn(async () => {})
      vi.mocked(stat).mockResolvedValue(makeFileInfo({ size: 2048, mtime: testDate }))
      vi.mocked(open).mockResolvedValue({
        read: vi.fn(async (buffer: Uint8Array) => {
          buffer.set(new TextEncoder().encode('some content'))
          return 12
        }),
        close
      } as never)

      const result = await tauriFilesystemApi.getFileInfo('/test/file.txt')

      expect(result.success).toBe(true)
      if (result.success) {
        expect(result.data.size).toBe(2048)
        expect(result.data.path).toBe('/test/file.txt')
        expect(result.data.type).toBe('file')
        expect(result.data.isReadOnly).toBe(false)
        expect(result.data.isBinary).toBe(false)
      }
      expect(open).toHaveBeenCalledWith('/test/file.txt', { read: true })
      expect(close).toHaveBeenCalled()
      expect(readTextFile).not.toHaveBeenCalled()
    })

    it('should not read directory contents when returning directory metadata', async () => {
      const testDate = new Date('2024-01-01T00:00:00Z')
      vi.mocked(stat).mockResolvedValue(
        makeFileInfo({ isFile: false, isDirectory: true, size: 0, mtime: testDate })
      )

      const result = await tauriFilesystemApi.getFileInfo('/test/folder')

      expect(result.success).toBe(true)
      if (result.success) {
        expect(result.data.type).toBe('directory')
        expect(result.data.isBinary).toBe(false)
      }
      expect(open).not.toHaveBeenCalled()
      expect(readTextFile).not.toHaveBeenCalled()
    })

    it('should detect binary files from a bounded byte sample', async () => {
      const testDate = new Date('2024-01-01T00:00:00Z')
      const close = vi.fn(async () => {})
      vi.mocked(stat).mockResolvedValue(makeFileInfo({ size: 10, mtime: testDate }))
      vi.mocked(open).mockResolvedValue({
        read: vi.fn(async (buffer: Uint8Array) => {
          buffer.set(new Uint8Array([72, 101, 108, 108, 111, 0, 87, 111, 114, 108, 100]))
          return 11
        }),
        close
      } as never)

      const result = await tauriFilesystemApi.getFileInfo('/test/binary.bin')

      expect(result.success).toBe(true)
      if (result.success) {
        expect(result.data.isBinary).toBe(true)
      }
      expect(open).toHaveBeenCalledWith('/test/binary.bin', { read: true })
      expect(close).toHaveBeenCalled()
      expect(readTextFile).not.toHaveBeenCalled()
    })

    it('should handle errors', async () => {
      vi.mocked(stat).mockRejectedValue(new Error('Stat failed'))

      const result = await tauriFilesystemApi.getFileInfo('/test/file.txt')

      expect(result.success).toBe(false)
      if (!result.success) {
        expect(result.code).toBe('STAT_ERROR')
      }
    })
  })

  describe('writeFile', () => {
    it('should successfully write file', async () => {
      const result = await tauriFilesystemApi.writeFile('/test/file.txt', 'content')

      expect(result.success).toBe(true)
      expect(vi.mocked(writeTextFile)).toHaveBeenCalledWith('/test/file.txt', 'content')
    })

    it('should handle errors', async () => {
      vi.mocked(writeTextFile).mockRejectedValue(new Error('Write failed'))

      const result = await tauriFilesystemApi.writeFile('/test/file.txt', 'content')

      expect(result.success).toBe(false)
      if (!result.success) {
        expect(result.code).toBe('WRITE_ERROR')
      }
    })
  })

  describe('createFile', () => {
    it('should successfully create new file', async () => {
      const result = await tauriFilesystemApi.createFile('/test/new.txt', 'content')

      expect(result.success).toBe(true)
      expect(vi.mocked(writeTextFile)).toHaveBeenCalledWith('/test/new.txt', 'content')
    })

    it('should handle errors', async () => {
      vi.mocked(writeTextFile).mockRejectedValue(new Error('Create failed'))

      const result = await tauriFilesystemApi.createFile('/test/new.txt', 'content')

      expect(result.success).toBe(false)
      if (!result.success) {
        expect(result.code).toBe('CREATE_ERROR')
      }
    })

    // Patch H (byte-identical desktop guarantee): when isTauriContext() is
    // true, createFile MUST call the @tauri-apps/plugin-fs `writeTextFile`
    // mock (not the web/server client).
    it('calls the @tauri-apps/plugin-fs writeTextFile mock on the desktop branch (Patch H)', async () => {
      vi.mocked(writeTextFile).mockClear()
      vi.mocked(writeTextFile).mockResolvedValue(undefined)

      await tauriFilesystemApi.createFile('/test/desktop-create.txt', 'body')

      expect(vi.mocked(writeTextFile)).toHaveBeenCalledTimes(1)
      expect(vi.mocked(writeTextFile)).toHaveBeenCalledWith('/test/desktop-create.txt', 'body')
    })
  })

  describe('createDirectory', () => {
    it('should successfully create directory', async () => {
      const result = await tauriFilesystemApi.createDirectory('/test/new-dir')

      expect(result.success).toBe(true)
      expect(vi.mocked(mkdir)).toHaveBeenCalledWith('/test/new-dir', {
        recursive: true
      })
    })

    it('should handle errors', async () => {
      vi.mocked(mkdir).mockRejectedValue(new Error('Mkdir failed'))

      const result = await tauriFilesystemApi.createDirectory('/test/new-dir')

      expect(result.success).toBe(false)
      if (!result.success) {
        expect(result.code).toBe('MKDIR_ERROR')
      }
    })

    // Patch H (byte-identical desktop guarantee): when isTauriContext() is
    // true, createDirectory MUST call the @tauri-apps/plugin-fs `mkdir` mock
    // (not the web/server client). The three methods moved to the web branch
    // (createDirectory, createFile, readDirectory) must keep their desktop
    // path byte-identical — this pins it.
    it('calls the @tauri-apps/plugin-fs mkdir mock on the desktop branch (Patch H)', async () => {
      vi.mocked(mkdir).mockClear()
      vi.mocked(mkdir).mockResolvedValue(undefined)

      await tauriFilesystemApi.createDirectory('/test/desktop-mkdir')

      expect(vi.mocked(mkdir)).toHaveBeenCalledTimes(1)
      expect(vi.mocked(mkdir)).toHaveBeenCalledWith('/test/desktop-mkdir', {
        recursive: true
      })
    })
  })

  describe('deletePath', () => {
    it('should successfully delete file', async () => {
      const result = await tauriFilesystemApi.deletePath('/test/file.txt')

      expect(result.success).toBe(true)
      expect(vi.mocked(remove)).toHaveBeenCalledWith('/test/file.txt', {
        recursive: false
      })
    })

    it('should recursively delete directory', async () => {
      const result = await tauriFilesystemApi.deletePath('/test/folder', {
        recursive: true
      })

      expect(result.success).toBe(true)
      expect(vi.mocked(remove)).toHaveBeenCalledWith('/test/folder', {
        recursive: true
      })
    })

    it('should handle errors', async () => {
      vi.mocked(remove).mockRejectedValue(new Error('Delete failed'))

      const result = await tauriFilesystemApi.deletePath('/test/file.txt')

      expect(result.success).toBe(false)
      if (!result.success) {
        expect(result.code).toBe('DELETE_ERROR')
      }
    })
  })

  describe('renameFile', () => {
    it('should successfully rename file', async () => {
      const result = await tauriFilesystemApi.renameFile('/test/old.txt', '/test/new.txt')

      expect(result.success).toBe(true)
      expect(vi.mocked(rename)).toHaveBeenCalledWith('/test/old.txt', '/test/new.txt')
    })

    it('should handle errors', async () => {
      vi.mocked(rename).mockRejectedValue(new Error('Rename failed'))

      const result = await tauriFilesystemApi.renameFile('/test/old.txt', '/test/new.txt')

      expect(result.success).toBe(false)
      if (!result.success) {
        expect(result.code).toBe('RENAME_ERROR')
      }
    })
  })

  describe('copyFile', () => {
    it('should successfully copy file', async () => {
      const result = await tauriFilesystemApi.copyFile('/test/src.bin', '/test/dest.bin')

      expect(result.success).toBe(true)
      expect(vi.mocked(copyFile)).toHaveBeenCalledWith('/test/src.bin', '/test/dest.bin')
    })

    it('should handle errors', async () => {
      vi.mocked(copyFile).mockRejectedValue(new Error('Copy failed'))

      const result = await tauriFilesystemApi.copyFile('/test/src.bin', '/test/dest.bin')

      expect(result.success).toBe(false)
      if (!result.success) {
        expect(result.code).toBe('COPY_ERROR')
      }
    })
  })

  describe('watchDirectory', () => {
    it('should successfully watch directory', async () => {
      const result = await tauriFilesystemApi.watchDirectory('/test')

      expect(result.success).toBe(true)
      expect(watchCalls()).toHaveLength(1)
    })

    it('should handle errors', async () => {
      mockInvoke.mockRejectedValueOnce(new Error('Watch failed'))

      const result = await tauriFilesystemApi.watchDirectory('/test')

      expect(result.success).toBe(false)
      if (!result.success) {
        expect(result.code).toBe('WATCH_ERROR')
      }
    })

    it('should return success if already watching', async () => {
      await tauriFilesystemApi.watchDirectory('/test')
      const result = await tauriFilesystemApi.watchDirectory('/test')

      expect(result.success).toBe(true)
      expect(watchCalls()).toHaveLength(1)
    })
  })

  describe('consolidated watcher (one FsEventWatcher for every directory)', () => {
    // Regression cover for the quit hang: Tauri's `cleanup_before_exit` drops
    // the fs plugin's watchers serially on the main thread and notify's
    // `stop()` joins its FSEvents thread with no timeout, so watcher COUNT is
    // what decides whether macOS reports the app as unresponsive.
    it('holds exactly one live watcher no matter how many directories are watched', async () => {
      await tauriFilesystemApi.watchDirectory('/proj/a')
      expect(_liveWatcherCountForTesting()).toBe(1)

      await tauriFilesystemApi.watchDirectory('/proj/b')
      await tauriFilesystemApi.watchDirectory('/proj/c')
      await tauriFilesystemApi.watchDirectory('/proj/d')

      expect(_liveWatcherCountForTesting()).toBe(1)
    })

    it('releases the superseded handle on every rebuild so handles cannot pile up', async () => {
      await tauriFilesystemApi.watchDirectory('/proj/a')
      await tauriFilesystemApi.watchDirectory('/proj/b')
      await tauriFilesystemApi.watchDirectory('/proj/c')

      // Three rebuilds happened, two predecessors must already be gone.
      expect(watchCalls()).toHaveLength(3)
      expect(releasedRids()).toEqual([1, 2])
      expect(_liveWatcherCountForTesting()).toBe(1)
    })

    // Recursive watching would also collapse the watcher count to one, but it
    // buys that with an event flood out of node_modules/.git — trading a quit
    // stall for a runtime one. Consolidation keeps the original semantics.
    it('watches non-recursively', async () => {
      await tauriFilesystemApi.watchDirectory('/proj/a')

      const args = watchCalls().at(-1) as unknown as {
        options: { recursive: boolean }
      }
      expect(args.options.recursive).toBe(false)
    })

    // The release must never be the plugin's own `resources|close`: that command
    // is synchronous, so Tauri drops the FsEventWatcher — and joins its
    // FSEvents thread — on the macOS UI thread. A spindump caught that join
    // freezing the window for 2.31s on an ordinary project switch.
    it('releases through the off-main-thread command, never resources|close', async () => {
      await tauriFilesystemApi.watchDirectory('/proj/a')
      await tauriFilesystemApi.watchDirectory('/proj/b')
      await tauriFilesystemApi.unwatchAllDirectories()

      const commands = mockInvoke.mock.calls.map((call) => call[0])
      expect(commands).toContain('release_fs_watcher')
      expect(commands).not.toContain('plugin:resources|close')
    })

    it('hands the watcher every watched path, using the original OS-native form', async () => {
      await tauriFilesystemApi.watchDirectory('C:\\proj\\a')
      await tauriFilesystemApi.watchDirectory('/proj/b')

      const lastPaths = watchCalls().at(-1)?.paths ?? []
      expect([...lastPaths].sort()).toEqual(['C:\\proj\\a', '/proj/b'].sort())
    })

    it('drops the last watcher entirely when the final directory is unwatched', async () => {
      await tauriFilesystemApi.watchDirectory('/proj/a')
      await tauriFilesystemApi.watchDirectory('/proj/b')
      await tauriFilesystemApi.unwatchDirectory('/proj/a')
      expect(_liveWatcherCountForTesting()).toBe(1)

      await tauriFilesystemApi.unwatchDirectory('/proj/b')
      expect(_liveWatcherCountForTesting()).toBe(0)
    })

    it('stops delivering events for an unwatched directory', async () => {
      await tauriFilesystemApi.watchDirectory('/proj/a')
      await tauriFilesystemApi.watchDirectory('/proj/b')

      const onChanged = vi.fn()
      const cleanup = tauriFilesystemApi.onFileChanged(onChanged)

      emitWatchEvent({ type: { type: 'modify' }, paths: ['/proj/a/x.txt'] })
      expect(onChanged).toHaveBeenCalledTimes(1)

      await tauriFilesystemApi.unwatchDirectory('/proj/a')
      onChanged.mockClear()

      // After unwatching, the rebuilt watcher no longer covers /proj/a. The
      // stale callback is what the OS would stop feeding; assert the registry
      // no longer attributes it either.
      emitWatchEvent({ type: { type: 'modify' }, paths: ['/proj/b/y.txt'] })
      expect(onChanged).toHaveBeenCalledWith({ type: 'change', path: '/proj/b/y.txt' })

      cleanup()
    })

    it('rolls back the registry when the watcher rebuild fails', async () => {
      mockInvoke.mockRejectedValueOnce(new Error('boom'))

      const failed = await tauriFilesystemApi.watchDirectory('/proj/a')
      expect(failed.success).toBe(false)
      expect(_liveWatcherCountForTesting()).toBe(0)

      // A phantom registry entry would make this early-return as "already
      // watching" and leave the directory silently unwatched forever.
      const retried = await tauriFilesystemApi.watchDirectory('/proj/a')
      expect(retried.success).toBe(true)
      expect(_liveWatcherCountForTesting()).toBe(1)
    })

    it('unwatchAllDirectories releases the watcher in one call', async () => {
      await tauriFilesystemApi.watchDirectory('/proj/a')
      await tauriFilesystemApi.watchDirectory('/proj/b')
      await tauriFilesystemApi.watchDirectory('/proj/c')

      const result = await tauriFilesystemApi.unwatchAllDirectories()

      expect(result.success).toBe(true)
      expect(_liveWatcherCountForTesting()).toBe(0)
      // Every watcher ever created is accounted for, the live one included.
      expect(releasedRids()).toEqual([1, 2, 3])
    })

    it('concurrent watch calls still converge on a single watcher', async () => {
      mockInvoke.mockImplementation(async (command: string) => {
        if (command !== 'plugin:fs|watch') return undefined
        // Resolve on a later tick so the calls genuinely overlap.
        await new Promise((resolve) => setTimeout(resolve, 0))
        return ridSeq.next++
      })

      await Promise.all([
        tauriFilesystemApi.watchDirectory('/proj/a'),
        tauriFilesystemApi.watchDirectory('/proj/b'),
        tauriFilesystemApi.watchDirectory('/proj/c')
      ])

      expect(_liveWatcherCountForTesting()).toBe(1)
      // Serialised through one chain — never a watcher per concurrent caller.
      expect(watchCalls().length).toBeLessThanOrEqual(3)
    })
  })

  describe('unwatchDirectory', () => {
    it('should successfully unwatch directory', async () => {
      await tauriFilesystemApi.watchDirectory('/test')
      const result = await tauriFilesystemApi.unwatchDirectory('/test')

      expect(result.success).toBe(true)
      expect(releasedRids()).toEqual([1])
    })

    it('should handle unwatching non-watched directory gracefully', async () => {
      const result = await tauriFilesystemApi.unwatchDirectory('/non-existent')

      expect(result.success).toBe(true)
    })

    it('should handle errors', async () => {
      await tauriFilesystemApi.watchDirectory('/test')
      mockInvoke.mockImplementation(async (command: string) => {
        if (command === 'release_fs_watcher') throw new Error('Unlisten failed')
        return undefined
      })
      const result = await tauriFilesystemApi.unwatchDirectory('/test')

      expect(result.success).toBe(false)
      if (!result.success) {
        expect(result.code).toBe('UNWATCH_ERROR')
      }
    })
  })

  describe('onFileChanged', () => {
    it('should return cleanup function', () => {
      const cleanup = tauriFilesystemApi.onFileChanged(vi.fn())

      expect(typeof cleanup).toBe('function')
    })

    it('should register callback for watched directories', async () => {
      const callback = vi.fn()
      await tauriFilesystemApi.watchDirectory('/test')
      const cleanupFirst = tauriFilesystemApi.onFileChanged(callback)

      const cleanup = tauriFilesystemApi.onFileChanged(callback)
      cleanupFirst()
      cleanup()
    })
  })

  describe('watch event dispatch (typed subscriptions, GH-539)', () => {
    // Emits through the live watcher's channel so tests deliver raw
    // notify-style events exactly as the Tauri plugin would.
    async function watchAndCapture(): Promise<(event: unknown) => void> {
      const result = await tauriFilesystemApi.watchDirectory('/test')
      expect(result.success).toBe(true)
      expect(watchChannels.at(-1)?.onmessage).toBeTypeOf('function')

      return emitWatchEvent
    }

    function emitEvent(kindType: string, path: string): unknown {
      return { type: { type: kindType }, paths: [path] }
    }

    it('routes create events only to onFileCreated subscribers', async () => {
      const emit = await watchAndCapture()
      const onChanged = vi.fn()
      const onCreated = vi.fn()
      const onDeleted = vi.fn()
      const cleanupChanged = tauriFilesystemApi.onFileChanged(onChanged)
      const cleanupCreated = tauriFilesystemApi.onFileCreated(onCreated)
      const cleanupDeleted = tauriFilesystemApi.onFileDeleted(onDeleted)

      emit(emitEvent('create', '/test/new.txt'))

      expect(onCreated).toHaveBeenCalledTimes(1)
      expect(onCreated).toHaveBeenCalledWith({ type: 'add', path: '/test/new.txt' })
      expect(onChanged).not.toHaveBeenCalled()
      expect(onDeleted).not.toHaveBeenCalled()

      cleanupChanged()
      cleanupCreated()
      cleanupDeleted()
    })

    it('routes modify events only to onFileChanged subscribers', async () => {
      const emit = await watchAndCapture()
      const onChanged = vi.fn()
      const onCreated = vi.fn()
      const onDeleted = vi.fn()
      const cleanupChanged = tauriFilesystemApi.onFileChanged(onChanged)
      const cleanupCreated = tauriFilesystemApi.onFileCreated(onCreated)
      const cleanupDeleted = tauriFilesystemApi.onFileDeleted(onDeleted)

      emit(emitEvent('modify', '/test/saved.txt'))

      expect(onChanged).toHaveBeenCalledTimes(1)
      expect(onChanged).toHaveBeenCalledWith({ type: 'change', path: '/test/saved.txt' })
      expect(onCreated).not.toHaveBeenCalled()
      expect(onDeleted).not.toHaveBeenCalled()

      cleanupChanged()
      cleanupCreated()
      cleanupDeleted()
    })

    it('routes remove events only to onFileDeleted subscribers', async () => {
      const emit = await watchAndCapture()
      const onChanged = vi.fn()
      const onCreated = vi.fn()
      const onDeleted = vi.fn()
      const cleanupChanged = tauriFilesystemApi.onFileChanged(onChanged)
      const cleanupCreated = tauriFilesystemApi.onFileCreated(onCreated)
      const cleanupDeleted = tauriFilesystemApi.onFileDeleted(onDeleted)

      emit(emitEvent('remove', '/test/gone.txt'))

      expect(onDeleted).toHaveBeenCalledTimes(1)
      expect(onDeleted).toHaveBeenCalledWith({ type: 'unlink', path: '/test/gone.txt' })
      expect(onChanged).not.toHaveBeenCalled()
      expect(onCreated).not.toHaveBeenCalled()

      cleanupChanged()
      cleanupCreated()
      cleanupDeleted()
    })

    it('fires a callback subscribed for all types exactly once per event', async () => {
      // Mirrors callers that pass one function to all three subscriptions
      // (e.g. use-composer-mentions cache invalidation).
      const emit = await watchAndCapture()
      const invalidate = vi.fn()
      const cleanups = [
        tauriFilesystemApi.onFileChanged(invalidate),
        tauriFilesystemApi.onFileCreated(invalidate),
        tauriFilesystemApi.onFileDeleted(invalidate)
      ]

      emit(emitEvent('create', '/test/a.txt'))
      emit(emitEvent('modify', '/test/a.txt'))
      emit(emitEvent('remove', '/test/a.txt'))

      expect(invalidate).toHaveBeenCalledTimes(3)
      expect(invalidate.mock.calls.map((call) => call[0].type)).toEqual(['add', 'change', 'unlink'])

      for (const cleanup of cleanups) cleanup()
    })

    it('stops dispatching to a callback after its subscription cleanup runs', async () => {
      const emit = await watchAndCapture()
      const onChanged = vi.fn()
      const cleanupChanged = tauriFilesystemApi.onFileChanged(onChanged)

      emit(emitEvent('modify', '/test/one.txt'))
      expect(onChanged).toHaveBeenCalledTimes(1)

      cleanupChanged()
      emit(emitEvent('modify', '/test/one.txt'))
      emit(emitEvent('create', '/test/two.txt'))
      emit(emitEvent('remove', '/test/one.txt'))

      expect(onChanged).toHaveBeenCalledTimes(1)
    })

    it('keeps other event types when only one subscription is cleaned up', async () => {
      const emit = await watchAndCapture()
      const shared = vi.fn()
      const cleanupChanged = tauriFilesystemApi.onFileChanged(shared)
      const cleanupCreated = tauriFilesystemApi.onFileCreated(shared)

      cleanupChanged()

      emit(emitEvent('modify', '/test/a.txt'))
      emit(emitEvent('create', '/test/b.txt'))

      expect(shared).toHaveBeenCalledTimes(1)
      expect(shared).toHaveBeenCalledWith({ type: 'add', path: '/test/b.txt' })

      cleanupCreated()
    })
  })
})
