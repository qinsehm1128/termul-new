import type {
  DirectoryEntry,
  FileChangeCallback,
  FileChangeEvent,
  FileContent,
  FileInfo,
  FilesystemApi,
  IpcResult,
  SearchFileHit
} from '@shared/types/ipc.types'
import { Channel, invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import {
  copyFile,
  mkdir,
  open,
  readDir,
  readTextFile,
  remove,
  rename,
  stat,
  writeTextFile
} from '@tauri-apps/plugin-fs'
import { runtimeT } from '../i18n/runtime'
import { sortDirectoryEntries } from './filesystem-sort'
import { logFrontendError } from './log-api'
import { cleanupTauriListener, isTauriContext } from './tauri-runtime'
import { webServerFilesystem } from './web-server-api'

// Names that are commonly git-ignored. Entries matching these are still shown in
// the file tree but rendered dimmed (and skipped during recursive walks for perf).
const ALWAYS_IGNORE = [
  'node_modules',
  '.git',
  '.next',
  '.cache',
  '.turbo',
  'dist',
  'build',
  '.output',
  '.nuxt',
  '.svelte-kit',
  '__pycache__',
  '.pytest_cache',
  'venv',
  '.env',
  'coverage',
  '.nyc_output'
]

/**
 * Largest file the editor will open, in bytes.
 *
 * Must stay equal to `MAX_FILE_SIZE` in `src-tauri/src/web/fs_api.rs`, which
 * enforces the same ceiling for web/remote clients — the two are checked by
 * `refuses_a_file_over_the_shared_size_ceiling` on each side.
 */
export const MAX_FILE_SIZE = 3 * 1024 * 1024
const _SEARCH_MAX_FILES_WITH_MATCHES = 100
const _SEARCH_MAX_MATCHES_PER_FILE = 30

function streamingSearchWebUnsupported(): string {
  return runtimeT(
    'projects',
    'filesystemErrors.streamingSearchWebUnsupported',
    'Streaming search is not available in the web client'
  )
}

async function searchWithRipgrep(
  scopeRoot: string,
  rootPath: string,
  query: string
): Promise<{
  results: Array<{ filePath: string; matches: Array<{ lineNumber: number; lineText: string }> }>
  truncated: boolean
  scannedFiles: number
  failedFiles: number
} | null> {
  try {
    const response = await invoke<{
      success: boolean
      data?: {
        results: Array<{
          filePath: string
          matches: Array<{ lineNumber: number; lineText: string }>
        }>
        truncated: boolean
        scannedFiles: number
        failedFiles: number
      }
    }>('search_content', {
      request: {
        scopeRoot,
        rootPath,
        query
      }
    })

    if (!response?.success || !response.data) {
      return null
    }

    return response.data
  } catch {
    return null
  }
}

/**
 * Watch event types dispatched by the Tauri watcher (mapped from notify kinds).
 * Type filtering is an internal facade detail — the shared `FilesystemApi`
 * contract keeps one `FileChangeCallback` signature per subscription method.
 */
type FileWatchEventType = 'change' | 'add' | 'unlink'

/** Registry of callbacks keyed by the event types they subscribed for. */
type TypedCallbackRegistry = Map<FileChangeCallback, Set<FileWatchEventType>>

function registerTypedCallback(
  registry: TypedCallbackRegistry,
  callback: FileChangeCallback,
  eventType: FileWatchEventType
): void {
  const types = registry.get(callback)
  if (types) {
    types.add(eventType)
  } else {
    registry.set(callback, new Set([eventType]))
  }
}

function unregisterTypedCallback(
  registry: TypedCallbackRegistry,
  callback: FileChangeCallback,
  eventType: FileWatchEventType
): void {
  const types = registry.get(callback)
  if (!types) return
  types.delete(eventType)
  if (types.size === 0) {
    registry.delete(callback)
  }
}

function dispatchTypedEvent(
  registry: TypedCallbackRegistry,
  eventType: FileWatchEventType,
  event: FileChangeEvent
): void {
  registry.forEach((types, callback) => {
    if (types.has(eventType)) {
      callback(event)
    }
  })
}

/** Wire shape of one host-side change. `kind` matches `FileWatchEventType`. */
interface HostFileChangeEvent {
  kind: FileWatchEventType
  path: string
}

/**
 * Callbacks subscribed through `onFileChanged` / `onFileCreated` / `onFileDeleted`.
 *
 * There is exactly one registry. The facade used to keep a second, per-directory
 * one keyed off the watched set, but nothing ever wrote to it — `FilesystemApi`
 * exposes no per-directory subscription — so every event was attributed to a
 * watched directory purely to hand it to an empty map.
 */
const globalCallbacks: TypedCallbackRegistry = new Map()

/**
 * Roots the host watches, normalised.
 *
 * Roots, NOT directories. The host watches each one recursively, so a folder the
 * file explorer expands is already covered and needs no registration of its own.
 * That is the entire point of the model: expanding a folder is a UI operation,
 * not an OS-resource operation. Previously the watched set was the expanded-directory
 * set, which made every expand/collapse rebuild the underlying watcher — and on
 * macOS releasing one joins its FSEvents thread on whichever thread dispatched
 * the IPC, i.e. the UI thread.
 */
let watchRoots: string[] = []
/** Signature of the root set the host was last told about; skips redundant syncs. */
let syncedRootKey = ''
/** Serialises syncs so two concurrent callers cannot race each other to the host. */
let rootSyncChain: Promise<void> = Promise.resolve()
/** Memoised host-channel registration; null until the first successful subscribe. */
let hostSubscription: Promise<void> | null = null

function dispatchHostBatch(batch: readonly HostFileChangeEvent[]): void {
  for (const change of batch) {
    dispatchTypedEvent(globalCallbacks, change.kind, { type: change.kind, path: change.path })
  }
}

/**
 * Register the single channel the host pushes coalesced batches down.
 *
 * Memoised for the app's lifetime: the host keeps one sink, so re-subscribing
 * would only swap it for an identical channel. A failure clears the memo so the
 * next caller retries rather than inheriting a permanently rejected promise.
 */
function ensureHostSubscription(): Promise<void> {
  if (!hostSubscription) {
    const channel = new Channel<HostFileChangeEvent[]>()
    channel.onmessage = dispatchHostBatch
    hostSubscription = invoke<void>('fs_watcher_subscribe', { onEvent: channel }).catch(
      (err: unknown) => {
        hostSubscription = null
        throw err
      }
    )
  }
  return hostSubscription
}

/**
 * Tell the host exactly which roots to watch.
 *
 * Reads `watchRoots` at call time rather than closing over it, so a queued sync
 * carries the latest set instead of whichever intermediate one scheduled it.
 * Child paths are not filtered here — the host drops roots already covered by a
 * parent, which is what lets a stray `watchDirectory` for an expanded subfolder
 * cost nothing.
 */
async function syncWatchRoots(): Promise<void> {
  const desired = [...watchRoots].sort()
  const key = desired.join('\u0000')
  if (key === syncedRootKey) return
  await ensureHostSubscription()
  await invoke('fs_watcher_set_roots', { roots: desired })
  syncedRootKey = key
}

/**
 * Queue a root sync. Each link re-reads `watchRoots`, so a burst of
 * register/unregister calls collapses into one host round-trip.
 */
function scheduleRootSync(): Promise<void> {
  const run = rootSyncChain.then(syncWatchRoots, syncWatchRoots)
  // Keep the chain alive after a rejection; the caller still awaits `run`.
  rootSyncChain = run.catch(() => {})
  return run
}

function shouldIgnore(name: string): boolean {
  return ALWAYS_IGNORE.includes(name)
}

function isBinaryFile(content: string): boolean {
  // Check for null bytes in first 512 chars
  const sample = content.slice(0, 512)
  // biome-ignore lint/suspicious/noControlCharactersInRegex: intentional control-char handling
  return /[\x00-\x08]/.test(sample)
}

async function readBinarySample(filePath: string, byteCount: number): Promise<string> {
  const file = await open(filePath, { read: true })

  try {
    const bytes = new Uint8Array(byteCount)
    const bytesRead = await file.read(bytes)
    if (!bytesRead) {
      return ''
    }

    return new TextDecoder().decode(bytes.subarray(0, bytesRead))
  } finally {
    await file.close()
  }
}

function getExtension(filename: string): string | null {
  const idx = filename.lastIndexOf('.')
  return idx >= 0 ? filename.slice(idx) : null
}

function _includesCaseInsensitive(haystack: string, needle: string): boolean {
  return haystack.toLocaleLowerCase().includes(needle.toLocaleLowerCase())
}

async function _collectFilesRecursively(rootPath: string): Promise<string[]> {
  const files: string[] = []
  const queue: string[] = [rootPath.replace(/\\/g, '/')]

  while (queue.length > 0) {
    const dir = queue.shift()
    if (!dir) continue

    let entries: Awaited<ReturnType<typeof readDir>>
    try {
      entries = await readDir(dir)
    } catch {
      continue
    }

    for (const entry of entries) {
      const name = entry.name
      if (shouldIgnore(name)) continue
      const fullPath = `${dir}/${name}`.replace(/\/+/g, '/')
      if (entry.isDirectory) {
        queue.push(fullPath)
      } else {
        files.push(fullPath)
      }
    }
  }

  return files
}

/**
 * Create a FilesystemApi implementation using Tauri's plugin-fs
 *
 * This adapter uses Tauri's filesystem plugin for direct file operations.
 * It maintains the same interface as the Electron preload script for easy migration.
 */
export function createTauriFilesystemApi(): FilesystemApi {
  return {
    async readDirectory(dirPath: string): Promise<IpcResult<DirectoryEntry[]>> {
      // Web/remote mode: route through the same-origin server (Story: Web/
      // remote project creation). Desktop stays on @tauri-apps/plugin-fs.
      if (!isTauriContext()) {
        // The web server (fs_api.rs `ls`) returns OS-native entry paths — on
        // Windows that is backslash separators. The file-explorer store keys
        // `expandedDirs`/`directoryContents` by normalizePath (`\`→`/`) but
        // FileTreeNode reads them by raw `entry.path`, so backslash paths
        // break subdir expansion at level 2+. Normalize to forward slashes to
        // match the Tauri branch below.
        const result = await webServerFilesystem.readDirectory(dirPath)
        if (result.success) {
          return {
            success: true,
            data: result.data.map((entry) => ({
              ...entry,
              path: entry.path.replace(/\\/g, '/')
            }))
          }
        }
        return result
      }
      try {
        const normalizedDirPath = dirPath.replace(/\\/g, '/')
        const entries = await readDir(dirPath)

        // Stat all entries in parallel instead of sequentially — a directory
        // with N entries previously incurred N sequential IPC round-trips,
        // which dominated tree-expansion latency for large directories (#378).
        const filtered = await Promise.all(
          entries.map(async (entry): Promise<DirectoryEntry> => {
            const name = entry.name
            const fullPath = `${normalizedDirPath}/${name}`.replace(/\/+/g, '/')
            let size = 0
            let modified = Date.now()
            try {
              const info = await stat(fullPath)
              size = info.size
              modified = info.mtime?.getTime() ?? Date.now()
            } catch {
              // Ignore stat errors, use defaults
            }

            const isDir = entry.isDirectory ?? false
            return {
              name,
              path: fullPath,
              type: isDir ? 'directory' : 'file',
              extension: isDir ? null : getExtension(name),
              size,
              modifiedAt: modified,
              ignored: shouldIgnore(name)
            }
          })
        )

        // Sort: directories first, then files, both A-Z
        const sorted = sortDirectoryEntries(filtered)
        return { success: true, data: sorted }
      } catch (err) {
        return { success: false, error: String(err), code: 'READ_DIR_ERROR' }
      }
    },

    async readFile(filePath: string): Promise<IpcResult<FileContent>> {
      // Web/remote mode: route through the same-origin server. The server
      // enforces size + binary checks (FILE_TOO_LARGE / BINARY_FILE) so this
      // is a thin passthrough mirroring the desktop facade's behavior.
      if (!isTauriContext()) {
        return webServerFilesystem.readFile(filePath)
      }
      try {
        const info = await stat(filePath)
        if (info.size > MAX_FILE_SIZE) {
          return {
            success: false,
            error: runtimeT(
              'projects',
              'filesystemErrors.fileTooLarge',
              'File too large ({{size}} bytes, max {{max}})',
              { size: info.size, max: MAX_FILE_SIZE }
            ),
            code: 'FILE_TOO_LARGE'
          }
        }

        const content = await readTextFile(filePath)

        // Binary detection on already-read content: avoids a separate
        // open()/read()/close() round-trip that getFileInfo() used to perform.
        if (isBinaryFile(content)) {
          return {
            success: false,
            error: runtimeT(
              'projects',
              'filesystemErrors.binaryFile',
              'Binary file cannot be displayed'
            ),
            code: 'BINARY_FILE'
          }
        }

        return {
          success: true,
          data: {
            content,
            encoding: 'utf-8',
            size: info.size,
            modifiedAt: info.mtime?.getTime() ?? Date.now()
          }
        }
      } catch (err) {
        return { success: false, error: String(err), code: 'READ_ERROR' }
      }
    },

    async getFileInfo(filePath: string): Promise<IpcResult<FileInfo>> {
      // Web/remote mode: route through the same-origin server (`GET /fs/info`).
      if (!isTauriContext()) {
        return webServerFilesystem.getFileInfo(filePath)
      }
      try {
        const info = await stat(filePath)
        const modifiedAt = info.mtime?.getTime() ?? Date.now()

        if (info.isDirectory) {
          return {
            success: true,
            data: {
              path: filePath,
              size: info.size,
              modifiedAt,
              type: 'directory',
              isReadOnly: false,
              isBinary: false
            }
          }
        }

        const content = await readBinarySample(filePath, 512).catch(() => '')

        return {
          success: true,
          data: {
            path: filePath,
            size: info.size,
            modifiedAt,
            type: 'file',
            isReadOnly: false, // Tauri plugin-fs doesn't expose readonly
            isBinary: isBinaryFile(content)
          }
        }
      } catch (err) {
        return { success: false, error: String(err), code: 'STAT_ERROR' }
      }
    },

    async searchContent(scopeRoot: string, rootPath: string, query: string) {
      const normalizedScopeRoot = scopeRoot.replace(/\\/g, '/')
      const normalizedRootPath = rootPath.replace(/\\/g, '/')
      const trimmedQuery = query.trim()
      if (!trimmedQuery) {
        return {
          success: true,
          data: {
            results: [],
            truncated: false,
            scannedFiles: 0,
            failedFiles: 0
          }
        }
      }

      const ripgrepResult = await searchWithRipgrep(
        normalizedScopeRoot,
        normalizedRootPath,
        trimmedQuery
      )
      if (ripgrepResult) {
        return {
          success: true,
          data: ripgrepResult
        }
      }

      return {
        success: false,
        error: runtimeT(
          'projects',
          'filesystemErrors.searchBackendUnavailable',
          'Search backend unavailable (ripgrep command failed)'
        ),
        code: 'SEARCH_BACKEND_UNAVAILABLE'
      }

      /* fallback disabled intentionally to preserve VSCode-like performance guarantees
			try {
				const allFiles = await collectFilesRecursively(normalizedRootPath);
				const results: Array<{ filePath: string; matches: Array<{ lineNumber: number; lineText: string }> }> = [];
				let truncated = false;
				let scannedFiles = 0;
				let failedFiles = 0;

				for (const filePath of allFiles) {
					if (results.length >= SEARCH_MAX_FILES_WITH_MATCHES) {
						truncated = true;
						break;
					}

					let info;
					try {
						info = await stat(filePath);
					} catch {
						failedFiles += 1;
						continue;
					}

					if (info.isDirectory || info.size > MAX_FILE_SIZE) {
						continue;
					}

					scannedFiles += 1;

					let content = "";
					try {
						content = await readTextFile(filePath);
					} catch {
						failedFiles += 1;
						continue;
					}

					if (isBinaryFile(content)) {
						continue;
					}

					const lines = content.split(/\r?\n/);
					const matches: Array<{ lineNumber: number; lineText: string }> = [];

					for (let i = 0; i < lines.length; i += 1) {
						if (includesCaseInsensitive(lines[i], trimmedQuery)) {
							matches.push({ lineNumber: i + 1, lineText: lines[i] });
							if (matches.length >= SEARCH_MAX_MATCHES_PER_FILE) {
								truncated = true;
								break;
							}
						}
					}

					if (matches.length > 0) {
						results.push({ filePath, matches });
					}
				}

				return {
					success: true,
					data: {
						results,
						truncated,
						scannedFiles,
						failedFiles,
					},
				};
			} catch (err) {
				return {
					success: false,
					error: String(err),
					code: "SEARCH_ERROR",
				};
			}
			*/
    },

    async searchContentStreamStart(
      searchId: string,
      scopeRoot: string,
      rootPath: string,
      query: string
    ) {
      // Web/remote mode: streaming search transport (`/search/ws`) is not yet
      // implemented — return an explicit unsupported result instead of
      // invoking a Tauri-only command that silently fails.
      if (!isTauriContext()) {
        return {
          success: false as const,
          code: 'WEB_UNSUPPORTED',
          error: streamingSearchWebUnsupported()
        }
      }
      try {
        const response = await invoke<{ success: boolean; error?: string; code?: string }>(
          'search_content_stream',
          { request: { searchId, scopeRoot, rootPath, query } }
        )
        if (!response?.success) {
          return {
            success: false as const,
            error:
              response?.error ??
              runtimeT(
                'projects',
                'filesystemErrors.startSearchStream',
                'Failed to start search stream'
              ),
            code: response?.code ?? 'SEARCH_STREAM_ERROR'
          }
        }
        return { success: true as const, data: undefined }
      } catch (err) {
        return { success: false as const, error: String(err), code: 'SEARCH_STREAM_ERROR' }
      }
    },

    async searchContentStreamCancel(searchId: string) {
      if (!isTauriContext()) {
        return {
          success: false as const,
          code: 'WEB_UNSUPPORTED',
          error: streamingSearchWebUnsupported()
        }
      }
      try {
        const response = await invoke<{ success: boolean; error?: string; code?: string }>(
          'search_content_cancel',
          { request: { searchId } }
        )
        if (!response?.success) {
          return {
            success: false as const,
            error:
              response?.error ??
              runtimeT(
                'projects',
                'filesystemErrors.cancelSearchStream',
                'Failed to cancel search stream'
              ),
            code: response?.code ?? 'SEARCH_STREAM_CANCEL_ERROR'
          }
        }
        return { success: true as const, data: undefined }
      } catch (err) {
        return {
          success: false as const,
          error: String(err),
          code: 'SEARCH_STREAM_CANCEL_ERROR'
        }
      }
    },

    onSearchContentBatch(callback) {
      if (!isTauriContext()) return () => {}
      let unlisten: Promise<UnlistenFn> | undefined
      try {
        unlisten = listen<{
          searchId: string
          results: Array<{
            filePath: string
            matches: Array<{ lineNumber: number; lineText: string }>
          }>
          truncated: boolean
        }>('search-content-batch', ({ payload }) => callback(payload))
      } catch {
        return () => {}
      }
      return () => cleanupTauriListener(unlisten)
    },

    async searchFileNamesStreamStart(
      searchId: string,
      scopeRoot: string,
      rootPath: string,
      query: string,
      includeIgnored?: boolean
    ) {
      if (!isTauriContext()) {
        return {
          success: false as const,
          code: 'WEB_UNSUPPORTED',
          error: streamingSearchWebUnsupported()
        }
      }
      try {
        const response = await invoke<{ success: boolean; error?: string; code?: string }>(
          'search_file_names_stream',
          {
            request: {
              searchId,
              scopeRoot,
              rootPath,
              query,
              ...(includeIgnored ? { includeIgnored } : {})
            }
          }
        )
        if (!response?.success) {
          return {
            success: false as const,
            error:
              response?.error ??
              runtimeT(
                'projects',
                'filesystemErrors.startFileNamesStream',
                'Failed to start file names stream'
              ),
            code: response?.code ?? 'SEARCH_FILENAMES_STREAM_ERROR'
          }
        }
        return { success: true as const, data: undefined }
      } catch (err) {
        return {
          success: false as const,
          error: String(err),
          code: 'SEARCH_FILENAMES_STREAM_ERROR'
        }
      }
    },

    async searchFileNamesStreamCancel(searchId: string) {
      if (!isTauriContext()) {
        return {
          success: false as const,
          code: 'WEB_UNSUPPORTED',
          error: streamingSearchWebUnsupported()
        }
      }
      try {
        const response = await invoke<{ success: boolean; error?: string; code?: string }>(
          'search_file_names_cancel',
          { request: { searchId } }
        )
        if (!response?.success) {
          return {
            success: false as const,
            error:
              response?.error ??
              runtimeT(
                'projects',
                'filesystemErrors.cancelFileNamesStream',
                'Failed to cancel file names stream'
              ),
            code: response?.code ?? 'SEARCH_FILENAMES_CANCEL_ERROR'
          }
        }
        return { success: true as const, data: undefined }
      } catch (err) {
        return {
          success: false as const,
          error: String(err),
          code: 'SEARCH_FILENAMES_CANCEL_ERROR'
        }
      }
    },

    onSearchFileNamesBatch(
      callback: (event: { searchId: string; files: SearchFileHit[]; truncated?: boolean }) => void
    ) {
      if (!isTauriContext()) return () => {}
      let unlisten: Promise<UnlistenFn> | undefined
      try {
        unlisten = listen<{ searchId: string; files: SearchFileHit[]; truncated?: boolean }>(
          'search-file-names-batch',
          ({ payload }) => callback(payload)
        )
      } catch {
        return () => {}
      }
      return () => cleanupTauriListener(unlisten)
    },

    onSearchFileNamesDone(
      callback: (event: {
        searchId: string
        truncated: boolean
        totalFiles: number
        code?: string
        error?: string
      }) => void
    ) {
      if (!isTauriContext()) return () => {}
      let unlisten: Promise<UnlistenFn> | undefined
      try {
        unlisten = listen<{
          searchId: string
          truncated: boolean
          totalFiles: number
          code?: string
          error?: string
        }>('search-file-names-done', ({ payload }) => callback(payload))
      } catch {
        return () => {}
      }
      return () => cleanupTauriListener(unlisten)
    },

    onSearchContentDone(callback) {
      if (!isTauriContext()) return () => {}
      let unlisten: Promise<UnlistenFn> | undefined
      try {
        unlisten = listen<{
          searchId: string
          truncated: boolean
          scannedFiles: number
          failedFiles: number
          error?: string
        }>('search-content-done', ({ payload }) => callback(payload))
      } catch {
        return () => {}
      }
      return () => cleanupTauriListener(unlisten)
    },

    async writeFile(filePath: string, content: string): Promise<IpcResult<void>> {
      // Web/remote mode: route through the same-origin server (`POST /fs/write`,
      // which truncates+overwrites — matches desktop `writeTextFile`).
      if (!isTauriContext()) {
        return webServerFilesystem.writeFile(filePath, content)
      }
      try {
        await writeTextFile(filePath, content)
        return { success: true, data: undefined }
      } catch (err) {
        return { success: false, error: String(err), code: 'WRITE_ERROR' }
      }
    },

    async createFile(filePath: string, content = ''): Promise<IpcResult<void>> {
      // Web/remote mode: route through the same-origin server.
      if (!isTauriContext()) {
        return webServerFilesystem.createFile(filePath, content)
      }
      try {
        await writeTextFile(filePath, content)
        return { success: true, data: undefined }
      } catch (err) {
        return { success: false, error: String(err), code: 'CREATE_ERROR' }
      }
    },

    async createDirectory(dirPath: string): Promise<IpcResult<void>> {
      // Web/remote mode: route through the same-origin server.
      if (!isTauriContext()) {
        return webServerFilesystem.createDirectory(dirPath)
      }
      try {
        await mkdir(dirPath, { recursive: true })
        return { success: true, data: undefined }
      } catch (err) {
        return { success: false, error: String(err), code: 'MKDIR_ERROR' }
      }
    },

    async deletePath(path: string, options?: { recursive?: boolean }): Promise<IpcResult<void>> {
      // Web/remote mode: route through the same-origin server.
      if (!isTauriContext()) {
        return webServerFilesystem.deletePath(path, options)
      }
      try {
        await remove(path, { recursive: options?.recursive ?? false })
        return { success: true, data: undefined }
      } catch (err) {
        return { success: false, error: String(err), code: 'DELETE_ERROR' }
      }
    },

    async renameFile(oldPath: string, newPath: string): Promise<IpcResult<void>> {
      // Web/remote mode: route through the same-origin server.
      if (!isTauriContext()) {
        return webServerFilesystem.renameFile(oldPath, newPath)
      }
      try {
        await rename(oldPath, newPath)
        return { success: true, data: undefined }
      } catch (err) {
        return { success: false, error: String(err), code: 'RENAME_ERROR' }
      }
    },

    /**
     * Copy a file to a new path using a binary-safe native copy.
     * Returns `COPY_ERROR` on failure (e.g. when the source is a directory).
     */
    async copyFile(srcPath: string, destPath: string): Promise<IpcResult<void>> {
      // Web/remote mode: route through the same-origin server.
      if (!isTauriContext()) {
        return webServerFilesystem.copyFile(srcPath, destPath)
      }
      try {
        await copyFile(srcPath, destPath)
        return { success: true, data: undefined }
      } catch (err) {
        return { success: false, error: String(err), code: 'COPY_ERROR' }
      }
    },

    /**
     * Replace the watched root set in one call — the primitive this model rests on.
     *
     * Callers declare which roots matter (project roots, a conversation's
     * workspace cwd) and the host watches each one recursively. Directories
     * below a root are already covered and need no registration, which is what
     * makes expanding a folder free.
     *
     * Authoritative: this replaces the set rather than adding to it, so exactly
     * one owner may call it. Two owners writing a shared watch key is what made
     * an unwatch from either of them silently release the other's watch.
     */
    async setWatchRoots(roots: string[]): Promise<IpcResult<void>> {
      // Web/remote mode: server-side watching (notify + WS/SSE event channel) is
      // not implemented. An explicit unsupported result lets callers branch on
      // `code`; the mobile explorer re-fetches on action instead of subscribing.
      if (!isTauriContext()) {
        return {
          success: false,
          code: 'WEB_UNSUPPORTED',
          error: 'Directory watching is not available in the web client'
        }
      }
      const previous = watchRoots
      watchRoots = roots
        .map((root) => root.replace(/\\/g, '/'))
        .filter((root) => root.trim().length > 0)
      try {
        await scheduleRootSync()
        return { success: true, data: undefined }
      } catch (err) {
        // Restore, or the cached set would claim roots the host never took and
        // the next sync would short-circuit as "already in sync".
        watchRoots = previous
        return { success: false, error: String(err), code: 'WATCH_ERROR' }
      }
    },

    /**
     * @deprecated Roots are owned by `setWatchRoots`; this is a no-op on desktop.
     *
     * Kept so the shared `FilesystemApi` contract (and the web client's
     * `WEB_UNSUPPORTED` branch) stay intact. Under recursive root watching every
     * directory inside a root already delivers events, so registering one
     * individually has nothing to do — and making it a no-op is what guarantees
     * a stray call can never resurrect the per-directory watcher churn.
     */
    async watchDirectory(_dirPath: string): Promise<IpcResult<void>> {
      if (!isTauriContext()) {
        return {
          success: false,
          code: 'WEB_UNSUPPORTED',
          error: 'Directory watching is not available in the web client'
        }
      }
      return { success: true, data: undefined }
    },

    /**
     * @deprecated Roots are owned by `setWatchRoots`; this is a no-op on desktop.
     *
     * Releasing a single directory is meaningless under recursive root watching,
     * and it was the mechanism behind two separate defects: a second owner could
     * release a root the first still believed it held, and a collapse whose exit
     * animation never ran left the directory watched forever.
     */
    async unwatchDirectory(_dirPath: string): Promise<IpcResult<void>> {
      return { success: true, data: undefined }
    },

    async unwatchAllDirectories(): Promise<IpcResult<void>> {
      // Called on the close path so the watcher is released while the app can
      // still respond. It does NOT make the host-side release optional: macOS
      // Dock/menu quit arrives as an Apple Event and terminates through
      // `applicationWillTerminate` without ever reaching the renderer, so on
      // that path the watcher is still dropped by Tauri's `cleanup_before_exit`.
      // One watcher is survivable there; the per-directory fleet was not.
      if (!isTauriContext()) {
        return { success: true, data: undefined }
      }
      const previous = watchRoots
      watchRoots = []
      try {
        await scheduleRootSync()
        return { success: true, data: undefined }
      } catch (err) {
        watchRoots = previous
        return { success: false, error: String(err), code: 'UNWATCH_ERROR' }
      }
    },

    onFileChanged(callback: FileChangeCallback): () => void {
      registerTypedCallback(globalCallbacks, callback, 'change')

      // Return cleanup function — removes only the 'change' subscription so
      // callers that registered the same callback for several event types
      // (e.g. onFileChanged + onFileCreated + onFileDeleted) keep the others.
      return () => {
        unregisterTypedCallback(globalCallbacks, callback, 'change')
      }
    },

    onFileCreated(callback: FileChangeCallback): () => void {
      registerTypedCallback(globalCallbacks, callback, 'add')

      return () => {
        unregisterTypedCallback(globalCallbacks, callback, 'add')
      }
    },

    onFileDeleted(callback: FileChangeCallback): () => void {
      registerTypedCallback(globalCallbacks, callback, 'unlink')

      return () => {
        unregisterTypedCallback(globalCallbacks, callback, 'unlink')
      }
    }
  }
}

/**
 * Direct export singleton for convenience (matches api-bridge pattern)
 */
export const tauriFilesystemApi = createTauriFilesystemApi()

/**
 * @internal Testing only - reset module state
 */
export function _resetFilesystemStateForTesting() {
  globalCallbacks.clear()
  watchRoots = []
  syncedRootKey = ''
  rootSyncChain = Promise.resolve()
  hostSubscription = null
}

/**
 * @internal Testing only — how many host watchers this module keeps alive.
 *
 * Stays at 1 no matter how many roots are registered (the host watches them all
 * through one recursive watcher) and at 0 when nothing is watched.
 */
export function _liveWatcherCountForTesting(): number {
  return watchRoots.length === 0 ? 0 : 1
}

/** @internal Testing only — the roots last handed to the host, sorted. */
export function _watchRootsForTesting(): string[] {
  return [...watchRoots].sort()
}
