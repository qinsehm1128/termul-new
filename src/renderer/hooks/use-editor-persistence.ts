import { useEffect, useRef } from 'react'
import { persistenceApi } from '@/lib/api'
import { randomUUID } from '@/lib/uuid'
import { useBrowserSessionStore } from '@/stores/browser-session-store'
import { useConversationStore } from '@/stores/conversation-store'
import type { EditorFileState } from '@/stores/editor-store'
import { useEditorStore } from '@/stores/editor-store'
import { useFileExplorerStore } from '@/stores/file-explorer-store'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { setManifestRestoreInProgress } from '@/stores/workspace-manifest-sync-store'
import type { WorkspaceTab } from '@/stores/workspace-store'
import {
  browserTabId,
  editorTabId,
  findPaneById,
  getAllLeafPanes,
  terminalTabId,
  useWorkspaceStore
} from '@/stores/workspace-store'
import type { Terminal } from '@/types/project'
import type { PaneDirection, PaneNode, SplitNode } from '@/types/workspace.types'
import type { PersistedTerminalLayout } from '../../shared/types/persistence.types'
import { loadWorkspaceManifest } from './use-workspace-manifest-sync'
import { loadPersistedTerminals } from './useTerminalAutoSave'

interface PersistedEditorFile {
  filePath: string
  cursorPosition: { line: number; col: number }
  scrollTop: number
  viewMode: 'code' | 'markdown'
  isDirty: boolean
  draftContent?: string
  lastModified: number
}

// Serialized pane tree for persistence
interface PersistedEditorTabRef {
  type: 'editor'
  filePath: string
}

interface PersistedTerminalTabRef {
  type: 'terminal'
  terminalId: string
}

interface PersistedBrowserTabRef {
  type: 'browser'
  browserTabId: string
  url?: string
}

interface PersistedGitTabRef {
  type: 'git'
  id: string
  cwd: string
}

interface PersistedAgentChatTabRef {
  type: 'agent-chat'
  id: string
  conversationId?: string
  sessionId?: string
}

interface PersistedGitHistoryTabRef {
  type: 'git-history'
  id: string
  cwd: string
}

type PersistedTabRef =
  | PersistedEditorTabRef
  | PersistedTerminalTabRef
  | PersistedBrowserTabRef
  | PersistedGitTabRef
  | PersistedAgentChatTabRef
  | PersistedGitHistoryTabRef

interface PersistedLeafNode {
  type: 'leaf'
  id: string
  tabs: PersistedTabRef[]
  activeTabId: string | null
}

interface PersistedSplitNode {
  type: 'split'
  id: string
  direction: PaneDirection
  children: PersistedPaneNode[]
  sizes: number[]
}

interface LegacyPersistedLeafNode {
  type: 'leaf'
  id: string
  editorFilePaths: string[]
  activeTabId: string | null
}

type PersistedPaneNode = PersistedLeafNode | PersistedSplitNode

type PersistedPaneNodeInput = PersistedPaneNode | LegacyPersistedLeafNode

interface PersistedEditorState {
  openFiles: PersistedEditorFile[]
  activeFilePath: string | null
  expandedDirs: string[]
  activeTabId: string | null
  // v2: pane layout
  paneLayout?: PersistedPaneNodeInput
  activePaneId?: string
}

function editorStateKey(projectId: string): string {
  return `editor-state/${projectId}`
}

const projectWorkspaceRestoredListeners = new Set<(projectId: string) => void>()

/** Fires after a project pane restore finishes so the workspace can spawn a terminal. */
export function subscribeProjectWorkspaceRestored(
  listener: (projectId: string) => void
): () => void {
  projectWorkspaceRestoredListeners.add(listener)
  return () => {
    projectWorkspaceRestoredListeners.delete(listener)
  }
}

function notifyProjectWorkspaceRestored(projectId: string): void {
  for (const listener of projectWorkspaceRestoredListeners) listener(projectId)
}

function normalizePath(path: string): string {
  return path.replace(/\\/g, '/')
}

function filterExpandedDirsByRoots(expandedDirs: string[], rootPaths: readonly string[]): string[] {
  if (rootPaths.length === 0) {
    return []
  }

  const normalizedRoots = rootPaths.map(normalizePath)
  return expandedDirs
    .map((dir) => normalizePath(dir))
    .filter((dir) => normalizedRoots.some((root) => dir === root || dir.startsWith(`${root}/`)))
}

async function loadPersistedTerminalsForProjects(
  projectIds: readonly string[]
): Promise<PersistedTerminalLayout | null> {
  const layouts = await Promise.all(
    projectIds.map((projectId) => loadPersistedTerminals(projectId))
  )
  const available = layouts.filter((layout): layout is PersistedTerminalLayout => layout !== null)
  if (available.length === 0) return null
  return {
    activeTerminalId: available.find((layout) => layout.activeTerminalId)?.activeTerminalId ?? null,
    terminals: available.flatMap((layout) => layout.terminals),
    updatedAt: available.reduce(
      (latest, layout) => (layout.updatedAt > latest ? layout.updatedAt : latest),
      available[0].updatedAt
    )
  }
}

// Serialize pane tree for persistence with both editor and terminal tabs
function serializePaneTree(node: PaneNode): PersistedPaneNode {
  if (node.type === 'leaf') {
    const tabs: PersistedTabRef[] = node.tabs.flatMap((tab): PersistedTabRef[] => {
      if (tab.type === 'editor') {
        return [{ type: 'editor', filePath: tab.filePath }]
      }

      if (tab.type === 'terminal') {
        return [{ type: 'terminal', terminalId: tab.terminalId }]
      }

      if (tab.type === 'browser') {
        const browserTab = useBrowserSessionStore.getState().tabs.get(tab.browserTabId)
        return [{ type: 'browser', browserTabId: tab.browserTabId, url: browserTab?.url }]
      }

      if (tab.type === 'git') {
        return [{ type: 'git', id: tab.id, cwd: tab.cwd }]
      }

      if (tab.type === 'agent-chat') {
        // The session itself is persisted separately (P5 history); we persist
        // the tab so the pane reappears on restart. The chat shows its closed/
        // empty state until reopened from history.
        return tab.conversationId
          ? [{ type: 'agent-chat', id: tab.id, conversationId: tab.conversationId }]
          : [{ type: 'agent-chat', id: tab.id, sessionId: tab.sessionId }]
      }

      if (tab.type === 'git-history') {
        return [{ type: 'git-history', id: tab.id, cwd: tab.cwd }]
      }

      return []
    })

    return {
      type: 'leaf',
      id: node.id,
      tabs,
      activeTabId: node.activeTabId
    }
  }

  return {
    type: 'split',
    id: node.id,
    direction: node.direction,
    children: node.children.map(serializePaneTree),
    sizes: node.sizes
  }
}

function stripAgentChatTabs(node: PaneNode): PaneNode {
  if (node.type === 'leaf') {
    const tabs = node.tabs.filter((tab) => tab.type !== 'agent-chat')
    const activeTabId = tabs.some((tab) => tab.id === node.activeTabId)
      ? node.activeTabId
      : (tabs[0]?.id ?? null)
    return { ...node, tabs, activeTabId }
  }
  return { ...node, children: node.children.map(stripAgentChatTabs) }
}

function sanitizePaneNode(node: PaneNode): PaneNode | null {
  if (node.type === 'leaf') {
    return node
  }

  // Track original indices to correctly map sizes after filtering
  const survivingEntries = node.children
    .map((child, originalIndex) => ({
      child: sanitizePaneNode(child),
      originalIndex
    }))
    .filter((entry): entry is { child: PaneNode; originalIndex: number } => entry.child !== null)

  if (survivingEntries.length === 0) {
    return null
  }

  if (survivingEntries.length === 1) {
    return survivingEntries[0].child
  }

  const rawSizes = node.sizes
  const validSizes = survivingEntries.map((entry) => {
    const value = rawSizes[entry.originalIndex]
    return typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : 1
  })
  const total = validSizes.reduce((sum, value) => sum + value, 0)

  return {
    ...node,
    children: survivingEntries.map((entry) => entry.child),
    sizes: validSizes.map((value) => (value / total) * 100)
  }
}

function normalizePaneTree(root: PaneNode): PaneNode {
  const normalized = sanitizePaneNode(root)
  if (normalized) {
    return normalized
  }

  return {
    type: 'leaf',
    id: randomUUID(),
    tabs: [],
    activeTabId: null
  }
}

function createTerminalMatcher(
  liveTerminals: Terminal[],
  layout: PersistedTerminalLayout | null
): {
  hasLiveTerminals: boolean
  matchTerminalId: (persistedTerminalId: string) => string | null
} {
  const liveTerminalsById = new Map(liveTerminals.map((terminal) => [terminal.id, terminal]))
  const layoutTerminalsById = new Map(
    layout?.terminals.map((terminal) => [terminal.id, terminal]) ?? []
  )
  const unusedLiveTerminals = [...liveTerminals]

  const consumeLiveTerminal = (terminalId: string): string | null => {
    const match = liveTerminalsById.get(terminalId)
    if (!match) {
      return null
    }

    const index = unusedLiveTerminals.findIndex((terminal) => terminal.id === terminalId)
    if (index >= 0) {
      unusedLiveTerminals.splice(index, 1)
    }
    return match.id
  }

  return {
    hasLiveTerminals: liveTerminals.length > 0,
    matchTerminalId: (persistedTerminalId: string): string | null => {
      const directMatch = consumeLiveTerminal(persistedTerminalId)
      if (directMatch) {
        return directMatch
      }

      const persistedTerminal = layoutTerminalsById.get(persistedTerminalId)
      if (!persistedTerminal) {
        return null
      }

      const exactIndex = unusedLiveTerminals.findIndex((terminal) => {
        return (
          terminal.name === persistedTerminal.name &&
          terminal.shell === persistedTerminal.shell &&
          terminal.cwd === persistedTerminal.cwd
        )
      })
      if (exactIndex >= 0) {
        const [match] = unusedLiveTerminals.splice(exactIndex, 1)
        return match.id
      }

      const nameAndShellIndex = unusedLiveTerminals.findIndex((terminal) => {
        return (
          terminal.name === persistedTerminal.name && terminal.shell === persistedTerminal.shell
        )
      })
      if (nameAndShellIndex >= 0) {
        const [match] = unusedLiveTerminals.splice(nameAndShellIndex, 1)
        return match.id
      }

      const nameOnlyIndex = unusedLiveTerminals.findIndex(
        (terminal) => terminal.name === persistedTerminal.name
      )
      if (nameOnlyIndex >= 0) {
        const [match] = unusedLiveTerminals.splice(nameOnlyIndex, 1)
        return match.id
      }

      return null
    }
  }
}

export function reconcileTerminalTabs(
  root: PaneNode,
  openFilePaths: Set<string>,
  liveTerminals: Terminal[],
  layout: PersistedTerminalLayout | null
): PaneNode {
  const { hasLiveTerminals, matchTerminalId } = createTerminalMatcher(liveTerminals, layout)
  const shouldKeepPersistedTerminalTabs = !hasLiveTerminals && !!layout?.terminals.length

  const visit = (node: PaneNode): PaneNode => {
    if (node.type === 'leaf') {
      const terminalTabIdMap = new Map<string, string>()
      const validTabs = node.tabs.flatMap((tab): WorkspaceTab[] => {
        if (tab.type === 'editor') {
          return openFilePaths.has(tab.filePath) ? [tab] : []
        }

        if (tab.type === 'browser') {
          return [tab]
        }

        if (tab.type === 'git') {
          return [tab]
        }

        if (tab.type === 'agent-chat') {
          return [tab]
        }

        if (tab.type === 'git-history') {
          return [tab]
        }

        if (shouldKeepPersistedTerminalTabs) {
          return [tab]
        }

        const mappedTerminalId = matchTerminalId(tab.terminalId)
        if (!mappedTerminalId) {
          return []
        }

        const mappedTabId = terminalTabId(mappedTerminalId)
        terminalTabIdMap.set(tab.id, mappedTabId)

        return [
          {
            type: 'terminal',
            id: mappedTabId,
            terminalId: mappedTerminalId
          }
        ]
      })

      let activeTabId = node.activeTabId
      if (activeTabId && terminalTabIdMap.has(activeTabId)) {
        activeTabId = terminalTabIdMap.get(activeTabId) ?? activeTabId
      }
      if (activeTabId && !validTabs.some((tab) => tab.id === activeTabId)) {
        activeTabId = validTabs.length > 0 ? validTabs[0].id : null
      }

      return {
        ...node,
        tabs: validTabs,
        activeTabId
      }
    }

    return {
      ...node,
      children: node.children.map(visit)
    } as SplitNode
  }

  return normalizePaneTree(visit(root))
}

// Deserialize pane tree with full tab mapping
export function deserializePaneTree(persisted: PersistedPaneNodeInput): PaneNode {
  if (persisted.type === 'leaf') {
    const tabs: WorkspaceTab[] = ('tabs' in persisted ? persisted.tabs : []).flatMap(
      (tab): WorkspaceTab[] => {
        if (tab.type === 'editor') {
          return [
            {
              type: 'editor',
              id: editorTabId(tab.filePath),
              filePath: tab.filePath
            }
          ]
        }

        if (tab.type === 'browser') {
          const bTabId = browserTabId(tab.browserTabId)
          // Restore browser session entry lazily
          useBrowserSessionStore.getState().createTab(tab.browserTabId, tab.url)
          return [
            {
              type: 'browser',
              id: bTabId,
              browserTabId: tab.browserTabId
            }
          ]
        }

        if (tab.type === 'git') {
          return [
            {
              type: 'git',
              id: tab.id,
              cwd: tab.cwd
            }
          ]
        }

        if (tab.type === 'agent-chat') {
          if (tab.conversationId) {
            return [
              {
                type: 'agent-chat',
                id: tab.id,
                conversationId: tab.conversationId
              }
            ]
          }
          if (tab.sessionId) {
            return [
              {
                type: 'agent-chat',
                id: tab.id,
                sessionId: tab.sessionId
              }
            ]
          }
          return []
        }

        if (tab.type === 'git-history') {
          return [
            {
              type: 'git-history',
              id: tab.id,
              cwd: tab.cwd
            }
          ]
        }

        return [
          {
            type: 'terminal',
            id: terminalTabId(tab.terminalId),
            terminalId: tab.terminalId
          }
        ]
      }
    )

    // Backward-compatibility fallback for legacy pre-release shape.
    if (tabs.length === 0 && 'editorFilePaths' in persisted) {
      persisted.editorFilePaths.forEach((filePath) => {
        tabs.push({
          type: 'editor',
          id: editorTabId(filePath),
          filePath
        })
      })
    }

    return {
      type: 'leaf',
      id: persisted.id,
      tabs,
      activeTabId: persisted.activeTabId
    }
  }

  return {
    type: 'split',
    id: persisted.id,
    direction: persisted.direction,
    children: persisted.children.map(deserializePaneTree),
    sizes: persisted.sizes
  }
}

export interface EditorPersistenceScopeOptions {
  /** Projects whose terminals belong to this pane layout. */
  projectIds?: readonly string[]
  /** Filesystem roots whose expanded-directory state may be restored. */
  rootPaths?: readonly string[]
  /** Host manifests remain project-only; group scopes disable this bridge. */
  manifestProjectId?: string | null
  /** Project id emitted to project-entry listeners after restoration. */
  notificationProjectId?: string
}

export function useEditorPersistence(
  projectId: string,
  options: EditorPersistenceScopeOptions = {}
): void {
  const isRestoringRef = useRef(false)
  const prevProjectIdRef = useRef('')
  const restoreRunIdRef = useRef(0)
  const projectIdsKey = (options.projectIds ?? [projectId]).join('\u0000')
  const rootPathsKey = (options.rootPaths ?? []).join('\u0000')
  const manifestProjectId =
    options.manifestProjectId === undefined ? projectId : options.manifestProjectId
  const notificationProjectId = options.notificationProjectId ?? projectId

  // Restore state when project changes
  useEffect(() => {
    if (!projectId || projectId === prevProjectIdRef.current) return
    const oldProjectId = prevProjectIdRef.current
    prevProjectIdRef.current = projectId

    const restoreRunId = ++restoreRunIdRef.current
    let cancelled = false
    const isStale = (): boolean => {
      return (
        cancelled ||
        restoreRunIdRef.current !== restoreRunId ||
        prevProjectIdRef.current !== projectId
      )
    }

    async function restore(): Promise<void> {
      const scopedProjectIds = projectIdsKey ? projectIdsKey.split('\u0000') : []
      const scopedRootPaths =
        rootPathsKey.length > 0
          ? rootPathsKey.split('\u0000')
          : scopedProjectIds.flatMap((id) => {
              const path = useProjectStore
                .getState()
                .projects.find((project) => project.id === id)?.path
              return path ? [path] : []
            })
      isRestoringRef.current = true
      // Guard the manifest writer for the entire restore window so a half-built
      // tree (mid open-files loop, mid pane rebuild) is never persisted as the
      // new host manifest. The terminal-restore guard already covers PTY
      // reattachment; this mirrors it for the manifest's portable projection.
      if (manifestProjectId) setManifestRestoreInProgress(manifestProjectId, true)
      try {
        // Persist old project state before clearing
        if (oldProjectId) {
          persistState(oldProjectId)
        }

        // Clear editor files (in-memory state), but defer workspace pane reset
        // until we know the destination layout to avoid a flash of empty pane.
        useEditorStore.getState().clearAllFiles()

        // Read new project's persisted state
        const result = await persistenceApi.read<PersistedEditorState>(editorStateKey(projectId))

        if (isStale()) {
          return
        }

        if (!result.success || !result.data) {
          // No persisted renderer-local editor state. A host manifest may still
          // exist (cross-client: this client never opened the project before).
          // Consult the manifest; if absent, start fresh.
          const manifestRestored = manifestProjectId
            ? await loadWorkspaceManifest(manifestProjectId)
            : false
          if (isStale()) {
            return
          }
          if (!manifestRestored) {
            useWorkspaceStore.getState().resetLayout()
          }
          return
        }

        const persisted = result.data

        // Restore expanded dirs for this project root
        const explorerStore = useFileExplorerStore.getState()

        const filteredExpandedDirs = filterExpandedDirsByRoots(
          persisted.expandedDirs,
          scopedRootPaths
        )
        explorerStore.setExpandedDirs(new Set(filteredExpandedDirs))

        // Restore open files
        const editorStore = useEditorStore.getState()
        for (const file of persisted.openFiles) {
          if (isStale()) {
            return
          }

          try {
            await editorStore.openFile(file.filePath)
            if (isStale()) {
              return
            }

            editorStore.updateCursorPosition(
              file.filePath,
              file.cursorPosition.line,
              file.cursorPosition.col
            )
            editorStore.updateScrollTop(file.filePath, file.scrollTop)
            if (file.viewMode !== 'code') {
              editorStore.setViewMode(file.filePath, file.viewMode)
            }
            if (file.isDirty && file.draftContent) {
              const freshEditorState = useEditorStore.getState()
              const currentState = freshEditorState.openFiles.get(file.filePath)
              if (currentState) {
                if (currentState.lastModified <= file.lastModified) {
                  editorStore.updateContent(file.filePath, file.draftContent)
                }
              }
            }
          } catch {
            // File may have been deleted since last session
          }
        }

        if (isStale()) {
          return
        }

        // Restore active file
        if (persisted.activeFilePath) {
          editorStore.setActiveFilePath(persisted.activeFilePath)
        }

        // Restore pane layout. The manifest is authoritative for portable
        // topology (terminalIds + editorIds + activeTabId + activePaneId). If a
        // manifest exists it wins; else fall back to editorStateKey.paneLayout
        // (legacy path carrying all tab variants incl. browser/git/agent-chat).
        // The manifest load runs AFTER the open-files loop above so renderer-
        // local editor state (drafts/scroll/cursor) is reconciled with the
        // manifest topology. Non-portable tab variants are absent on manifest
        // restore (CAP-5 contract decision) — they survive only in the legacy
        // editorStateKey.paneLayout path. The manifestRestoreInProgress guard
        // set at the top of restore() covers this load + tree build.
        const manifestRestored = manifestProjectId
          ? await loadWorkspaceManifest(manifestProjectId)
          : false
        if (isStale()) {
          return
        }

        if (!manifestRestored) {
          // No manifest (or load failed — logged + degraded gracefully).
          // Fall back to the existing renderer-local paneLayout path.
          if (persisted.paneLayout) {
            const restoredTree = stripAgentChatTabs(deserializePaneTree(persisted.paneLayout))
            const openFilePaths = new Set(useEditorStore.getState().openFiles.keys())
            const liveProjectTerminals = useTerminalStore
              .getState()
              .terminals.filter(
                (terminal) =>
                  scopedProjectIds.includes(terminal.projectId ?? '') && !!terminal.ptyId
              )
            const persistedTerminalLayout =
              await loadPersistedTerminalsForProjects(scopedProjectIds)
            if (isStale()) {
              return
            }

            const cleanTree = reconcileTerminalTabs(
              restoredTree,
              openFilePaths,
              liveProjectTerminals,
              persistedTerminalLayout
            )
            useWorkspaceStore.getState().loadProjectWorkspace(cleanTree, persisted.activePaneId)
          } else {
            // Legacy fallback: build a fresh layout with editor tabs
            useWorkspaceStore.getState().resetLayout()
            const openFilePaths = Array.from(useEditorStore.getState().openFiles.keys())
            useWorkspaceStore.getState().syncEditorTabs(openFilePaths, persisted.activeTabId)
          }
        }

        // Restore expanded directory tree after root initialization.
        await explorerStore.restoreExpandedDirs(filteredExpandedDirs)
        if (isStale()) {
          return
        }
      } finally {
        if (restoreRunIdRef.current === restoreRunId) {
          isRestoringRef.current = false
          if (manifestProjectId) setManifestRestoreInProgress(manifestProjectId, false)
          if (!cancelled && prevProjectIdRef.current === projectId) {
            notifyProjectWorkspaceRestored(notificationProjectId)
          }
        } else if (prevProjectIdRef.current !== projectId) {
          // Superseded by a run for a DIFFERENT project: that run owns its own
          // guard key, so this project's key would otherwise stay `true`
          // forever (blocking manifest writes for it until the user returns).
          // The same-project supersession is still owned by the newer run
          // (handled by the branch above when it completes).
          if (manifestProjectId) setManifestRestoreInProgress(manifestProjectId, false)
        }
      }
    }

    void restore()

    return () => {
      cancelled = true
    }
  }, [projectId, projectIdsKey, rootPathsKey, manifestProjectId, notificationProjectId])

  // Save state on changes (debounced) - coalesced across all store subscriptions
  useEffect(() => {
    if (!projectId) return

    let persistTimeoutId: ReturnType<typeof setTimeout> | null = null

    const schedulePersist = (): void => {
      if (isRestoringRef.current) return
      if (useConversationStore.getState().activeConversationId) return
      if (persistTimeoutId) clearTimeout(persistTimeoutId)
      persistTimeoutId = setTimeout(() => {
        persistState(projectId)
      }, 500)
    }

    const unsubEditor = useEditorStore.subscribe(schedulePersist)
    const unsubExplorer = useFileExplorerStore.subscribe((state, prevState) => {
      if (state.expandedDirs !== prevState.expandedDirs) {
        schedulePersist()
      }
    })
    const unsubWorkspace = useWorkspaceStore.subscribe(schedulePersist)
    const unsubBrowserSessions = useBrowserSessionStore.subscribe((state, prevState) => {
      if (state.tabs !== prevState.tabs) {
        schedulePersist()
      }
    })

    return () => {
      unsubEditor()
      unsubExplorer()
      unsubWorkspace()
      unsubBrowserSessions()
      if (persistTimeoutId) clearTimeout(persistTimeoutId)
    }
  }, [projectId])
}

async function restoreWorkspaceScope(
  scopeKey: string,
  projectIds: readonly string[],
  manifestProjectId?: string
): Promise<boolean> {
  if (!scopeKey) return false
  const result = await persistenceApi.read<PersistedEditorState>(editorStateKey(scopeKey))
  if (result.success && result.data?.paneLayout) {
    const restoredTree = stripAgentChatTabs(deserializePaneTree(result.data.paneLayout))
    const openFilePaths = new Set(useEditorStore.getState().openFiles.keys())
    const liveProjectTerminals = useTerminalStore
      .getState()
      .terminals.filter(
        (terminal) => projectIds.includes(terminal.projectId ?? '') && !!terminal.ptyId
      )
    const persistedTerminalLayout = await loadPersistedTerminalsForProjects(projectIds)
    const cleanTree = reconcileTerminalTabs(
      restoredTree,
      openFilePaths,
      liveProjectTerminals,
      persistedTerminalLayout
    )
    useWorkspaceStore.getState().loadProjectWorkspace(cleanTree, result.data.activePaneId)
    return getAllLeafPanes(cleanTree).some((leaf) => leaf.tabs.length > 0)
  }
  return manifestProjectId ? loadWorkspaceManifest(manifestProjectId) : false
}

/** Reload the project pane tree after leaving a Conversation workspace. */
export async function restoreProjectWorkspace(projectId: string): Promise<boolean> {
  return restoreWorkspaceScope(projectId, [projectId], projectId)
}

// `restoreProjectGroupWorkspace` lived here and read `editor-state/group-<id>`.
// Removed with the group-scoped workspace itself: leaving a reader for a key
// nothing writes any more is how the second tab set would come back.

export function persistState(projectId: string): void {
  if (useConversationStore.getState().activeConversationId) return
  const editorState = useEditorStore.getState()
  const explorerState = useFileExplorerStore.getState()
  const workspaceState = useWorkspaceStore.getState()

  const openFiles: PersistedEditorFile[] = []
  editorState.openFiles.forEach((file: EditorFileState) => {
    const persisted: PersistedEditorFile = {
      filePath: file.filePath,
      cursorPosition: file.cursorPosition,
      scrollTop: file.scrollTop,
      viewMode: file.viewMode,
      isDirty: file.isDirty,
      lastModified: file.lastModified
    }
    if (file.isDirty) {
      persisted.draftContent = file.content
    }
    openFiles.push(persisted)
  })

  const expandedDirs = Array.from(explorerState.expandedDirs)

  const data: PersistedEditorState = {
    openFiles,
    activeFilePath: editorState.activeFilePath,
    expandedDirs,
    activeTabId: (() => {
      const pane = findPaneById(workspaceState.root, workspaceState.activePaneId)
      return pane && pane.type === 'leaf' ? pane.activeTabId : null
    })(),
    paneLayout: serializePaneTree(workspaceState.root),
    activePaneId: workspaceState.activePaneId
  }

  persistenceApi.writeDebounced(editorStateKey(projectId), data)
}
