import type { DirectoryEntry } from '@shared/types/filesystem.types'
import {
  ChevronDown,
  ChevronRight,
  ChevronsDownUp,
  FilePlus,
  FolderGit2,
  FolderPlus,
  LoaderCircle,
  PanelRightClose,
  RefreshCw,
  Search,
  SquareTerminal,
  X
} from 'lucide-react'
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { useUpdatePanelVisibility } from '@/hooks/use-app-settings'
import { clipboardApi, filesystemApi, openerApi } from '@/lib/api'
import { openTerminalAtCwd } from '@/lib/terminal-spawn'
import { cn } from '@/lib/utils'
import { useEditorStore } from '@/stores/editor-store'
import {
  useFileExplorer,
  useFileExplorerActions,
  useFileExplorerStore
} from '@/stores/file-explorer-store'
import { useProjectStore } from '@/stores/project-store'

import { editorTabId, useWorkspaceStore } from '@/stores/workspace-store'
import { FileTreeContextMenuContent } from './FileTreeContextMenu'
import { FileTreeNodeWrapper } from './FileTreeNode'

interface InlineInputState {
  parentPath: string
  type: 'file' | 'folder'
  mode: 'create' | 'rename'
  existingEntry?: DirectoryEntry
}

/** Outcome of expanding the create-target chain (GH-539). */
type ExpandChainResult =
  | { status: 'expanded'; dir: string }
  | { status: 'load-failed' }
  | { status: 'root-changed' }

interface FileExplorerProps {
  side?: 'left' | 'right'
  /** Fill the parent rail. The parent owns width and the drag handle. */
  fillContainer?: boolean
}

export function FileExplorer({
  side = 'right',
  fillContainer = false
}: FileExplorerProps): React.JSX.Element {
  const { t } = useTranslation('workspace')
  const {
    roots: rawRoots,
    rootPath,
    expandedDirs,
    directoryContents,
    loadingDirs,
    rootLoadError,
    rootLoadErrors: rawRootLoadErrors,
    selectedPaths,
    clipboard,
    searchQuery,
    searchResults,
    searchFileNameMatches,
    searchLoading,
    searchError,
    searchTruncated,
    searchScannedFiles,
    searchFailedFiles,
    searchLastCompletedQuery
  } = useFileExplorer()
  const roots = rawRoots ?? []
  const rootLoadErrors = rawRootLoadErrors ?? new Map()
  const {
    toggleDirectory,
    selectPath,
    togglePathSelection,
    selectPathRange,
    selectAll,
    clearSelection,
    copySelected,
    cutSelected,
    paste,
    duplicateSelected,
    collapseAll,
    refreshDirectory,
    refreshTree,
    setFocusedRoot,
    setRootLoadError,
    setSearchQuery,
    searchInRoot,
    resetSearch
  } = useFileExplorerActions()

  const [inlineInput, setInlineInput] = useState<InlineInputState | null>(null)
  const [inputValue, setInputValue] = useState('')
  // A list, not a single entry: the confirm dialog reports `selectedPaths.size`,
  // so holding one entry made it promise "delete 3" and then delete one.
  const [deleteConfirm, setDeleteConfirm] = useState<DirectoryEntry[] | null>(null)
  const [searchResultTab, setSearchResultTab] = useState<'content' | 'files'>('content')
  const [expandedSearchResultPaths, setExpandedSearchResultPaths] = useState<Set<string>>(new Set())
  const [explorerWidth, setExplorerWidth] = useState(() => {
    try {
      const savedWidth = window.localStorage?.getItem('termul:file-explorer-width')
      if (!savedWidth) return 256
      const parsed = Number.parseInt(savedWidth, 10)
      if (Number.isNaN(parsed)) return 256
      return Math.max(220, Math.min(560, parsed))
    } catch {
      return 256
    }
  })
  const inputRef = useRef<HTMLInputElement>(null)
  const containerRef = useRef<HTMLDivElement>(null)
  const searchDebounceRef = useRef<number | null>(null)
  const searchRequestIdRef = useRef(0)
  const resizeStateRef = useRef<{ startX: number; startWidth: number } | null>(null)
  const resizeCleanupRef = useRef<(() => void) | null>(null)
  const userSelectedTabRef = useRef(false)
  // Mirrors of component state so async header-create handlers can re-check
  // the latest values after awaiting chain expansion (GH-539 / GH-540).
  // Synced in useLayoutEffect (not during render) so the mirror is committed
  // before any post-paint handler runs.
  const inlineInputRef = useRef<InlineInputState | null>(null)
  useLayoutEffect(() => {
    inlineInputRef.current = inlineInput
  }, [inlineInput])
  const headerCreateInFlightRef = useRef(false)

  const rootEntries = rootPath ? directoryContents.get(rootPath) : undefined
  const isMultiRoot = roots.length > 1
  const normalizedSearchQuery = searchQuery ?? ''
  const safeSearchResults = searchResults ?? []
  const safeSearchFileNameMatches = searchFileNameMatches ?? []
  const fileNameMatchesPending = searchFileNameMatches === null
  const hasSearchInput = normalizedSearchQuery.length > 0
  const trimmedSearchQuery = normalizedSearchQuery.trim()
  const isSearchActive = trimmedSearchQuery.length > 0
  const isSearchTooShort = isSearchActive && trimmedSearchQuery.length < 2
  const hasContentResults = safeSearchResults.length > 0
  const hasFileResults = safeSearchFileNameMatches.length > 0
  const hasAnySearchResults = hasContentResults || hasFileResults
  const hasPartialSearchError = Boolean(searchError) && hasAnySearchResults
  const _totalContentMatches = safeSearchResults.reduce(
    (total, fileResult) => total + fileResult.matches.length,
    0
  )
  const resultsAreCurrent = searchLastCompletedQuery === trimmedSearchQuery
  const showSearchEmptyState =
    trimmedSearchQuery.length >= 2 &&
    resultsAreCurrent &&
    !searchLoading &&
    !searchError &&
    !hasAnySearchResults

  useEffect(() => {
    userSelectedTabRef.current = false
  }, [])

  useEffect(() => {
    if (userSelectedTabRef.current) {
      return
    }
    if (
      searchResultTab === 'content' &&
      safeSearchResults.length === 0 &&
      safeSearchFileNameMatches.length > 0 &&
      !searchLoading
    ) {
      setSearchResultTab('files')
      return
    }
    if (
      searchResultTab === 'files' &&
      safeSearchFileNameMatches.length === 0 &&
      safeSearchResults.length > 0 &&
      !searchLoading
    ) {
      setSearchResultTab('content')
    }
  }, [safeSearchFileNameMatches.length, searchResultTab, safeSearchResults.length, searchLoading])

  useEffect(() => {
    try {
      window.localStorage?.setItem('termul:file-explorer-width', String(explorerWidth))
    } catch {
      // Ignore localStorage access failures in restricted environments.
    }
  }, [explorerWidth])

  const finalizeResizeDrag = useCallback(() => {
    resizeStateRef.current = null
    document.body.style.userSelect = ''
    if (resizeCleanupRef.current) {
      resizeCleanupRef.current()
      resizeCleanupRef.current = null
    }
  }, [])

  const applyResizedWidth = useCallback((rawWidth: number) => {
    const nextWidth = Math.max(220, Math.min(560, rawWidth))
    setExplorerWidth(nextWidth)
  }, [])

  const handleResizeMouseDown = useCallback(
    (event: React.MouseEvent<HTMLButtonElement>) => {
      event.preventDefault()
      document.body.style.userSelect = 'none'
      resizeStateRef.current = { startX: event.clientX, startWidth: explorerWidth }

      const onMouseMove = (moveEvent: MouseEvent) => {
        const state = resizeStateRef.current
        if (!state) return
        const delta = moveEvent.clientX - state.startX
        const rawWidth = side === 'right' ? state.startWidth - delta : state.startWidth + delta
        applyResizedWidth(rawWidth)
      }

      const onMouseUp = () => {
        finalizeResizeDrag()
      }

      const onWindowBlur = () => {
        finalizeResizeDrag()
      }

      document.addEventListener('mousemove', onMouseMove)
      document.addEventListener('mouseup', onMouseUp)
      window.addEventListener('blur', onWindowBlur)
      resizeCleanupRef.current = () => {
        document.removeEventListener('mousemove', onMouseMove)
        document.removeEventListener('mouseup', onMouseUp)
        window.removeEventListener('blur', onWindowBlur)
      }
    },
    [applyResizedWidth, explorerWidth, finalizeResizeDrag, side]
  )

  const handleResizeKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLButtonElement>) => {
      if (
        event.key !== 'ArrowLeft' &&
        event.key !== 'ArrowRight' &&
        event.key !== 'Home' &&
        event.key !== 'End'
      ) {
        return
      }
      event.preventDefault()
      const step = 16
      if (event.key === 'Home') {
        applyResizedWidth(220)
        return
      }
      if (event.key === 'End') {
        applyResizedWidth(560)
        return
      }
      const directionalDelta = event.key === 'ArrowLeft' ? -step : step
      const signedDelta = side === 'right' ? -directionalDelta : directionalDelta
      applyResizedWidth(explorerWidth + signedDelta)
    },
    [applyResizedWidth, explorerWidth, side]
  )

  // Auto-expand every project root in a multi-root group.
  useEffect(() => {
    const activeRoots =
      roots.length > 0 ? roots : rootPath ? [{ projectId: '', name: '', path: rootPath }] : []
    for (const root of activeRoots) {
      if (!directoryContents.has(root.path) && !rootLoadErrors.has(root.path)) {
        void toggleDirectory(root.path)
      }
    }
  }, [roots, rootPath, directoryContents, rootLoadErrors, toggleDirectory])

  const handleOpenRootTerminal = useCallback(
    async (projectId: string, cwd: string): Promise<void> => {
      const outcome = await openTerminalAtCwd(projectId, cwd)
      if (outcome.status === 'spawn-failed') {
        toast.error(outcome.error ?? t('errors.createTerminal'))
      } else if (outcome.status === 'no-pane') {
        toast.error(t('errors.createTerminal'))
      }
    },
    [t]
  )

  useEffect(() => {
    resetSearch()
    setExpandedSearchResultPaths(new Set())
  }, [resetSearch])

  useEffect(() => {
    setExpandedSearchResultPaths(new Set())
  }, [])

  useEffect(() => {
    if (searchDebounceRef.current !== null) {
      window.clearTimeout(searchDebounceRef.current)
    }

    if (!rootPath) {
      return
    }

    searchDebounceRef.current = window.setTimeout(
      () => {
        searchRequestIdRef.current += 1
        void searchInRoot(normalizedSearchQuery, searchRequestIdRef.current)
      },
      trimmedSearchQuery.length >= 3 ? 90 : 180
    )

    return () => {
      if (searchDebounceRef.current !== null) {
        window.clearTimeout(searchDebounceRef.current)
      }
    }
  }, [rootPath, normalizedSearchQuery, searchInRoot, trimmedSearchQuery.length])

  useEffect(() => {
    return () => {
      finalizeResizeDrag()
    }
  }, [finalizeResizeDrag])

  // Cancel any in-flight filename/content stream when the explorer unmounts.
  // The store is a module-level singleton and the file explorer can be torn
  // down while a stream is still mid-walk; without this cleanup the rg child
  // process outlives the component and the next mount sees stale events.
  //
  // We capture the `searchRequestId` at effect setup time (not at cleanup
  // time) so that a new search started via a different code path between
  // setup and cleanup does not get cancelled by mistake.
  useEffect(() => {
    const id = useFileExplorerStore.getState().searchRequestId
    return () => {
      if (id > 0) {
        const sid = `search-${id}`
        // Surface silent IPC failures so a stuck rg process is at least
        // visible in the console; the cancel is still fire-and-forget
        // from the user's perspective.
        filesystemApi.searchFileNamesStreamCancel(sid).catch((e) => {
          console.warn(`[file-explorer] searchFileNamesStreamCancel(${sid}) failed:`, e)
        })
        filesystemApi.searchContentStreamCancel(sid).catch((e) => {
          console.warn(`[file-explorer] searchContentStreamCancel(${sid}) failed:`, e)
        })
      }
    }
  }, [])

  // Focus inline input when it appears
  useEffect(() => {
    if (inlineInput && inputRef.current) {
      inputRef.current.focus()
      if (inlineInput.mode === 'rename' && inlineInput.existingEntry) {
        // Select the name without extension for files
        const name = inlineInput.existingEntry.name
        if (inlineInput.existingEntry.type === 'file') {
          const dotIndex = name.lastIndexOf('.')
          if (dotIndex > 0) {
            inputRef.current.setSelectionRange(0, dotIndex)
          } else {
            inputRef.current.select()
          }
        } else {
          inputRef.current.select()
        }
      }
    }
  }, [inlineInput])

  const handleSelect = useCallback(
    async (path: string) => {
      selectPath(path)
      try {
        await useEditorStore.getState().openFile(path)
        useWorkspaceStore.getState().addEditorTab(path)
      } catch {
        // File couldn't be opened (binary, too large, etc.)
      }
    },
    [selectPath]
  )

  const handleContextMenu = useCallback(
    (e: React.MouseEvent, entry: DirectoryEntry) => {
      // F1: no preventDefault() — Radix's `<ContextMenuTrigger asChild>` composes
      // this handler ahead of its own handleOpen (checkForDefaultPrevented: true);
      // a preventDefault here would make Radix skip opening the menu. Radix's
      // own handleContextMenu already suppresses the native menu.
      e.stopPropagation()
      // If right-clicking on an unselected item, select only that item
      // If right-clicking on a selected item, keep the current selection.
      // Stopping propagation also keeps the global `GlobalContextMenu` trigger
      // from firing over the file tree.
      if (!selectedPaths.has(entry.path)) {
        selectPath(entry.path)
      }
    },
    [selectPath, selectedPaths]
  )

  // Handle multi-select clicks
  const handleNodeClick = useCallback(
    (e: React.MouseEvent, entry: DirectoryEntry) => {
      const lastClickedPath = useFileExplorerStore.getState().lastClickedPath

      if (e.ctrlKey || e.metaKey) {
        // Ctrl+Click: Toggle selection
        togglePathSelection(entry.path)
      } else if (e.shiftKey && lastClickedPath) {
        // Shift+Click: Range selection
        selectPathRange(lastClickedPath, entry.path)
      } else {
        // Normal click: Single selection (and toggle directory if it's a directory)
        if (entry.type === 'directory') {
          toggleDirectory(entry.path)
        } else {
          selectPath(entry.path)
          handleSelect(entry.path)
        }
      }
    },
    [togglePathSelection, selectPathRange, selectPath, toggleDirectory, handleSelect]
  )

  // Handle keyboard shortcuts
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // Only handle shortcuts when file explorer is focused
      if (
        !containerRef.current?.contains(document.activeElement) &&
        document.activeElement !== document.body
      ) {
        return
      }

      // Don't handle shortcuts when typing in an input
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) {
        return
      }

      // Ctrl+A: Select all
      if ((e.ctrlKey || e.metaKey) && e.key === 'a') {
        e.preventDefault()
        selectAll()
        return
      }

      // Ctrl+C: Copy
      if ((e.ctrlKey || e.metaKey) && e.key === 'c') {
        e.preventDefault()
        copySelected()
        return
      }

      // Ctrl+X: Cut
      if ((e.ctrlKey || e.metaKey) && e.key === 'x') {
        e.preventDefault()
        cutSelected()
        return
      }

      // Ctrl+V: Paste
      if ((e.ctrlKey || e.metaKey) && e.key === 'v') {
        e.preventDefault()
        let targetPath: string | undefined
        if (selectedPaths.size === 1) {
          const [selectedPath] = [...selectedPaths]
          let isDirectory = false
          outer: for (const [, entries] of directoryContents) {
            for (const entry of entries) {
              if (entry.path === selectedPath && entry.type === 'directory') {
                isDirectory = true
                break outer
              }
            }
          }
          if (isDirectory) {
            targetPath = selectedPath
          } else {
            const normalized = selectedPath.replace(/\\/g, '/')
            const lastSlash = normalized.lastIndexOf('/')
            targetPath = lastSlash > 0 ? normalized.slice(0, lastSlash) : (rootPath ?? '')
          }
        }
        if (!targetPath && rootPath) {
          targetPath = rootPath
        }
        if (targetPath) {
          void paste(targetPath)
        }
        return
      }

      // F2: Rename
      if (e.key === 'F2' && selectedPaths.size === 1) {
        e.preventDefault()
        const [path] = selectedPaths
        const normalizedPath = path.replace(/\\/g, '/')
        const lastSlash = normalizedPath.lastIndexOf('/')
        const parentPath =
          lastSlash > 0 ? normalizedPath.slice(0, lastSlash) : lastSlash === 0 ? '/' : ''
        // Find the entry for this path
        for (const [, entries] of directoryContents) {
          const entry = entries.find((e) => e.path === path)
          if (entry) {
            setInlineInput({
              parentPath,
              type: entry.type === 'directory' ? 'folder' : 'file',
              mode: 'rename',
              existingEntry: entry
            })
            setInputValue(entry.name)
            break
          }
        }
        return
      }

      // Delete: Move to trash (for now, permanent delete)
      if (e.key === 'Delete' && selectedPaths.size > 0) {
        e.preventDefault()
        const seen = new Set<string>()
        const targets: DirectoryEntry[] = []
        for (const [, entries] of directoryContents) {
          for (const entry of entries) {
            if (selectedPaths.has(entry.path) && !seen.has(entry.path)) {
              seen.add(entry.path)
              targets.push(entry)
            }
          }
        }
        if (targets.length > 0) setDeleteConfirm(targets)
        return
      }

      // Escape: Clear selection
      if (e.key === 'Escape') {
        clearSelection()
        return
      }
    }

    document.addEventListener('keydown', handleKeyDown)
    return () => document.removeEventListener('keydown', handleKeyDown)
  }, [
    selectAll,
    copySelected,
    cutSelected,
    paste,
    selectedPaths,
    directoryContents,
    clearSelection,
    rootPath
  ])

  const handleNewFile = useCallback((dirPath: string) => {
    setInlineInput({ parentPath: dirPath, type: 'file', mode: 'create' })
    setInputValue('')
  }, [])

  const handleNewFolder = useCallback((dirPath: string) => {
    setInlineInput({ parentPath: dirPath, type: 'folder', mode: 'create' })
    setInputValue('')
  }, [])

  /** Find a directory entry by absolute path across all loaded directories. */
  const findEntryByPath = useCallback(
    (path: string): DirectoryEntry | undefined => {
      for (const entries of directoryContents.values()) {
        const found = entries.find((entry) => entry.path === path)
        if (found) return found
      }
      return undefined
    },
    [directoryContents]
  )

  /**
   * VSCode-style target resolution for header creation actions (GH-540):
   * selected directory > parent of selected file > project root.
   * Multi-selection or unresolvable selections fall back to the root.
   */
  const getCreateTargetDir = useCallback((): string => {
    if (!rootPath) return ''
    if (selectedPaths.size === 1) {
      const selectedPath = Array.from(selectedPaths)[0]
      const selectedEntry = findEntryByPath(selectedPath)
      if (selectedEntry?.type === 'directory') return selectedEntry.path.replace(/\\/g, '/')
      if (selectedEntry?.type === 'file') {
        const normalized = selectedEntry.path.replace(/\\/g, '/')
        const lastSlash = normalized.lastIndexOf('/')
        return lastSlash > 0 ? normalized.slice(0, lastSlash) : lastSlash === 0 ? '/' : rootPath
      }
    }
    return rootPath
  }, [rootPath, selectedPaths, findEntryByPath])

  /** Expand every directory from the project root down to (and including) the target. */
  const expandDirectoryChain = useCallback(
    async (targetDir: string): Promise<ExpandChainResult> => {
      if (!rootPath) return { status: 'load-failed' }
      const normalizedRoot = rootPath.replace(/\\/g, '/')
      // Separator-safe prefix check (handles root '/' without '//').
      const rootPrefix = normalizedRoot.endsWith('/') ? normalizedRoot : `${normalizedRoot}/`
      // Resolve the target to an in-root directory; stale/out-of-root
      // selections clamp to the active root so creation can never escape it.
      let resolved = targetDir.replace(/\\/g, '/')
      if (resolved !== normalizedRoot && !resolved.startsWith(rootPrefix)) {
        resolved = normalizedRoot
      }

      const chain: string[] = []
      let current = resolved
      while (current !== normalizedRoot && current.startsWith(rootPrefix)) {
        chain.push(current)
        const lastSlash = current.lastIndexOf('/')
        if (lastSlash <= 0) break
        current = current.slice(0, lastSlash)
      }
      chain.push(normalizedRoot)
      chain.reverse()

      for (const dir of chain) {
        if (!useFileExplorerStore.getState().expandedDirs.has(dir)) {
          await toggleDirectory(dir)
        }
        // Abort if the directory could not be expanded (load failure) or the
        // project changed mid-chain — never create into an invisible or
        // foreign tree.
        const state = useFileExplorerStore.getState()
        if (state.rootPath !== rootPath) return { status: 'root-changed' }
        if (!state.expandedDirs.has(dir)) return { status: 'load-failed' }
      }
      return { status: 'expanded', dir: resolved }
    },
    [rootPath, toggleDirectory]
  )

  /** Best-effort scroll of a tree row (by entry path) into view. */
  const revealTreePath = useCallback((path: string) => {
    window.requestAnimationFrame(() => {
      const row = containerRef.current?.querySelector(`[data-path="${CSS.escape(path)}"]`)
      row?.scrollIntoView({ block: 'nearest' })
    })
  }, [])

  /** Select a newly created entry and scroll its row into view (best-effort). */
  const revealCreatedEntry = useCallback(
    (path: string) => {
      selectPath(path)
      revealTreePath(path)
    },
    [selectPath, revealTreePath]
  )

  /**
   * Header New File / New Folder (GH-540): resolve + expand + reveal the
   * target directory, then start the existing inline create flow.
   */
  const startHeaderCreate = useCallback(
    async (type: 'file' | 'folder') => {
      // Never clobber an in-progress create/rename input, and serialize
      // header requests while the chain expansion is awaiting.
      if (inlineInputRef.current || headerCreateInFlightRef.current) return
      const targetDir = getCreateTargetDir()
      if (!targetDir) return
      headerCreateInFlightRef.current = true
      try {
        const result = await expandDirectoryChain(targetDir)
        // Re-check after the awaits: another flow may have opened an input or
        // switched projects while the chain was expanding.
        if (result.status !== 'expanded' || inlineInputRef.current) {
          if (result.status === 'load-failed') {
            toast.error(t('fileExplorer.targetDirectoryFailed'), {
              description: t('fileExplorer.targetDirectoryFailedDescription')
            })
          }
          return
        }
        revealTreePath(result.dir)
        setInlineInput({ parentPath: result.dir, type, mode: 'create' })
        setInputValue('')
      } finally {
        headerCreateInFlightRef.current = false
      }
    },
    [getCreateTargetDir, expandDirectoryChain, revealTreePath, t]
  )

  /** Header Refresh (GH-540): re-read root + expanded dirs, keeping state. */
  const handleHeaderRefresh = useCallback(() => {
    void refreshTree()
  }, [refreshTree])

  const handleRename = useCallback((entry: DirectoryEntry) => {
    const normalizedPath = entry.path.replace(/\\/g, '/')
    const lastSlash = normalizedPath.lastIndexOf('/')
    const parentPath =
      lastSlash > 0 ? normalizedPath.slice(0, lastSlash) : lastSlash === 0 ? '/' : ''
    setInlineInput({
      parentPath,
      type: entry.type === 'directory' ? 'folder' : 'file',
      mode: 'rename',
      existingEntry: entry
    })
    setInputValue(entry.name)
  }, [])

  /**
   * VS Code semantics: acting on an entry that belongs to a multi-selection acts
   * on the whole selection; acting on an entry outside it acts on that one alone.
   */
  const handleDelete = useCallback(
    (entry: DirectoryEntry) => {
      if (selectedPaths.size <= 1 || !selectedPaths.has(entry.path)) {
        setDeleteConfirm([entry])
        return
      }
      const targets: DirectoryEntry[] = []
      for (const path of selectedPaths) {
        const found = findEntryByPath(path)
        if (found) targets.push(found)
      }
      setDeleteConfirm(targets.length > 0 ? targets : [entry])
    },
    [selectedPaths, findEntryByPath]
  )

  /**
   * The panel toggle also lives in the title bar, but that is far from where the
   * user is looking when they want this tree out of the way — the same
   * discoverability gap that made VS Code move its terminal list out of a hidden
   * dropdown. Both drive the one shared visibility flag.
   */
  const updatePanelVisibility = useUpdatePanelVisibility()
  const handleHidePanel = useCallback(async () => {
    try {
      await updatePanelVisibility('fileExplorerVisible', false)
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('fileExplorer.hidePanelFailed'))
    }
  }, [updatePanelVisibility, t])

  const handleCopyPath = useCallback((path: string) => {
    void clipboardApi.writeText(path)
  }, [])

  const isSubmittingRef = useRef(false)
  const submitFailedRef = useRef(false)

  const handleInlineInputSubmit = useCallback(async () => {
    if (isSubmittingRef.current) return
    isSubmittingRef.current = true
    submitFailedRef.current = false

    if (!inlineInput || !inputValue.trim()) {
      setInlineInput(null)
      isSubmittingRef.current = false
      return
    }

    const name = inputValue.trim()
    // Reject path separators and dot-segments so creation can never escape
    // the target directory (GH-539). Release the submission lock here — this
    // branch returns before the try/finally that resets it.
    if (name === '.' || name === '..' || /[/\\]/.test(name)) {
      toast.error(t('fileExplorer.invalidName'))
      submitFailedRef.current = true
      isSubmittingRef.current = false
      return
    }
    // Separator-safe join: a filesystem-root parent ('/') must not gain a
    // second slash before the name.
    const targetParent = inlineInput.parentPath
    const fullPath = targetParent.endsWith('/')
      ? `${targetParent}${name}`
      : `${targetParent}/${name}`

    try {
      let result: { success: boolean; error?: string } | undefined
      if (inlineInput.mode === 'create') {
        if (inlineInput.type === 'file') {
          result = await filesystemApi.createFile(fullPath)
        } else {
          result = await filesystemApi.createDirectory(fullPath)
        }
      } else if (inlineInput.mode === 'rename' && inlineInput.existingEntry) {
        result = await filesystemApi.renameFile(inlineInput.existingEntry.path, fullPath)
      }

      if (!result?.success) {
        toast.error(result?.error || t('fileExplorer.operationFailed'))
        submitFailedRef.current = true
        return
      }

      // Success side-effects
      if (inlineInput.mode === 'rename' && inlineInput.existingEntry) {
        // If the renamed file was open in editor, close old tab
        const editorState = useEditorStore.getState()
        if (editorState.openFiles.has(inlineInput.existingEntry.path)) {
          editorState.closeFile(inlineInput.existingEntry.path)
          useWorkspaceStore.getState().removeTab(editorTabId(inlineInput.existingEntry.path))
        }
      }
      await refreshDirectory(inlineInput.parentPath)
      if (inlineInput.mode === 'create') {
        // GH-539: newly created entries are selected and revealed in the tree.
        revealCreatedEntry(fullPath)
      }
      setInlineInput(null)
      setInputValue('')
    } catch (err) {
      toast.error(err instanceof Error ? err.message : t('fileExplorer.unknownError'))
      submitFailedRef.current = true
    } finally {
      isSubmittingRef.current = false
    }
  }, [inlineInput, inputValue, refreshDirectory, revealCreatedEntry, t])

  const handleInlineInputCancel = useCallback(() => {
    if (isSubmittingRef.current) return
    if (submitFailedRef.current) {
      submitFailedRef.current = false
      return
    }
    setInlineInput(null)
    setInputValue('')
  }, [])

  const handleDeleteConfirm = useCallback(async () => {
    if (!deleteConfirm || deleteConfirm.length === 0) return

    const store = useFileExplorerStore.getState()
    const newExpanded = new Set(store.expandedDirs)
    const newContents = new Map(store.directoryContents)
    const parentsToRefresh = new Set<string>()
    let directoryWasDeleted = false

    for (const target of deleteConfirm) {
      const isDir = target.type === 'directory'
      const result = await filesystemApi.deletePath(target.path, { recursive: isDir })

      if (!result.success) {
        toast.error(t('fileExplorer.deleteFailed', { path: target.path, message: result.error }))
        // Keep going. Abandoning the rest on the first failure is exactly what
        // "it said 3 and removed 1" looks like from the outside.
        continue
      }

      const normalizedDeletePath = target.path.replace(/\\/g, '/')
      parentsToRefresh.add(normalizedDeletePath.substring(0, normalizedDeletePath.lastIndexOf('/')))

      // Re-read per iteration: closeFile mutates the store, so a snapshot taken
      // before the loop goes stale after the first deletion.
      const editorState = useEditorStore.getState()
      const workspaceState = useWorkspaceStore.getState()

      if (isDir) {
        directoryWasDeleted = true
        for (const [openFilePath] of editorState.openFiles) {
          const normalizedOpenPath = openFilePath.replace(/\\/g, '/')
          if (
            normalizedOpenPath === normalizedDeletePath ||
            normalizedOpenPath.startsWith(`${normalizedDeletePath}/`)
          ) {
            editorState.closeFile(openFilePath)
            workspaceState.removeTab(editorTabId(openFilePath))
          }
        }

        for (const key of newContents.keys()) {
          if (key === normalizedDeletePath || key.startsWith(`${normalizedDeletePath}/`)) {
            newExpanded.delete(key)
            newContents.delete(key)
            void filesystemApi.unwatchDirectory(key)
          }
        }
      } else if (editorState.openFiles.has(target.path)) {
        editorState.closeFile(target.path)
        workspaceState.removeTab(editorTabId(target.path))
      }
    }

    if (directoryWasDeleted) {
      useFileExplorerStore.setState({
        expandedDirs: newExpanded,
        directoryContents: newContents
      })
    }

    useFileExplorerStore.getState().clearSelection()
    for (const parentPath of parentsToRefresh) {
      await refreshDirectory(parentPath)
    }
    setDeleteConfirm(null)
  }, [deleteConfirm, refreshDirectory, t])

  const handleSearchMatchClick = useCallback(
    async (filePath: string, lineNumber: number) => {
      const searchTerm = searchLastCompletedQuery.trim()
      selectPath(filePath)
      try {
        await useEditorStore.getState().openFile(filePath)
        useWorkspaceStore.getState().addEditorTab(filePath)
        const isMarkdown = /\.md$/i.test(filePath)
        if (isMarkdown) {
          useEditorStore.getState().setViewMode(filePath, 'code')
        }
        useEditorStore.getState().updateCursorPosition(filePath, lineNumber, 1)
        const revealDetail = { filePath, lineNumber, searchTerm }
        ;(
          window as unknown as { __termulPendingRevealLine?: typeof revealDetail }
        ).__termulPendingRevealLine = revealDetail
        window.dispatchEvent(
          new CustomEvent('termul:reveal-line', {
            detail: revealDetail
          })
        )
        requestAnimationFrame(() => {
          window.dispatchEvent(
            new CustomEvent('termul:reveal-line', {
              detail: revealDetail
            })
          )
        })
      } catch {
        toast.warning(t('fileExplorer.fileFocusFailed'))
      }
    },
    [searchLastCompletedQuery, selectPath, t]
  )

  const handleRootRetry = useCallback(() => {
    if (!rootPath) return
    setRootLoadError(null)
    void toggleDirectory(rootPath)
  }, [rootPath, setRootLoadError, toggleDirectory])

  const toggleExpandedSearchResult = useCallback((filePath: string) => {
    setExpandedSearchResultPaths((current) => {
      const next = new Set(current)
      if (next.has(filePath)) {
        next.delete(filePath)
      } else {
        next.add(filePath)
      }
      return next
    })
  }, [])

  const getFileLabel = useCallback(
    (filePath: string) => {
      const normalizedFilePath = filePath.replace(/\\/g, '/')
      const owningRoot = roots
        .filter((root) => {
          const normalizedRoot = root.path.replace(/\\/g, '/')
          return (
            normalizedFilePath === normalizedRoot ||
            normalizedFilePath.startsWith(`${normalizedRoot}/`)
          )
        })
        .sort((left, right) => right.path.length - left.path.length)[0]
      const normalizedRootPath = (owningRoot?.path ?? rootPath ?? '').replace(/\\/g, '/')
      const fileName = normalizedFilePath.split('/').pop() ?? normalizedFilePath
      const rootRelativePath = normalizedRootPath
        ? normalizedFilePath.replace(`${normalizedRootPath}/`, '')
        : normalizedFilePath
      const relativePath =
        roots.length > 1 && owningRoot ? `${owningRoot.name}/${rootRelativePath}` : rootRelativePath
      const folderPath = relativePath.includes('/')
        ? relativePath.slice(0, relativePath.lastIndexOf('/'))
        : ''

      return { fileName, folderPath, relativePath }
    },
    [rootPath, roots]
  )

  const renderHighlightedLine = useCallback(
    (lineText: string) => {
      const query = normalizedSearchQuery.trim()
      if (!query) return lineText

      const lowerLine = lineText.toLowerCase()
      const lowerQuery = query.toLowerCase()
      const parts: React.ReactNode[] = []
      let startIndex = 0
      let matchIndex = lowerLine.indexOf(lowerQuery, startIndex)

      while (matchIndex !== -1) {
        if (matchIndex > startIndex) {
          parts.push(lineText.slice(startIndex, matchIndex))
        }

        parts.push(
          <span
            key={`${matchIndex}-${matchIndex + query.length}`}
            className="bg-primary/18 text-foreground"
          >
            {lineText.slice(matchIndex, matchIndex + query.length)}
          </span>
        )

        startIndex = matchIndex + query.length
        matchIndex = lowerLine.indexOf(lowerQuery, startIndex)
      }

      if (startIndex < lineText.length) {
        parts.push(lineText.slice(startIndex))
      }

      return parts.length > 0 ? parts : lineText
    },
    [normalizedSearchQuery]
  )

  // Open terminal in directory
  const handleOpenInTerminal = useCallback(
    async (dirPath: string) => {
      const owningRoot = roots
        .filter((root) => dirPath === root.path || dirPath.startsWith(`${root.path}/`))
        .sort((left, right) => right.path.length - left.path.length)[0]
      const projectId = owningRoot?.projectId || useProjectStore.getState().activeProjectId
      if (!projectId) {
        toast.error(t('fileExplorer.noActiveProject'))
        return
      }
      const outcome = await openTerminalAtCwd(projectId, dirPath)
      if (outcome.status === 'spawn-failed') {
        toast.error(outcome.error ?? t('fileExplorer.failedOpenTerminal'))
      } else if (outcome.status === 'no-pane') {
        toast.error(t('fileExplorer.failedOpenTerminal'))
      }
    },
    [roots, t]
  )

  // Open with external app
  const handleOpenWithExternal = useCallback(
    async (filePath: string) => {
      const result = await openerApi.openWithExternalApp(filePath)
      if (!result.success) {
        toast.error(t('fileExplorer.failedOpenFile', { message: result.error }))
      }
    },
    [t]
  )

  // Show in file manager
  const handleShowInFileManager = useCallback(
    async (path: string) => {
      const result = await openerApi.revealInFileManager(path)
      if (!result.success) {
        toast.error(t('fileExplorer.failedReveal', { message: result.error }))
      }
    },
    [t]
  )

  // Copy handler
  const handleCopy = useCallback(() => {
    copySelected()
  }, [copySelected])

  // Cut handler
  const handleCut = useCallback(() => {
    cutSelected()
  }, [cutSelected])

  // Paste handler
  const handlePaste = useCallback(
    async (destinationPath: string) => {
      await paste(destinationPath)
    },
    [paste]
  )

  // Duplicate handler
  const handleDuplicate = useCallback(async () => {
    await duplicateSelected()
  }, [duplicateSelected])

  // Per-node Radix context-menu content. `FileTreeNode` wraps each node row in
  // `<ContextMenu><ContextMenuTrigger asChild>{row}</ContextMenuTrigger>{...}</ContextMenu>`;
  // this callback supplies the declarative `<ContextMenuContent>` for a given
  // entry (icons on every item, desktop-only reveal/external-open gated by
  // `isTauriContext()`). The selection count + clipboard presence are captured
  // fresh on every render so the menu reflects the current multi-select state.
  const renderFileTreeContextMenu = useCallback(
    (entry: DirectoryEntry) => (
      <FileTreeContextMenuContent
        entry={entry}
        onNewFile={handleNewFile}
        onNewFolder={handleNewFolder}
        onRename={handleRename}
        onDelete={handleDelete}
        onCopyPath={handleCopyPath}
        onCopy={handleCopy}
        onCut={handleCut}
        onPaste={handlePaste}
        onDuplicate={handleDuplicate}
        onOpenInTerminal={handleOpenInTerminal}
        onOpenWithExternal={handleOpenWithExternal}
        onShowInFileManager={handleShowInFileManager}
        selectedCount={selectedPaths.size}
        hasClipboardContent={clipboard !== null}
      />
    ),
    [
      handleNewFile,
      handleNewFolder,
      handleRename,
      handleDelete,
      handleCopyPath,
      handleCopy,
      handleCut,
      handlePaste,
      handleDuplicate,
      handleOpenInTerminal,
      handleOpenWithExternal,
      handleShowInFileManager,
      selectedPaths,
      clipboard
    ]
  )

  return (
    <div
      id="file-explorer-panel"
      ref={containerRef}
      className={cn(
        'relative flex h-full min-w-0 flex-col overflow-hidden bg-sidebar text-foreground',
        fillContainer ? 'w-full' : 'flex-shrink-0'
      )}
      style={fillContainer ? undefined : { width: explorerWidth }}
    >
      {/* Header */}
      <div className="flex h-8 shrink-0 items-center justify-between border-b border-sidebar-border/70 px-2.5">
        <span className="label-section text-sidebar-foreground">{t('fileExplorer.title')}</span>
        <div className="flex items-center gap-0.5">
          <button
            type="button"
            onClick={() => void startHeaderCreate('file')}
            disabled={!rootPath || !!rootLoadError}
            className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-40"
            title={t('fileExplorer.newFile')}
            aria-label={t('fileExplorer.newFile')}
          >
            <FilePlus size={14} />
          </button>
          <button
            type="button"
            onClick={() => void startHeaderCreate('folder')}
            disabled={!rootPath || !!rootLoadError}
            className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-40"
            title={t('fileExplorer.newFolder')}
            aria-label={t('fileExplorer.newFolder')}
          >
            <FolderPlus size={14} />
          </button>
          <button
            type="button"
            onClick={handleHeaderRefresh}
            disabled={!rootPath || !!rootLoadError}
            className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-40"
            title={t('fileExplorer.refresh')}
            aria-label={t('fileExplorer.refresh')}
          >
            <RefreshCw size={14} />
          </button>
          <button
            type="button"
            onClick={collapseAll}
            disabled={!rootPath || !!rootLoadError}
            className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-40"
            title={t('fileExplorer.collapseAll')}
            aria-label={t('fileExplorer.collapseAll')}
          >
            <ChevronsDownUp size={14} />
          </button>
          <button
            type="button"
            onClick={() => {
              void handleHidePanel()
            }}
            className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            title={t('fileExplorer.hidePanel')}
            aria-label={t('fileExplorer.hidePanel')}
          >
            <PanelRightClose size={14} />
          </button>
        </div>
      </div>

      <div className="border-b border-sidebar-border/70 px-2.5 py-1.5">
        <div className="relative">
          <Search
            className="pointer-events-none absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
            aria-hidden="true"
          />
          <input
            type="text"
            value={normalizedSearchQuery}
            onChange={(event) => setSearchQuery(event.target.value)}
            placeholder={t('fileExplorer.searchPlaceholder')}
            className="h-8 w-full rounded-none border-0 bg-transparent pl-7 pr-7 text-xs text-foreground outline-none placeholder:text-muted-foreground/60 focus-visible:ring-1 focus-visible:ring-ring/50"
            aria-label={t('fileExplorer.searchAria')}
          />
          {hasSearchInput && (
            <button
              onClick={() => resetSearch()}
              className="absolute right-0 top-1/2 inline-flex size-7 -translate-y-1/2 items-center justify-center rounded-md text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              title={t('fileExplorer.clearSearch')}
              aria-label={t('fileExplorer.clearSearch')}
            >
              <X className="size-3.5" />
            </button>
          )}
        </div>
      </div>

      {/* Tree / Search Results */}
      <div className="flex-1 overflow-y-auto overflow-x-hidden py-1">
        {roots.length === 0 && !rootPath && (
          <div className="px-3 py-5" role="status">
            <p className="text-xs leading-relaxed text-muted-foreground">
              {t('fileExplorer.noProject')}
            </p>
          </div>
        )}

        {!isMultiRoot && rootPath && rootLoadError && (
          <div className="px-3 py-5" role="alert">
            <p className="text-xs font-medium text-foreground">{t('fileExplorer.loadFailed')}</p>
            <p className="mt-1 break-words text-xs leading-relaxed text-muted-foreground">
              {rootLoadError.message}
            </p>
            <button
              onClick={handleRootRetry}
              className="mt-2 inline-flex h-7 items-center rounded-md px-2 text-xs text-muted-foreground transition-colors duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            >
              {t('fileExplorer.retry')}
            </button>
          </div>
        )}

        {!isMultiRoot && rootPath && !rootEntries && !rootLoadError && (
          <div className="px-3 py-5 text-xs text-muted-foreground" role="status">
            {t('fileExplorer.loading')}
          </div>
        )}

        {!isMultiRoot &&
          rootPath &&
          rootEntries &&
          !rootLoadError &&
          (!isSearchActive ||
            isSearchTooShort ||
            (searchLoading && searchLastCompletedQuery !== trimmedSearchQuery)) &&
          rootEntries.map((entry) => (
            <FileTreeNodeWrapper
              key={entry.path}
              entry={entry}
              depth={0}
              onToggle={toggleDirectory}
              onSelect={handleSelect}
              onContextMenu={handleContextMenu}
              onClick={handleNodeClick}
              renderContextMenu={renderFileTreeContextMenu}
            />
          ))}

        {isMultiRoot &&
          !isSearchActive &&
          roots.map((root) => {
            const entries = directoryContents.get(root.path)
            const error = rootLoadErrors.get(root.path)
            const expanded = expandedDirs.has(root.path)
            const loading = loadingDirs.has(root.path)
            const focused = rootPath === root.path
            return (
              <section
                key={`${root.projectId}:${root.path}`}
                data-testid={`file-root-${root.projectId}`}
              >
                <div
                  className={cn(
                    'group mx-1 flex h-7 min-w-0 items-center gap-1 rounded-sm px-1.5 text-xs transition-colors duration-150 ease-[var(--ease-out)]',
                    focused
                      ? 'bg-sidebar-accent text-foreground ring-1 ring-inset ring-primary/35'
                      : 'text-sidebar-foreground hover:bg-sidebar-accent/50'
                  )}
                >
                  <button
                    type="button"
                    className="flex min-w-0 flex-1 items-center gap-1.5 text-left"
                    aria-expanded={expanded}
                    onClick={() => {
                      setFocusedRoot(root.path)
                      void toggleDirectory(root.path)
                    }}
                  >
                    <span className="flex size-4 shrink-0 items-center justify-center text-muted-foreground">
                      {loading ? (
                        <LoaderCircle size={12} className="animate-spin" />
                      ) : expanded ? (
                        <ChevronDown size={12} />
                      ) : (
                        <ChevronRight size={12} />
                      )}
                    </span>
                    <FolderGit2 size={13} className="shrink-0 text-muted-foreground" />
                    <span className="min-w-0 flex-1 truncate leading-4">{root.name}</span>
                  </button>
                  <button
                    type="button"
                    className="inline-flex size-5 shrink-0 items-center justify-center rounded-md text-muted-foreground opacity-0 transition-opacity hover:bg-sidebar-accent hover:text-foreground group-hover:opacity-100 focus:opacity-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                    title={t('tabs.terminal')}
                    aria-label={`${t('tabs.terminal')}: ${root.name}`}
                    onClick={() => void handleOpenRootTerminal(root.projectId, root.path)}
                  >
                    <SquareTerminal size={13} />
                  </button>
                </div>

                {error ? (
                  <div className="px-3 py-3" role="alert">
                    <p className="text-xs leading-relaxed text-muted-foreground">{error.message}</p>
                    <button
                      type="button"
                      className="mt-1.5 text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
                      onClick={() => void toggleDirectory(root.path)}
                    >
                      {t('fileExplorer.retry')}
                    </button>
                  </div>
                ) : expanded && entries ? (
                  entries.map((entry) => (
                    <FileTreeNodeWrapper
                      key={entry.path}
                      entry={entry}
                      depth={1}
                      onToggle={toggleDirectory}
                      onSelect={handleSelect}
                      onContextMenu={handleContextMenu}
                      onClick={handleNodeClick}
                      renderContextMenu={renderFileTreeContextMenu}
                    />
                  ))
                ) : !entries && loading ? (
                  <div className="px-3 py-2 text-xs text-muted-foreground">
                    {t('fileExplorer.loading')}
                  </div>
                ) : null}
              </section>
            )
          })}

        {rootPath && !rootLoadError && isSearchActive && (
          <div className="space-y-1 px-1 py-1">
            {(searchLoading ||
              searchError ||
              isSearchTooShort ||
              showSearchEmptyState ||
              !resultsAreCurrent) && (
              <div className="px-2 py-1.5 text-3xs leading-relaxed text-muted-foreground">
                <p className="text-3xs font-medium text-foreground">
                  {searchLoading
                    ? t('fileExplorer.searching', { query: trimmedSearchQuery })
                    : hasPartialSearchError
                      ? t('fileExplorer.partialResults', { query: trimmedSearchQuery })
                      : searchError
                        ? t('fileExplorer.searchUnavailable')
                        : isSearchTooShort
                          ? t('fileExplorer.keepTyping')
                          : showSearchEmptyState
                            ? t('fileExplorer.noMatches', { query: trimmedSearchQuery })
                            : t('fileExplorer.updatingResults', { query: trimmedSearchQuery })}
                </p>
                <p className="mt-0.5">
                  {hasPartialSearchError
                    ? t('fileExplorer.partialDescription', { error: searchError })
                    : searchError
                      ? searchError
                      : isSearchTooShort
                        ? t('fileExplorer.minimumCharacters')
                        : showSearchEmptyState
                          ? t('fileExplorer.broadenSearch')
                          : t('fileExplorer.finishingSearch')}
                </p>
              </div>
            )}

            {(searchTruncated || searchFailedFiles > 0) && (
              <div className="px-2 py-1.5 text-3xs leading-relaxed text-muted-foreground">
                {searchTruncated ? t('fileExplorer.truncated') : t('fileExplorer.someFilesFailed')}
                {searchFailedFiles > 0
                  ? ` ${t('fileExplorer.filesSkipped', { count: searchFailedFiles })}`
                  : ''}
                {searchScannedFiles > 0
                  ? ` ${t('fileExplorer.filesScanned', { count: searchScannedFiles })}`
                  : ''}
              </div>
            )}

            {hasAnySearchResults && (
              <div
                className="grid grid-cols-2"
                role="tablist"
                aria-label={t('fileExplorer.resultTypes')}
              >
                <button
                  onClick={() => {
                    userSelectedTabRef.current = true
                    setSearchResultTab('content')
                  }}
                  className={cn(
                    'flex h-7 items-center justify-center gap-1 border-b px-2 text-3xs font-medium transition-colors duration-150 ease-[var(--ease-out)]',
                    searchResultTab === 'content'
                      ? 'border-primary/50 text-foreground'
                      : 'border-sidebar-border/70 text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground'
                  )}
                  type="button"
                  role="tab"
                  aria-selected={searchResultTab === 'content'}
                >
                  {searchLoading && <LoaderCircle size={10} className="animate-spin" />}
                  {t('fileExplorer.content')}{' '}
                  <span className="text-muted-foreground">{safeSearchResults.length}</span>
                </button>
                <button
                  onClick={() => {
                    userSelectedTabRef.current = true
                    setSearchResultTab('files')
                  }}
                  className={cn(
                    'flex h-7 items-center justify-center gap-1 border-b px-2 text-3xs font-medium transition-colors duration-150 ease-[var(--ease-out)]',
                    searchResultTab === 'files'
                      ? 'border-primary/50 text-foreground'
                      : 'border-sidebar-border/70 text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground'
                  )}
                  type="button"
                  role="tab"
                  aria-selected={searchResultTab === 'files'}
                >
                  {searchLoading && <LoaderCircle size={10} className="animate-spin" />}
                  {t('fileExplorer.files')}{' '}
                  <span className="text-muted-foreground">
                    {fileNameMatchesPending ? '…' : safeSearchFileNameMatches.length}
                  </span>
                </button>
              </div>
            )}

            {searchResultTab === 'files' && hasFileResults && (
              <div>
                {safeSearchFileNameMatches.map((filePath) => {
                  const { fileName, relativePath } = getFileLabel(filePath)
                  return (
                    <button
                      key={`fname:${filePath}`}
                      onClick={() => void handleSearchMatchClick(filePath, 1)}
                      className="flex h-7 w-full min-w-0 items-center px-2 text-left transition-colors duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent/50 focus:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring"
                      title={filePath}
                    >
                      <span className="flex min-w-0 flex-1 items-center gap-1.5">
                        <span className="truncate text-xs text-foreground">{fileName}</span>
                        <span className="min-w-0 truncate text-2xs text-muted-foreground">
                          {relativePath}
                        </span>
                      </span>
                    </button>
                  )
                })}
              </div>
            )}

            {searchResultTab === 'content' && hasContentResults && (
              <div>
                {safeSearchResults.map((fileResult) => {
                  const { fileName, relativePath } = getFileLabel(fileResult.filePath)
                  const isExpanded = expandedSearchResultPaths.has(fileResult.filePath)
                  const visibleMatches = isExpanded
                    ? fileResult.matches
                    : fileResult.matches.slice(0, 3)
                  const hiddenCount = Math.max(fileResult.matches.length - visibleMatches.length, 0)
                  return (
                    <div key={fileResult.filePath}>
                      <button
                        onClick={() =>
                          void handleSearchMatchClick(
                            fileResult.filePath,
                            fileResult.matches[0]?.lineNumber ?? 1
                          )
                        }
                        className="flex h-7 w-full min-w-0 items-center px-2 text-left transition-colors duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent/50 focus:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring"
                        title={fileResult.filePath}
                      >
                        <span className="flex min-w-0 flex-1 items-center gap-1.5">
                          <span className="truncate text-xs text-foreground">{fileName}</span>
                          <span className="min-w-0 truncate text-2xs text-muted-foreground">
                            {relativePath}
                          </span>
                        </span>
                        <span className="ml-2 shrink-0 text-2xs text-muted-foreground">
                          {t('fileExplorer.hits', { count: fileResult.matches.length })}
                        </span>
                      </button>
                      <div className="pb-0.5">
                        {visibleMatches.map((match, idx) => (
                          <button
                            key={`${fileResult.filePath}:${match.lineNumber}:${idx}`}
                            onClick={() =>
                              void handleSearchMatchClick(fileResult.filePath, match.lineNumber)
                            }
                            className="group flex h-7 w-full items-center gap-2 overflow-hidden px-2 text-left text-3xs text-foreground transition-colors duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent/50 focus:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring"
                          >
                            <span className="w-5 shrink-0 text-right text-2xs tabular-nums text-muted-foreground">
                              {match.lineNumber}
                            </span>
                            <span className="block min-w-0 flex-1 truncate text-foreground/90">
                              {renderHighlightedLine(match.lineText)}
                            </span>
                          </button>
                        ))}
                        {hiddenCount > 0 && (
                          <button
                            type="button"
                            onClick={() => toggleExpandedSearchResult(fileResult.filePath)}
                            className="flex h-7 items-center px-2 text-3xs text-muted-foreground transition-colors hover:text-foreground focus:outline-none"
                          >
                            {t('fileExplorer.showMore', { count: hiddenCount })}
                          </button>
                        )}
                        {isExpanded && fileResult.matches.length > 3 && (
                          <button
                            type="button"
                            onClick={() => toggleExpandedSearchResult(fileResult.filePath)}
                            className="flex h-7 items-center px-2 text-3xs text-muted-foreground transition-colors hover:text-foreground focus:outline-none"
                          >
                            {t('fileExplorer.showLess')}
                          </button>
                        )}
                      </div>
                    </div>
                  )
                })}
              </div>
            )}
          </div>
        )}

        {/* Inline input for new file/folder/rename */}
        {inlineInput && (
          <div className="flex items-center px-2 py-0.5" style={{ paddingLeft: 20 }}>
            <input
              ref={inputRef}
              type="text"
              value={inputValue}
              onChange={(e) => setInputValue(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault()
                  void handleInlineInputSubmit().catch((error) => {
                    console.error('Inline input submit failed:', error)
                  })
                } else if (e.key === 'Escape') {
                  handleInlineInputCancel()
                }
              }}
              onBlur={handleInlineInputCancel}
              className="h-7 min-w-0 flex-1 bg-transparent px-1.5 text-xs text-foreground outline-none ring-1 ring-inset ring-ring/50"
              placeholder={
                inlineInput.mode === 'create'
                  ? inlineInput.type === 'file'
                    ? t('fileExplorer.fileNamePlaceholder')
                    : t('fileExplorer.folderNamePlaceholder')
                  : t('fileExplorer.newNamePlaceholder')
              }
            />
          </div>
        )}
      </div>

      {fillContainer ? null : (
        <button
          type="button"
          onMouseDown={handleResizeMouseDown}
          onKeyDown={handleResizeKeyDown}
          className={`absolute ${side === 'right' ? 'left-0' : 'right-0'} top-0 h-full w-1 cursor-col-resize bg-transparent hover:bg-ring/25 focus-visible:bg-ring/30 focus-visible:outline-none`}
          title={t('fileExplorer.resizeTitle')}
          aria-label={t('fileExplorer.resizeAria')}
          role="separator"
          aria-controls="file-explorer-panel"
          aria-valuenow={explorerWidth}
          aria-valuemin={220}
          aria-valuemax={560}
        />
      )}

      {/* Delete Confirmation Dialog */}
      {deleteConfirm && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-[2px]">
          <div
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="file-explorer-delete-title"
            className="w-full max-w-sm rounded-md border border-destructive/30 bg-card p-4 shadow-[0_18px_60px_hsl(var(--background)/0.7),inset_0_1px_0_0_hsl(var(--foreground)/0.05)]"
          >
            <p id="file-explorer-delete-title" className="mb-4 text-sm text-foreground">
              {deleteConfirm.length === 1
                ? t('fileExplorer.deleteConfirm', { name: deleteConfirm[0].name })
                : t('fileExplorer.deleteConfirmMany', { count: deleteConfirm.length })}
            </p>
            <div className="flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setDeleteConfirm(null)}
                className="inline-flex h-8 items-center rounded-md bg-secondary/50 px-3 text-xs font-medium text-foreground transition-colors hover:bg-secondary focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {t('fileExplorer.cancel')}
              </button>
              <button
                type="button"
                onClick={handleDeleteConfirm}
                className="inline-flex h-8 items-center rounded-md bg-destructive px-3 text-xs font-medium text-destructive-foreground transition-colors hover:bg-destructive/90 focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {t('fileExplorer.delete')}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
