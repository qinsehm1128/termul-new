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

// The watcher lifecycle no longer goes through the plugin at all. The plugin's
// `watchImmediate` wrapper hides the resource id behind a closure whose only
// release path is the synchronous `plugin:resources|close`, which drops the
// `FsEventWatcher` — and joins its FSEvents thread — on the macOS UI thread.
// The host owns one watcher now: `fs_watcher_subscribe` installs the sink and
// `fs_watcher_set_roots` replaces the watched set, both async, so no release
// ever runs on a thread the UI needs.
const { mockInvoke, watchChannels } = vi.hoisted(() => ({
  mockInvoke: vi.fn(),
  watchChannels: [] as Array<{ onmessage: ((batch: unknown) => void) | null }>
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

/** Root lists handed to the host, in call order. */
function setRootsCalls(): string[][] {
  return mockInvoke.mock.calls
    .filter((call) => call[0] === 'fs_watcher_set_roots')
    .map((call) => (call[1] as { roots: string[] }).roots)
}

/** Push a coalesced batch down the host channel, the way the service does. */
function emitHostBatch(batch: Array<{ kind: string; path: string }>): void {
  watchChannels.at(-1)?.onmessage?.(batch)
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
  _watchRootsForTesting,
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
    mockInvoke.mockImplementation(async () => undefined)
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

  describe('setWatchRoots (the watched set is roots, not directories)', () => {
    it('hands the host exactly the roots it was given', async () => {
      const result = await tauriFilesystemApi.setWatchRoots(['/proj/a', '/proj/b'])

      expect(result.success).toBe(true)
      expect(setRootsCalls()).toEqual([['/proj/a', '/proj/b']])
    })

    it('subscribes to the host event channel before setting roots', async () => {
      await tauriFilesystemApi.setWatchRoots(['/proj/a'])

      const commands = mockInvoke.mock.calls.map((call) => call[0])
      expect(commands.indexOf('fs_watcher_subscribe')).toBeLessThan(
        commands.indexOf('fs_watcher_set_roots')
      )
    })

    it('subscribes once across many root changes', async () => {
      await tauriFilesystemApi.setWatchRoots(['/proj/a'])
      await tauriFilesystemApi.setWatchRoots(['/proj/b'])
      await tauriFilesystemApi.setWatchRoots(['/proj/c'])

      const subscribes = mockInvoke.mock.calls.filter((call) => call[0] === 'fs_watcher_subscribe')
      expect(subscribes).toHaveLength(1)
    })

    it('skips the round-trip when the root set is unchanged', async () => {
      await tauriFilesystemApi.setWatchRoots(['/proj/b', '/proj/a'])
      // Same set, different order: the host is already watching exactly this.
      await tauriFilesystemApi.setWatchRoots(['/proj/a', '/proj/b'])

      expect(setRootsCalls()).toHaveLength(1)
    })

    it('normalises separators and drops blank roots', async () => {
      await tauriFilesystemApi.setWatchRoots(['C:\\proj\\a', '   ', ''])

      expect(setRootsCalls()).toEqual([['C:/proj/a']])
    })

    it('holds exactly one host watcher no matter how many roots are registered', async () => {
      await tauriFilesystemApi.setWatchRoots(['/proj/a', '/proj/b', '/proj/c', '/proj/d'])

      expect(_liveWatcherCountForTesting()).toBe(1)
    })

    it('restores the cached set when the host rejects, so a retry is not skipped', async () => {
      await tauriFilesystemApi.setWatchRoots(['/proj/a'])
      mockInvoke.mockRejectedValueOnce(new Error('boom'))

      const failed = await tauriFilesystemApi.setWatchRoots(['/proj/b'])
      expect(failed.success).toBe(false)
      // A cached set claiming roots the host never took would make the retry
      // short-circuit as "already in sync" and leave nothing watched.
      expect(_watchRootsForTesting()).toEqual(['/proj/a'])

      const retried = await tauriFilesystemApi.setWatchRoots(['/proj/b'])
      expect(retried.success).toBe(true)
      expect(setRootsCalls().at(-1)).toEqual(['/proj/b'])
    })

    it('reports WEB_UNSUPPORTED on the web client instead of a false success', async () => {
      mockIsTauriContext.mockReturnValue(false)
      const result = await tauriFilesystemApi.setWatchRoots(['/proj/a'])
      mockIsTauriContext.mockReturnValue(true)

      expect(result.success).toBe(false)
      if (!result.success) expect(result.code).toBe('WEB_UNSUPPORTED')
    })

    it('unwatchAllDirectories clears the root set in one call', async () => {
      await tauriFilesystemApi.setWatchRoots(['/proj/a', '/proj/b'])

      const result = await tauriFilesystemApi.unwatchAllDirectories()

      expect(result.success).toBe(true)
      expect(_liveWatcherCountForTesting()).toBe(0)
      expect(setRootsCalls().at(-1)).toEqual([])
    })

    it('serialises concurrent callers into a converged final set', async () => {
      await Promise.all([
        tauriFilesystemApi.setWatchRoots(['/proj/a']),
        tauriFilesystemApi.setWatchRoots(['/proj/a', '/proj/b']),
        tauriFilesystemApi.setWatchRoots(['/proj/a', '/proj/b', '/proj/c'])
      ])

      expect(_watchRootsForTesting()).toEqual(['/proj/a', '/proj/b', '/proj/c'])
      expect(setRootsCalls().at(-1)).toEqual(['/proj/a', '/proj/b', '/proj/c'])
    })

    // A failed call restores the set it replaced. It must restore only over its
    // OWN value: a newer caller can install a set while the failing one is still
    // in flight, and overwriting that stranded the host on the older set while
    // the newer call short-circuited as already-synced — and returned success.
    it('a failed call does not roll back over a newer set', async () => {
      await tauriFilesystemApi.setWatchRoots(['/proj/a'])
      mockInvoke.mockRejectedValueOnce(new Error('boom'))

      const [failed, latest] = await Promise.all([
        tauriFilesystemApi.setWatchRoots(['/proj/b']),
        tauriFilesystemApi.setWatchRoots(['/proj/c'])
      ])

      expect(failed.success).toBe(false)
      expect(latest.success).toBe(true)
      // `/proj/c` is what the caller last asked for, so it is what must be
      // cached and what the host must have been told.
      expect(_watchRootsForTesting()).toEqual(['/proj/c'])
      expect(setRootsCalls().at(-1)).toEqual(['/proj/c'])
    })
  })

  describe('watchDirectory / unwatchDirectory are no longer OS operations', () => {
    // Expanding a folder in the tree used to register an OS watcher, which made
    // the watched set a function of UI state and rebuilt the underlying watcher
    // on every expand and collapse. Recursive root watching already covers every
    // directory inside a root, so these do nothing on desktop — and a no-op is
    // what guarantees a stray caller cannot resurrect the churn.
    it('watchDirectory registers nothing with the host', async () => {
      const result = await tauriFilesystemApi.watchDirectory('/proj/a/src')

      expect(result.success).toBe(true)
      expect(setRootsCalls()).toHaveLength(0)
      expect(_watchRootsForTesting()).toEqual([])
    })

    it('unwatchDirectory releases nothing', async () => {
      await tauriFilesystemApi.setWatchRoots(['/proj/a'])
      mockInvoke.mockClear()

      const result = await tauriFilesystemApi.unwatchDirectory('/proj/a')

      expect(result.success).toBe(true)
      expect(setRootsCalls()).toHaveLength(0)
      // Crucially the root survives: a second owner calling unwatch used to
      // release a root the first still believed it held.
      expect(_watchRootsForTesting()).toEqual(['/proj/a'])
    })

    it('watchDirectory still reports WEB_UNSUPPORTED on the web client', async () => {
      mockIsTauriContext.mockReturnValue(false)
      const result = await tauriFilesystemApi.watchDirectory('/proj/a')
      mockIsTauriContext.mockReturnValue(true)

      expect(result.success).toBe(false)
      if (!result.success) expect(result.code).toBe('WEB_UNSUPPORTED')
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
    // Emits through the host channel so tests deliver batches exactly as the
    // Rust service does.
    async function watchAndCapture(): Promise<(batch: unknown) => void> {
      const result = await tauriFilesystemApi.setWatchRoots(['/test'])
      expect(result.success).toBe(true)
      expect(watchChannels.at(-1)?.onmessage).toBeTypeOf('function')

      return (batch) => emitHostBatch(batch as Array<{ kind: string; path: string }>)
    }

    function emitEvent(kindType: string, path: string): unknown {
      const kind = kindType === 'create' ? 'add' : kindType === 'remove' ? 'unlink' : 'change'
      return [{ kind, path }]
    }

    // A rename arrives from notify as one event carrying both the old and the
    // new path, and the host expands it into one entry per path. Dispatching
    // only the first entry of a batch would lose the other end of it — the
    // renderer used to read `event.paths[0]` and drop the rest.
    it('dispatches every entry in a batch, not just the first', async () => {
      const emit = await watchAndCapture()
      const onDeleted = vi.fn()
      const onCreated = vi.fn()
      const cleanupDeleted = tauriFilesystemApi.onFileDeleted(onDeleted)
      const cleanupCreated = tauriFilesystemApi.onFileCreated(onCreated)

      emit([
        { kind: 'unlink', path: '/test/old-name.ts' },
        { kind: 'add', path: '/test/new-name.ts' }
      ])

      expect(onDeleted).toHaveBeenCalledWith({ type: 'unlink', path: '/test/old-name.ts' })
      expect(onCreated).toHaveBeenCalledWith({ type: 'add', path: '/test/new-name.ts' })

      cleanupDeleted()
      cleanupCreated()
    })

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
