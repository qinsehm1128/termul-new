import type { DirectoryEntry, FileSearchResult } from '@shared/types/filesystem.types'
import { create } from 'zustand'
import { useShallow } from 'zustand/shallow'
import { runtimeT } from '@/i18n/runtime'
import { filesystemApi } from '@/lib/api'

function normalizePath(p: string): string {
  return p.replace(/\\/g, '/')
}

function isPathWithinRoot(path: string, rootPath: string): boolean {
  return path === rootPath || path.startsWith(`${rootPath}/`)
}

function parentDirectoryOf(path: string): string {
  const lastSlash = path.lastIndexOf('/')
  return lastSlash > 0 ? path.slice(0, lastSlash) : ''
}

function basenameOf(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1)
}

/**
 * Drops that would destroy the moved entry.
 *
 * A directory dragged onto itself or onto anything beneath itself has no
 * meaningful destination — `rename` would either no-op or recurse the tree
 * into its own child. Also rejects a move whose destination equals its current
 * parent, which is a no-op the user almost certainly triggered by accident.
 */
export function isRejectedMove(sourcePath: string, targetDirectory: string): boolean {
  const source = normalizePath(sourcePath)
  const target = normalizePath(targetDirectory)
  if (source === target) return true
  if (isPathWithinRoot(target, source)) return true
  return parentDirectoryOf(source) === target
}

/**
 * Copy a file or directory to a new location.
 *
 * Uses binary-safe `copyFile` to avoid UTF-8 round-trip corruption on binary
 * files (images, fonts, compiled artifacts). When `copyFile` fails, verifies
 * the source is actually a directory before creating one at the destination —
 * this avoids masking real failures (permissions, missing source, disk full)
 * behind an empty directory. Note: recursive directory copy is not yet
 * supported; only an empty directory is created.
 */
async function copyPath(srcPath: string, destPath: string): Promise<void> {
  const result = await filesystemApi.copyFile(srcPath, destPath)
  if (!result.success) {
    // copyFile fails on directories — confirm the source is actually a
    // directory before creating one, so we don't mask real copy failures.
    const info = await filesystemApi.getFileInfo(srcPath)
    if (info.success && info.data.type === 'directory') {
      await filesystemApi.createDirectory(destPath)
    }
  }
}

export interface FileExplorerRootError {
  message: string
  code?: string
}

/** One registered project root rendered inside a multi-root project group. */
export interface FileExplorerRoot {
  projectId: string
  name: string
  path: string
}

export interface FileClipboard {
  type: 'copy' | 'cut'
  paths: string[]
}

interface PendingDirectoryCollapse {
  contentPathsToRemove: string[]
  dirsToUnwatch: string[]
}

/** Worktree root override - when set, explorer roots at worktree path instead of project root */
export type WorktreeRootOverride = string | null

export interface FileExplorerState {
  /** Ordered roots in the active project or project group. */
  roots: FileExplorerRoot[]
  /** Focused root used by root-relative actions and as the terminal fallback. */
  rootPath: string | null
  /** Trusted project boundary for search IPC validation */
  scopeRoot: string | null
  /** Active worktree root override */
  worktreeRoot: string | null
  expandedDirs: Set<string>
  directoryContents: Map<string, DirectoryEntry[]>
  selectedPaths: Set<string>
  lastClickedPath: string | null
  clipboard: FileClipboard | null
  isVisible: boolean
  loadingDirs: Set<string>
  rootLoadError: FileExplorerRootError | null
  /** Per-root failures; `rootLoadError` mirrors the focused root for compatibility. */
  rootLoadErrors: Map<string, FileExplorerRootError>
  searchQuery: string
  searchResults: FileSearchResult[]
  searchFileNameMatches: string[] | null
  searchLoading: boolean
  searchError: string | null
  /**
   * Programmatic error code paired with `searchError`. Set when the
   * streaming done event carries a `code`; otherwise `null`. Values come
   * from `FilesystemApi`'s `onSearchContentDone` / `onSearchFileNamesDone`
   * event types (e.g. `QUERY_TOO_LONG`, `RG_STREAM_FAILED`).
   */
  searchErrorCode: string | null
  searchTruncated: boolean
  searchScannedFiles: number
  searchFailedFiles: number
  searchRequestId: number
  searchLastCompletedQuery: string
  /** Deferred directory cleanup while collapse exit animation runs */
  pendingCollapses: Map<string, PendingDirectoryCollapse>
  /** Skip tree motion (e.g. collapse all) */
  suppressTreeAnimations: boolean
  /** True while a header-initiated refreshTree pass is in flight (GH-540). */
  refreshingTree: boolean

  setRootPath: (path: string | null) => void
  setRoots: (roots: FileExplorerRoot[], focusedRootPath?: string | null) => void
  setFocusedRoot: (path: string) => void
  setWorktreeRoot: (path: string | null) => void
  toggleDirectory: (path: string) => Promise<void>
  finalizeDirectoryCollapse: (path: string) => void
  refreshDirectory: (path: string) => Promise<void>
  /** Re-read the root and every expanded directory (GH-540 header Refresh). */
  refreshTree: () => Promise<void>
  selectPath: (path: string | null) => void
  togglePathSelection: (path: string) => void
  selectPathRange: (fromPath: string, toPath: string) => void
  selectAll: () => void
  clearSelection: () => void
  copySelected: () => void
  cutSelected: () => void
  paste: (destinationPath: string) => Promise<void>
  /** Move entries into a directory, as a drag-and-drop drop does. */
  moveEntries: (sourcePaths: string[], targetDirectory: string) => Promise<void>
  /**
   * Paths currently being dragged inside the tree.
   *
   * Held in the store because `dragover` cannot read `dataTransfer` contents
   * (the spec puts it in protected mode), and deciding whether a row is a
   * legal drop target needs the payload, not just its MIME type.
   */
  dragPaths: string[]
  beginEntryDrag: (paths: string[]) => void
  endEntryDrag: () => void
  duplicateSelected: () => Promise<void>
  toggleVisibility: () => void
  collapseAll: () => void
  setDirectoryContents: (path: string, entries: DirectoryEntry[]) => void
  removeDirectoryContents: (path: string) => void
  setVisible: (visible: boolean) => void
  setExpandedDirs: (dirs: Set<string>) => void
  setRootLoadError: (error: FileExplorerRootError | null) => void
  restoreExpandedDirs: (dirs: string[]) => Promise<void>
  setSearchQuery: (query: string) => void
  searchInRoot: (query: string, requestId: number) => Promise<void>
  resetSearch: () => void
}

let streamSubscribed = false
let fileNameStreamSubscribed = false
let activeSearchRequestId = 0
const activeContentSearchIds = new Set<string>()
const activeFileNameSearchIds = new Set<string>()

function cancelActiveSearchStreams(): void {
  for (const searchId of activeContentSearchIds) {
    filesystemApi.searchContentStreamCancel(searchId).catch((error) => {
      console.warn(`[file-explorer] searchContentStreamCancel(${searchId}) failed:`, error)
    })
  }
  for (const searchId of activeFileNameSearchIds) {
    filesystemApi.searchFileNamesStreamCancel(searchId).catch((error) => {
      console.warn(`[file-explorer] searchFileNamesStreamCancel(${searchId}) failed:`, error)
    })
  }
  activeContentSearchIds.clear()
  activeFileNameSearchIds.clear()
}

function isActiveSearchEvent(searchId: string, requestId: number): boolean {
  const prefix = `search-${requestId}`
  return searchId === prefix || searchId.startsWith(`${prefix}:`)
}

function ensureSearchStreamSubscription(
  set: (partial: Partial<FileExplorerState>) => void,
  get: () => FileExplorerState
): void {
  if (streamSubscribed) return
  streamSubscribed = true

  filesystemApi.onSearchContentBatch((event) => {
    const state = get()
    if (!isActiveSearchEvent(event.searchId, state.searchRequestId)) return

    const merged = new Map(state.searchResults.map((file) => [file.filePath, file]))
    for (const file of event.results) merged.set(file.filePath, file)

    set({
      searchResults: Array.from(merged.values()),
      searchTruncated: event.truncated || state.searchTruncated
    })
  })

  filesystemApi.onSearchContentDone((event) => {
    const state = get()
    if (!isActiveSearchEvent(event.searchId, state.searchRequestId)) return
    activeContentSearchIds.delete(event.searchId)
    const allDone = activeContentSearchIds.size === 0 && activeFileNameSearchIds.size === 0

    set({
      searchLoading: !allDone,
      searchError: event.error ?? state.searchError,
      // Surface the programmatic code so consumers (telemetry, future UI
      // affordances) can distinguish QUERY_TOO_LONG from RG_STREAM_FAILED
      // without parsing the human-readable error string.
      searchErrorCode: event.code ?? null,
      searchTruncated: event.truncated || state.searchTruncated,
      searchScannedFiles: state.searchScannedFiles + event.scannedFiles,
      searchFailedFiles: state.searchFailedFiles + event.failedFiles,
      searchLastCompletedQuery: activeContentSearchIds.size === 0 ? state.searchQuery.trim() : ''
    })
  })
}

function ensureFileNameStreamSubscription(
  set: (partial: Partial<FileExplorerState>) => void,
  get: () => FileExplorerState
): void {
  if (fileNameStreamSubscribed) return
  fileNameStreamSubscribed = true

  filesystemApi.onSearchFileNamesBatch((event) => {
    const state = get()
    if (!isActiveSearchEvent(event.searchId, state.searchRequestId)) return
    const merged = new Set(state.searchFileNameMatches ?? [])
    for (const file of event.files) merged.add(file.path)

    set({
      searchFileNameMatches: Array.from(merged),
      searchTruncated: event.truncated || state.searchTruncated
    })
  })

  filesystemApi.onSearchFileNamesDone((event) => {
    const state = get()
    if (!isActiveSearchEvent(event.searchId, state.searchRequestId)) return
    activeFileNameSearchIds.delete(event.searchId)
    const allDone = activeContentSearchIds.size === 0 && activeFileNameSearchIds.size === 0

    // Surface backend errors and stop the spinner. Filename and content
    // streams share the same `searchError` / `searchLoading` slot; the
    // content stream's own done handler is the source of truth for
    // `searchLastCompletedQuery` and overwrites these fields when it
    // fires, so mirroring the error here is safe.
    const next: Partial<FileExplorerState> = {
      searchTruncated: event.truncated || state.searchTruncated,
      searchLoading: !allDone
    }
    if (event.error) {
      next.searchError = event.error
    }
    // Surface the programmatic code in the same slot as content search so
    // downstream consumers can branch on QUERY_TOO_LONG vs RG_STREAM_FAILED
    // without parsing the message. If the content stream fires afterwards,
    // its own `code` will overwrite this value (which is correct — that
    // event is the source of truth for the overall search result).
    if (event.code) {
      next.searchErrorCode = event.code
    } else if (!event.error) {
      // No error, no code: clear any stale code from a prior search.
      next.searchErrorCode = null
    }
    if (state.searchFileNameMatches === null) {
      // No batch ever landed (zero matches, no trailing flush). Drop the
      // pending placeholder so the tab shows `0` rather than `…` forever.
      next.searchFileNameMatches = []
    }
    set(next)
  })
}

export const useFileExplorerStore = create<FileExplorerState>((set, get) => ({
  roots: [],
  rootPath: null,
  scopeRoot: null,
  worktreeRoot: null,
  expandedDirs: new Set<string>(),
  directoryContents: new Map<string, DirectoryEntry[]>(),
  selectedPaths: new Set<string>(),
  lastClickedPath: null,
  clipboard: null,
  dragPaths: [],
  isVisible: true,
  loadingDirs: new Set<string>(),
  rootLoadError: null,
  rootLoadErrors: new Map<string, FileExplorerRootError>(),
  searchQuery: '',
  searchResults: [],
  searchFileNameMatches: null,
  searchLoading: false,
  searchError: null,
  searchErrorCode: null,
  searchTruncated: false,
  searchScannedFiles: 0,
  searchFailedFiles: 0,
  searchRequestId: 0,
  searchLastCompletedQuery: '',
  pendingCollapses: new Map<string, PendingDirectoryCollapse>(),
  suppressTreeAnimations: false,
  refreshingTree: false,

  setRootPath: (path: string | null): void => {
    // Unwatch all previously expanded directories
    const { expandedDirs } = get()
    cancelActiveSearchStreams()
    expandedDirs.forEach((dir) => {
      filesystemApi.unwatchDirectory(dir)
    })
    const normalized = path ? normalizePath(path) : null
    set({
      roots: normalized ? [{ projectId: '', name: '', path: normalized }] : [],
      rootPath: normalized,
      scopeRoot: normalized,
      expandedDirs: new Set<string>(),
      directoryContents: new Map<string, DirectoryEntry[]>(),
      selectedPaths: new Set<string>(),
      lastClickedPath: null,
      clipboard: null,
      loadingDirs: new Set<string>(),
      rootLoadError: null,
      rootLoadErrors: new Map<string, FileExplorerRootError>(),
      searchQuery: '',
      searchResults: [],
      searchFileNameMatches: null,
      searchLoading: false,
      searchError: null,
      searchErrorCode: null,
      searchTruncated: false,
      searchScannedFiles: 0,
      searchFailedFiles: 0,
      searchRequestId: 0,
      searchLastCompletedQuery: '',
      pendingCollapses: new Map<string, PendingDirectoryCollapse>(),
      suppressTreeAnimations: false
    })
  },

  setRoots: (roots: FileExplorerRoot[], focusedRootPath?: string | null): void => {
    const state = get()
    cancelActiveSearchStreams()
    state.expandedDirs.forEach((dir) => {
      filesystemApi.unwatchDirectory(dir)
    })

    const seenPaths = new Set<string>()
    const normalizedRoots = roots.flatMap((root) => {
      const path = normalizePath(root.path)
      if (!path || seenPaths.has(path)) return []
      seenPaths.add(path)
      return [{ ...root, path }]
    })
    const requestedFocus = focusedRootPath ? normalizePath(focusedRootPath) : null
    const focusedRoot =
      normalizedRoots.find((root) => root.path === requestedFocus) ?? normalizedRoots[0]
    const rootPath = focusedRoot?.path ?? null

    set({
      roots: normalizedRoots,
      rootPath,
      scopeRoot: rootPath,
      worktreeRoot: null,
      expandedDirs: new Set<string>(),
      directoryContents: new Map<string, DirectoryEntry[]>(),
      selectedPaths: new Set<string>(),
      lastClickedPath: null,
      clipboard: null,
      loadingDirs: new Set<string>(),
      rootLoadError: null,
      rootLoadErrors: new Map<string, FileExplorerRootError>(),
      searchQuery: '',
      searchResults: [],
      searchFileNameMatches: null,
      searchLoading: false,
      searchError: null,
      searchErrorCode: null,
      searchTruncated: false,
      searchScannedFiles: 0,
      searchFailedFiles: 0,
      searchRequestId: 0,
      searchLastCompletedQuery: '',
      pendingCollapses: new Map<string, PendingDirectoryCollapse>(),
      suppressTreeAnimations: false
    })
  },

  setFocusedRoot: (path: string): void => {
    const normalized = normalizePath(path)
    const state = get()
    if (!state.roots.some((root) => root.path === normalized)) return
    set({
      rootPath: normalized,
      scopeRoot: normalized,
      rootLoadError: state.rootLoadErrors.get(normalized) ?? null,
      selectedPaths: new Set<string>(),
      lastClickedPath: null
    })
  },

  setWorktreeRoot: (path: string | null): void => {
    const { scopeRoot } = get()
    cancelActiveSearchStreams()
    const worktreeRoot = path ? normalizePath(path) : null
    set({
      worktreeRoot,
      rootPath: worktreeRoot ?? scopeRoot,
      searchQuery: '',
      searchResults: [],
      searchFileNameMatches: null,
      searchLoading: false,
      searchError: null,
      searchErrorCode: null,
      searchTruncated: false,
      searchScannedFiles: 0,
      searchFailedFiles: 0,
      searchRequestId: 0,
      searchLastCompletedQuery: ''
    })
  },

  toggleDirectory: async (path: string): Promise<void> => {
    const normalized = normalizePath(path)
    const { expandedDirs, loadingDirs, roots, rootPath } = get()
    const isRootLoad =
      roots.some((root) => root.path === normalized) ||
      (roots.length === 0 && rootPath === normalized)

    if (expandedDirs.has(normalized)) {
      // Collapse: update expanded state immediately; defer content cleanup for exit animation
      const newExpanded = new Set(expandedDirs)
      newExpanded.delete(normalized)

      const contentPathsToRemove: string[] = [normalized]
      const dirsToUnwatch: string[] = [normalized]

      const newExpandedFiltered = new Set<string>()
      newExpanded.forEach((dir) => {
        if (!dir.startsWith(`${normalized}/`)) {
          newExpandedFiltered.add(dir)
        } else {
          contentPathsToRemove.push(dir)
          dirsToUnwatch.push(dir)
        }
      })

      const newPending = new Map(get().pendingCollapses)
      newPending.set(normalized, { contentPathsToRemove, dirsToUnwatch })

      set({ expandedDirs: newExpandedFiltered, pendingCollapses: newPending })
    } else {
      // Cancel deferred collapse cleanup when re-expanding before animation finishes
      const pending = get().pendingCollapses
      if (pending.has(normalized)) {
        const newPending = new Map(pending)
        newPending.delete(normalized)
        set({ pendingCollapses: newPending })
      }

      // Prevent duplicate expand work if already loading
      if (loadingDirs.has(normalized)) return

      // Expand - load contents
      const newLoading = new Set(loadingDirs)
      newLoading.add(normalized)
      set({ loadingDirs: newLoading })

      try {
        const result = await filesystemApi.readDirectory(normalized)
        if (result.success) {
          const {
            expandedDirs: currentExpanded,
            directoryContents: currentContents,
            rootLoadErrors
          } = get()
          const newExpanded = new Set(currentExpanded)
          newExpanded.add(normalized)
          const newContents = new Map(currentContents)
          newContents.set(normalized, result.data)
          const nextRootErrors = new Map(rootLoadErrors)
          nextRootErrors.delete(normalized)

          set({
            expandedDirs: newExpanded,
            directoryContents: newContents,
            rootLoadErrors: nextRootErrors,
            rootLoadError: rootPath === normalized ? null : get().rootLoadError
          })

          // Watch this directory for changes (fire-and-forget)
          filesystemApi.watchDirectory(normalized)
        } else if (isRootLoad) {
          const rootError = {
            message: result.error,
            code: result.code
          }
          const nextRootErrors = new Map(get().rootLoadErrors)
          nextRootErrors.set(normalized, rootError)
          set({
            rootLoadErrors: nextRootErrors,
            rootLoadError: rootPath === normalized ? rootError : get().rootLoadError
          })
        }
      } catch (error) {
        if (isRootLoad) {
          const message =
            error instanceof Error
              ? error.message
              : runtimeT(
                  'projects',
                  'filesystemErrors.loadProjectFiles',
                  'Failed to load project files'
                )
          const rootError = { message, code: 'UNKNOWN_ERROR' }
          const nextRootErrors = new Map(get().rootLoadErrors)
          nextRootErrors.set(normalized, rootError)
          set({
            rootLoadErrors: nextRootErrors,
            rootLoadError: rootPath === normalized ? rootError : get().rootLoadError
          })
        }
      } finally {
        const newLoadingDone = new Set(get().loadingDirs)
        newLoadingDone.delete(normalized)
        set({ loadingDirs: newLoadingDone })
      }
    }
  },

  refreshDirectory: async (path: string): Promise<void> => {
    const normalized = normalizePath(path)
    try {
      const result = await filesystemApi.readDirectory(normalized)
      if (result.success) {
        const { directoryContents, rootPath, rootLoadErrors } = get()
        const newContents = new Map(directoryContents)
        newContents.set(normalized, result.data)
        const nextRootErrors = new Map(rootLoadErrors)
        nextRootErrors.delete(normalized)
        set({
          directoryContents: newContents,
          rootLoadErrors: nextRootErrors,
          rootLoadError: rootPath === normalized ? null : get().rootLoadError
        })
      }
    } catch {
      // Silently fail on refresh
    }
  },

  refreshTree: async (): Promise<void> => {
    const { rootPath, roots } = get()
    if (!rootPath) return
    const activeRoots =
      roots.length > 0 ? roots.map((root) => root.path) : rootPath ? [rootPath] : []
    if (activeRoots.length === 0 || get().refreshingTree) return

    set({ refreshingTree: true })
    try {
      const capturedScope = `${rootPath}\u0001${activeRoots.join('\u0000')}`
      const dirsToRefresh = new Set<string>([
        ...activeRoots,
        ...Array.from(get().expandedDirs).filter((dir) =>
          activeRoots.some((root) => isPathWithinRoot(dir, root))
        )
      ])

      // Sequential on purpose: re-check live state before each read so a
      // collapse or project switch that lands mid-refresh is honored instead
      // of being overwritten by stale in-flight results.
      for (const dir of dirsToRefresh) {
        const state = get()
        const currentRoots =
          state.roots.length > 0
            ? state.roots.map((root) => root.path)
            : state.rootPath
              ? [state.rootPath]
              : []
        if (`${state.rootPath ?? ''}\u0001${currentRoots.join('\u0000')}` !== capturedScope) return
        if (!currentRoots.includes(dir) && !state.expandedDirs.has(dir)) continue
        await state.refreshDirectory(dir)
      }
    } finally {
      set({ refreshingTree: false })
    }
  },

  selectPath: (path: string | null): void => {
    set({
      selectedPaths: path ? new Set([normalizePath(path)]) : new Set<string>(),
      lastClickedPath: path ? normalizePath(path) : null
    })
  },

  togglePathSelection: (path: string): void => {
    const normalized = normalizePath(path)
    const { selectedPaths } = get()
    const newSet = new Set(selectedPaths)

    if (newSet.has(normalized)) {
      newSet.delete(normalized)
    } else {
      newSet.add(normalized)
    }

    set({ selectedPaths: newSet, lastClickedPath: normalized })
  },

  selectPathRange: (fromPath: string, toPath: string): void => {
    const normalizedFrom = normalizePath(fromPath)
    const normalizedTo = normalizePath(toPath)
    const { directoryContents, rootPath, roots, expandedDirs } = get()

    // Collect all visible paths in order
    const allPaths: string[] = []

    function collectPaths(dirPath: string): void {
      const contents = directoryContents.get(dirPath)
      if (!contents) return

      for (const entry of contents) {
        allPaths.push(entry.path)
        if (entry.type === 'directory' && expandedDirs.has(entry.path)) {
          collectPaths(entry.path)
        }
      }
    }

    const activeRoots =
      roots.length > 0 ? roots.map((root) => root.path) : rootPath ? [rootPath] : []
    for (const root of activeRoots) {
      collectPaths(root)
    }

    // Find indices
    const fromIndex = allPaths.indexOf(normalizedFrom)
    const toIndex = allPaths.indexOf(normalizedTo)

    if (fromIndex === -1 || toIndex === -1) return

    const start = Math.min(fromIndex, toIndex)
    const end = Math.max(fromIndex, toIndex)

    const newSet = new Set(get().selectedPaths)
    for (let i = start; i <= end; i++) {
      newSet.add(allPaths[i])
    }

    set({ selectedPaths: newSet, lastClickedPath: normalizedTo })
  },

  selectAll: (): void => {
    const { directoryContents, rootPath, roots, expandedDirs } = get()
    const allPaths: string[] = []

    function collectPaths(dirPath: string): void {
      const contents = directoryContents.get(dirPath)
      if (!contents) return

      for (const entry of contents) {
        allPaths.push(entry.path)
        if (entry.type === 'directory' && expandedDirs.has(entry.path)) {
          collectPaths(entry.path)
        }
      }
    }

    const activeRoots =
      roots.length > 0 ? roots.map((root) => root.path) : rootPath ? [rootPath] : []
    for (const root of activeRoots) {
      collectPaths(root)
    }

    set({ selectedPaths: new Set(allPaths) })
  },

  clearSelection: (): void => {
    set({ selectedPaths: new Set<string>(), lastClickedPath: null })
  },

  copySelected: (): void => {
    const { selectedPaths } = get()
    if (selectedPaths.size === 0) return

    set({ clipboard: { type: 'copy', paths: Array.from(selectedPaths) } })
  },

  cutSelected: (): void => {
    const { selectedPaths } = get()
    if (selectedPaths.size === 0) return

    set({ clipboard: { type: 'cut', paths: Array.from(selectedPaths) } })
  },

  paste: async (destinationPath: string): Promise<void> => {
    const { clipboard, refreshDirectory } = get()
    if (!clipboard || clipboard.paths.length === 0) return

    const normalizedDest = normalizePath(destinationPath)
    const isDirectory = await (async () => {
      try {
        const result = await filesystemApi.getFileInfo(normalizedDest)
        return !!(result.success && result.data)
      } catch {
        return false
      }
    })()

    const targetDir = isDirectory
      ? normalizedDest
      : normalizedDest.substring(0, normalizedDest.lastIndexOf('/'))

    for (const srcPath of clipboard.paths) {
      const normalizedSrc = normalizePath(srcPath)
      const fileName = normalizedSrc.substring(normalizedSrc.lastIndexOf('/') + 1)
      const destPath = `${targetDir}/${fileName}`

      if (clipboard.type === 'copy') {
        // Copy file/folder
        await copyPath(normalizedSrc, destPath)
      } else {
        // Move file/folder
        const renameResult = await filesystemApi.renameFile(normalizedSrc, destPath)
        if (!renameResult.success) {
          console.error('Failed to move:', renameResult.error)
        }
      }
    }

    // Clear clipboard after cut operation
    if (clipboard.type === 'cut') {
      set({ clipboard: null })
    }

    await refreshDirectory(targetDir)
  },

  beginEntryDrag: (paths: string[]): void => {
    set({ dragPaths: paths })
  },

  endEntryDrag: (): void => {
    set({ dragPaths: [] })
  },

  moveEntries: async (sourcePaths: string[], targetDirectory: string): Promise<void> => {
    const { refreshDirectory } = get()
    const target = normalizePath(targetDirectory)

    // Both sides are refreshed, unlike cut+paste which only refreshes the
    // destination: a drop leaves a hole in the source directory that the user
    // is still looking at.
    const touched = new Set<string>()
    let moved = false

    for (const sourcePath of sourcePaths) {
      const source = normalizePath(sourcePath)
      if (isRejectedMove(source, target)) continue

      const result = await filesystemApi.renameFile(source, `${target}/${basenameOf(source)}`)
      if (!result.success) {
        // Keep going: one unwritable entry must not strand the rest of a
        // multi-select drop half-moved and unreported.
        console.error('Failed to move:', result.error)
        continue
      }
      moved = true
      touched.add(parentDirectoryOf(source))
    }

    if (!moved) return

    touched.add(target)
    for (const directory of touched) {
      if (directory) await refreshDirectory(directory)
    }
  },

  duplicateSelected: async (): Promise<void> => {
    const { selectedPaths, refreshDirectory } = get()
    if (selectedPaths.size === 0) return

    for (const path of selectedPaths) {
      const normalized = normalizePath(path)
      const lastSlash = normalized.lastIndexOf('/')
      const dir = lastSlash > 0 ? normalized.substring(0, lastSlash) : ''
      const fileName = normalized.substring(lastSlash + 1)

      // Generate duplicate name
      const dotIndex = fileName.lastIndexOf('.')
      const baseName = dotIndex > 0 ? fileName.substring(0, dotIndex) : fileName
      const ext = dotIndex > 0 ? fileName.substring(dotIndex) : ''
      const newName = `${baseName} (copy)${ext}`
      const destPath = `${dir}/${newName}`

      await copyPath(normalized, destPath)

      if (dir) {
        await refreshDirectory(dir)
      }
    }
  },

  toggleVisibility: (): void => {
    set((state) => ({ isVisible: !state.isVisible }))
  },

  finalizeDirectoryCollapse: (path: string): void => {
    const normalized = normalizePath(path)
    const pending = get().pendingCollapses.get(normalized)
    if (!pending) return

    const newContents = new Map(get().directoryContents)
    for (const contentPath of pending.contentPathsToRemove) {
      newContents.delete(contentPath)
    }

    const newPending = new Map(get().pendingCollapses)
    newPending.delete(normalized)

    set({
      directoryContents: newContents,
      pendingCollapses: newPending
    })

    for (const dir of pending.dirsToUnwatch) {
      filesystemApi.unwatchDirectory(dir)
    }
  },

  collapseAll: (): void => {
    const { rootPath, roots, expandedDirs } = get()
    const activeRoots =
      roots.length > 0 ? roots.map((root) => root.path) : rootPath ? [rootPath] : []
    const rootSet = new Set(activeRoots)
    // Unwatch all expanded dirs except root
    expandedDirs.forEach((dir) => {
      if (!rootSet.has(dir)) {
        filesystemApi.unwatchDirectory(dir)
      }
    })
    // Keep only root contents
    const newContents = new Map<string, DirectoryEntry[]>()
    for (const root of activeRoots) {
      const existing = get().directoryContents.get(root)
      if (existing) newContents.set(root, existing)
    }
    set({
      suppressTreeAnimations: true,
      expandedDirs: new Set(activeRoots),
      directoryContents: newContents,
      pendingCollapses: new Map<string, PendingDirectoryCollapse>()
    })
    queueMicrotask(() => {
      set({ suppressTreeAnimations: false })
    })
  },

  setDirectoryContents: (path: string, entries: DirectoryEntry[]): void => {
    const normalized = normalizePath(path)
    const newContents = new Map(get().directoryContents)
    newContents.set(normalized, entries)
    set({ directoryContents: newContents })
  },

  removeDirectoryContents: (path: string): void => {
    const normalized = normalizePath(path)
    const newContents = new Map(get().directoryContents)
    newContents.delete(normalized)
    set({ directoryContents: newContents })
  },

  setVisible: (visible: boolean): void => {
    set({ isVisible: visible })
  },

  setExpandedDirs: (dirs: Set<string>): void => {
    set({ expandedDirs: dirs })
  },

  setRootLoadError: (error: FileExplorerRootError | null): void => {
    const { rootPath, rootLoadErrors } = get()
    const nextRootErrors = new Map(rootLoadErrors)
    if (rootPath) {
      if (error) nextRootErrors.set(rootPath, error)
      else nextRootErrors.delete(rootPath)
    }
    set({ rootLoadError: error, rootLoadErrors: nextRootErrors })
  },

  restoreExpandedDirs: async (dirs: string[]): Promise<void> => {
    const { rootPath, roots } = get()
    const normalizedRoots =
      roots.length > 0 ? roots.map((root) => root.path) : rootPath ? [normalizePath(rootPath)] : []
    if (normalizedRoots.length === 0 || dirs.length === 0) return

    for (const dir of dirs) {
      const normalizedDir = normalizePath(dir)

      if (normalizedRoots.includes(normalizedDir)) {
        continue
      }

      if (!normalizedRoots.some((root) => isPathWithinRoot(normalizedDir, root))) {
        continue
      }

      try {
        await get().toggleDirectory(normalizedDir)
      } catch {
        // Skip invalid/missing directories during restore
      }
    }
  },

  setSearchQuery: (query: string): void => {
    set({ searchQuery: query })
  },

  searchInRoot: async (query: string, requestId: number): Promise<void> => {
    const { rootPath, roots } = get()
    if (!rootPath) {
      set({
        searchLoading: false,
        searchError: runtimeT('projects', 'fileContext.noProjectSelected', 'No project selected'),
        searchErrorCode: null,
        searchResults: [],
        searchFileNameMatches: null,
        searchTruncated: false,
        searchScannedFiles: 0,
        searchFailedFiles: 0,
        searchRequestId: requestId
      })
      return
    }
    const searchRoots =
      roots.length > 0 ? roots.map((root) => root.path) : rootPath ? [rootPath] : []
    if (searchRoots.length === 0) {
      set({
        searchLoading: false,
        searchError: runtimeT('projects', 'fileContext.noProjectSelected', 'No project selected'),
        searchErrorCode: null,
        searchResults: [],
        searchFileNameMatches: null,
        searchTruncated: false,
        searchScannedFiles: 0,
        searchFailedFiles: 0,
        searchRequestId: requestId
      })
      return
    }

    const trimmed = query.trim()
    if (!trimmed || trimmed.length < 2) {
      cancelActiveSearchStreams()
      set({
        searchLoading: false,
        searchError: null,
        searchErrorCode: null,
        searchResults: [],
        searchFileNameMatches: null,
        searchTruncated: false,
        searchScannedFiles: 0,
        searchFailedFiles: 0,
        searchRequestId: requestId
      })
      return
    }

    ensureSearchStreamSubscription(set, get)
    ensureFileNameStreamSubscription(set, get)

    cancelActiveSearchStreams()
    activeSearchRequestId = requestId

    const { searchLastCompletedQuery, searchResults } = get()
    if (
      searchLastCompletedQuery &&
      trimmed.toLowerCase().startsWith(searchLastCompletedQuery.toLowerCase()) &&
      searchResults.length > 0
    ) {
      const filtered = searchResults
        .map((file) => ({
          ...file,
          matches: file.matches.filter((match) =>
            match.lineText.toLowerCase().includes(trimmed.toLowerCase())
          )
        }))
        .filter((file) => file.matches.length > 0)
      set({
        searchResults: filtered,
        searchFileNameMatches: null,
        searchLoading: true,
        searchError: null,
        searchRequestId: requestId,
        searchTruncated: false,
        searchScannedFiles: 0,
        searchFailedFiles: 0
      })
    } else {
      set({
        searchLoading: true,
        searchError: null,
        searchRequestId: requestId,
        searchResults: [],
        searchFileNameMatches: null,
        searchTruncated: false,
        searchScannedFiles: 0,
        searchFailedFiles: 0
      })
    }

    const starts = searchRoots.map(async (searchRoot, index) => {
      const searchId =
        searchRoots.length === 1 ? `search-${requestId}` : `search-${requestId}:${index}`
      activeContentSearchIds.add(searchId)
      activeFileNameSearchIds.add(searchId)
      const [content, fileNames] = await Promise.all([
        filesystemApi.searchContentStreamStart(searchId, searchRoot, searchRoot, trimmed),
        filesystemApi.searchFileNamesStreamStart(searchId, searchRoot, searchRoot, trimmed)
      ])
      return { searchId, content, fileNames }
    })
    const outcomes = await Promise.all(starts)

    if (get().searchRequestId !== requestId || activeSearchRequestId !== requestId) {
      cancelActiveSearchStreams()
      return
    }

    const failures: string[] = []
    for (const outcome of outcomes) {
      if (!outcome.content.success) {
        activeContentSearchIds.delete(outcome.searchId)
        failures.push(outcome.content.error)
      }
      if (!outcome.fileNames.success) {
        activeFileNameSearchIds.delete(outcome.searchId)
        failures.push(outcome.fileNames.error)
      }
      if (outcome.content.success && !outcome.fileNames.success) {
        void filesystemApi.searchContentStreamCancel(outcome.searchId)
        activeContentSearchIds.delete(outcome.searchId)
      }
      if (!outcome.content.success && outcome.fileNames.success) {
        void filesystemApi.searchFileNamesStreamCancel(outcome.searchId)
        activeFileNameSearchIds.delete(outcome.searchId)
      }
    }
    if (failures.length > 0) {
      const noStreamsRemain =
        activeContentSearchIds.size === 0 && activeFileNameSearchIds.size === 0
      set({
        searchLoading: !noStreamsRemain,
        searchError:
          failures[0] ?? runtimeT('projects', 'filesystemErrors.searchFailed', 'Search failed')
      })
    }
  },

  resetSearch: (): void => {
    cancelActiveSearchStreams()
    activeSearchRequestId = 0
    // Set filename matches to an empty list synchronously: the tab is hidden
    // while no search is active, but leaving a stale `null` here means a
    // consumer reading the state sees a lie. The next searchInRoot call will
    // re-seed `null` if it actually starts a new stream.
    set({
      searchQuery: '',
      searchResults: [],
      searchFileNameMatches: [],
      searchLoading: false,
      searchError: null,
      searchErrorCode: null,
      searchTruncated: false,
      searchScannedFiles: 0,
      searchFailedFiles: 0,
      searchRequestId: 0,
      searchLastCompletedQuery: ''
    })
  }
}))

// Selector hooks
export function useFileExplorer(): Pick<
  FileExplorerState,
  | 'roots'
  | 'rootPath'
  | 'expandedDirs'
  | 'directoryContents'
  | 'selectedPaths'
  | 'lastClickedPath'
  | 'clipboard'
  | 'isVisible'
  | 'loadingDirs'
  | 'rootLoadError'
  | 'rootLoadErrors'
  | 'searchQuery'
  | 'searchResults'
  | 'searchFileNameMatches'
  | 'searchLoading'
  | 'searchError'
  | 'searchTruncated'
  | 'searchScannedFiles'
  | 'searchFailedFiles'
  | 'searchLastCompletedQuery'
> {
  return useFileExplorerStore(
    useShallow((state) => ({
      roots: state.roots,
      rootPath: state.rootPath,
      expandedDirs: state.expandedDirs,
      directoryContents: state.directoryContents,
      selectedPaths: state.selectedPaths,
      lastClickedPath: state.lastClickedPath,
      clipboard: state.clipboard,
      isVisible: state.isVisible,
      loadingDirs: state.loadingDirs,
      rootLoadError: state.rootLoadError,
      rootLoadErrors: state.rootLoadErrors,
      searchQuery: state.searchQuery,
      searchResults: state.searchResults,
      searchFileNameMatches: state.searchFileNameMatches,
      searchLoading: state.searchLoading,
      searchError: state.searchError,
      searchTruncated: state.searchTruncated,
      searchScannedFiles: state.searchScannedFiles,
      searchFailedFiles: state.searchFailedFiles,
      searchLastCompletedQuery: state.searchLastCompletedQuery
    }))
  )
}

export function useFileExplorerActions(): Pick<
  FileExplorerState,
  | 'setRootPath'
  | 'setRoots'
  | 'setFocusedRoot'
  | 'setWorktreeRoot'
  | 'toggleDirectory'
  | 'finalizeDirectoryCollapse'
  | 'refreshDirectory'
  | 'refreshTree'
  | 'selectPath'
  | 'togglePathSelection'
  | 'selectPathRange'
  | 'selectAll'
  | 'clearSelection'
  | 'copySelected'
  | 'cutSelected'
  | 'paste'
  | 'duplicateSelected'
  | 'toggleVisibility'
  | 'collapseAll'
  | 'setVisible'
  | 'setExpandedDirs'
  | 'setRootLoadError'
  | 'restoreExpandedDirs'
  | 'setSearchQuery'
  | 'searchInRoot'
  | 'resetSearch'
> {
  return useFileExplorerStore(
    useShallow((state) => ({
      setRootPath: state.setRootPath,
      setRoots: state.setRoots,
      setFocusedRoot: state.setFocusedRoot,
      setWorktreeRoot: state.setWorktreeRoot,
      toggleDirectory: state.toggleDirectory,
      finalizeDirectoryCollapse: state.finalizeDirectoryCollapse,
      refreshDirectory: state.refreshDirectory,
      refreshTree: state.refreshTree,
      selectPath: state.selectPath,
      togglePathSelection: state.togglePathSelection,
      selectPathRange: state.selectPathRange,
      selectAll: state.selectAll,
      clearSelection: state.clearSelection,
      copySelected: state.copySelected,
      cutSelected: state.cutSelected,
      paste: state.paste,
      duplicateSelected: state.duplicateSelected,
      toggleVisibility: state.toggleVisibility,
      collapseAll: state.collapseAll,
      setVisible: state.setVisible,
      setExpandedDirs: state.setExpandedDirs,
      setRootLoadError: state.setRootLoadError,
      restoreExpandedDirs: state.restoreExpandedDirs,
      setSearchQuery: state.setSearchQuery,
      searchInRoot: state.searchInRoot,
      resetSearch: state.resetSearch
    }))
  )
}

export function useFileExplorerVisible(): boolean {
  return useFileExplorerStore((state) => state.isVisible)
}
