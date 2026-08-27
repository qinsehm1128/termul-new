import type { ShellInfo } from '@shared/types/ipc.types'
import type { SFTPEntry } from '@shared/types/ssh.types'
import { motion } from 'framer-motion'
import { FolderGit2, SquareTerminal, X } from 'lucide-react'
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Outlet, useLocation, useNavigate } from 'react-router-dom'
import { toast } from 'sonner'
import { ActivityRail } from '@/components/ActivityRail'
import { AgentLauncher } from '@/components/agents/AgentLauncher'
import { ConfirmDialog } from '@/components/ConfirmDialog'
import { CreateSnapshotModal } from '@/components/CreateSnapshotModal'
import { ConversationSidebar } from '@/components/conversation/ConversationSidebar'
import { ImportEditorWorkspacesDialog } from '@/components/ImportEditorWorkspacesDialog'
import { NewProjectModal } from '@/components/NewProjectModal'
import { ProjectSidebar } from '@/components/ProjectSidebar'
import { ResizeEdges } from '@/components/ResizeEdges'
import { StatusBar } from '@/components/StatusBar'
import { TitleBar } from '@/components/TitleBar'
import {
  CliSessionPanelToggleButton,
  FileExplorerToggleButton,
  SidebarToggleButton,
  titlebarNoDragStyle
} from '@/components/TitlebarPanelToggles'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle
} from '@/components/ui/dialog'
import { PanelFade } from '@/components/ui/panel-fade'
import { Sheet, SheetContent } from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { HiddenTerminalPopover } from '@/components/workspace/HiddenTerminalPopover'
import { LastSessionNotice } from '@/components/workspace/LastSessionNotice'
import { PanelResizeHandle } from '@/components/workspace/PanelResizeHandle'
import { PaneRenderer } from '@/components/workspace/PaneRenderer'
import { ResizableRail } from '@/components/workspace/ResizableRail'
import { TerminalListPanel } from '@/components/workspace/TerminalListPanel'
import { TerminalSwitcherBar } from '@/components/workspace/TerminalSwitcherBar'
import { WorkspaceConflictBanner } from '@/components/workspace/WorkspaceConflictBanner'
import {
  useUpdateAppSetting,
  useUpdatePanelVisibility,
  waitForPendingAppSettingsPersistence
} from '@/hooks/use-app-settings'
import {
  useAllCommandHistory,
  useCommandHistory,
  useCommandHistoryLoader
} from '@/hooks/use-command-history'
import {
  persistState,
  restoreProjectGroupWorkspace,
  restoreProjectWorkspace,
  subscribeProjectWorkspaceRestored,
  useEditorPersistence
} from '@/hooks/use-editor-persistence'
import { useFileWatcher } from '@/hooks/use-file-watcher'
import { useMobileWebShell } from '@/hooks/use-mobile-web-shell'
import { PaneDndProvider } from '@/hooks/use-pane-dnd'
import { usePersistedPanelSize } from '@/hooks/use-persisted-panel-size'
import { usePinnedCommandsLoader } from '@/hooks/use-pinned-commands'
import { useRecentCommandsLoader } from '@/hooks/use-recent-commands'
import { useSessionWorkspaceSync } from '@/hooks/use-session-workspace-sync'
import { useCreateSnapshot, useSnapshotLoader } from '@/hooks/use-snapshots'
import { useSSHConnection } from '@/hooks/use-ssh-connection'
import { useWorktreeShortcuts } from '@/hooks/use-worktree-shortcuts'
import { saveTerminalLayout } from '@/hooks/useTerminalAutoSave'
import { runtimeT } from '@/i18n/runtime'
import { flushSessionHistory, waitForPendingSessionIndexWrite } from '@/lib/acp-history-persistence'
import { launchAgentInPane } from '@/lib/agent-launch'
import { BUILT_IN_AGENTS } from '@/lib/agents/agent-registry'
import { loadCustomAgents } from '@/lib/agents/custom-agents'
import {
  filesystemApi,
  keyboardApi,
  persistenceApi,
  sshApi,
  terminalApi,
  windowApi
} from '@/lib/api'
import { browserTabHide, browserTabShow } from '@/lib/browser-api'
import { getColorClasses } from '@/lib/colors'
import { isSaveFileShortcut, requestSaveEditorFile } from '@/lib/editor-save'
import { logFrontendError } from '@/lib/log-api'
import { isMac, macOsTitlebarStripClass } from '@/lib/platform'
import { isConversationAreaPath, setRouterNavigate } from '@/lib/router-navigate'
import { listen, type UnlistenFn } from '@/lib/tauri-event'
import {
  applyPendingTerminalFocus,
  openBoardTerminal,
  peekPendingTerminalFocus
} from '@/lib/terminal-board-navigation'
import { spawnTerminalInPane } from '@/lib/terminal-spawn'
import { getEffectiveThemeId } from '@/lib/themes'
import { cn } from '@/lib/utils'
import { randomUUID } from '@/lib/uuid'
import { getDefaultCwdForProject } from '@/lib/worktree-context'
import { useAcpStore } from '@/stores/acp-store'
import {
  useAppearanceMode,
  useColorTheme,
  useConfirmTerminalClose,
  useDefaultShell,
  useMaxTerminalsPerProject,
  useUiZoomLevel
} from '@/stores/app-settings-store'
import { useBrowserSessionStore } from '@/stores/browser-session-store'
import { useCliSessionPanelVisible } from '@/stores/cli-session-panel-store'
import { useCommandHistoryStore } from '@/stores/command-history-store'
import { useConversationStore } from '@/stores/conversation-store'
import { useEditorStore } from '@/stores/editor-store'
import { useFileExplorerStore, useFileExplorerVisible } from '@/stores/file-explorer-store'
import { matchesShortcut, useKeyboardShortcutsStore } from '@/stores/keyboard-shortcuts-store'
import { useLastSessionStore } from '@/stores/last-session-store'
import {
  useActiveProject,
  useActiveProjectId,
  useProjectActions,
  useProjectStore,
  useProjects,
  useProjectsLoaded
} from '@/stores/project-store'
import { useSidebarVisible } from '@/stores/sidebar-store'
import {
  useActiveSSHProfile,
  useActiveSSHProfileId,
  useSSHActions,
  useSSHProfiles,
  useSSHStore
} from '@/stores/ssh-store'
import { useTerminalListPanelVisible } from '@/stores/terminal-list-panel-store'
import {
  useActiveTerminal,
  useActiveTerminalId,
  useTerminalActions,
  useTerminalStore,
  useTerminals
} from '@/stores/terminal-store'
import { useThemePickerOpen, useThemePickerStore } from '@/stores/theme-picker-store'
import {
  editorTabId,
  findPaneById,
  getActiveFilePathFromTree,
  getActiveTerminalIdFromTree,
  getAllLeafPanes,
  useActiveTab,
  useFullscreenPaneId,
  usePaneRoot,
  useWorkspaceStore
} from '@/stores/workspace-store'
import {
  isConversationScopedTerminal,
  isHiddenRunningTerminal,
  isOpenTerminalView
} from '@/types/project'
import { UI_ZOOM_DEFAULT, UI_ZOOM_MAX, UI_ZOOM_MIN, UI_ZOOM_STEP } from '@/types/settings'

const SSHWorkspace = lazy(() =>
  import('@/components/ssh/SSHWorkspace').then((m) => ({ default: m.SSHWorkspace }))
)
const CommandHistoryModal = lazy(() =>
  import('@/components/CommandHistoryModal').then((m) => ({
    default: m.CommandHistoryModal
  }))
)
const TerminalQuickSwitcher = lazy(() =>
  import('@/components/workspace/TerminalQuickSwitcher').then((m) => ({
    default: m.TerminalQuickSwitcher
  }))
)
const CommandPalette = lazy(() =>
  import('@/components/CommandPalette').then((m) => ({ default: m.CommandPalette }))
)
const GitPanel = lazy(() =>
  import('@/components/git/GitPanel').then((m) => ({ default: m.GitPanel }))
)
const ThemePicker = lazy(() =>
  import('@/components/ThemePicker').then((m) => ({ default: m.ThemePicker }))
)
const FileExplorer = lazy(() =>
  import('@/components/file-explorer/FileExplorer').then((m) => ({ default: m.FileExplorer }))
)
const CliSessionPanel = lazy(() =>
  import('@/components/cli-sessions/CliSessionPanel').then((m) => ({ default: m.CliSessionPanel }))
)
const MobileChatShell = lazy(() =>
  import('@/components/mobile/MobileChatShell').then((m) => ({ default: m.MobileChatShell }))
)
const SSHFileExplorer = lazy(() =>
  import('@/components/ssh/SSHFileExplorer').then((m) => ({ default: m.SSHFileExplorer }))
)

/** Lightweight skeleton Suspense fallback for lazy-loaded shell components. */
function ShellSkeleton(): React.JSX.Element {
  return <Skeleton className="h-full w-full" />
}

function getShortcutTargetContext(target: EventTarget | null): {
  isInEditor: boolean
  isInTerminal: boolean
  isInInput: boolean
} {
  const element = target instanceof HTMLElement ? target : document.body
  const isInEditor = !!(element.closest('.cm-content') || element.closest('.bn-editor'))
  const isInTerminal = !!element.closest('.xterm')
  const isInInput =
    !isInTerminal &&
    (element.tagName === 'INPUT' ||
      element.tagName === 'TEXTAREA' ||
      element.isContentEditable ||
      !!element.closest('[contenteditable="true"]'))

  return { isInEditor, isInTerminal, isInInput }
}

/**
 * Width of the draggable spacer that clears the macOS native traffic lights
 * (tauri.conf.json trafficLightPosition x=14; three ~12px lights ~8px apart
 * end near x=66). The spacer is its own drag handle so the clearance area
 * stays a window-drag zone; the toggle sits in a separate no-drag container.
 */
const macOsTrafficLightClearance = 'w-[80px] shrink-0'
function MacOsTitlebarStrip(): React.JSX.Element | null {
  const activeProject = useActiveProject()
  const activeGroupName = useProjectStore((state) => {
    if (!state.activeGroupId) return null
    return state.groups.find((group) => group.id === state.activeGroupId)?.name ?? null
  })

  if (!isMac) return null

  return (
    <div
      className={macOsTitlebarStripClass}
      data-tauri-drag-region
      data-testid="macos-titlebar-strip"
    >
      {/* Draggable spacer clearing the native traffic lights so the area
          left of the sidebar toggle stays a window-drag handle. */}
      <div className={`h-full ${macOsTrafficLightClearance}`} data-tauri-drag-region />

      {/* Left-sidebar toggle — no-drag, sits right of the traffic lights. */}
      <div className="flex items-center h-full" style={titlebarNoDragStyle}>
        <SidebarToggleButton />
      </div>

      {(activeGroupName || activeProject) && (
        <span className="pointer-events-none absolute left-1/2 max-w-[50%] -translate-x-1/2 select-none truncate text-2xs font-medium tracking-[0.01em] text-muted-foreground/80">
          {activeGroupName ?? activeProject?.name}
        </span>
      )}

      <div className="flex-1 h-full" data-tauri-drag-region />

      {/* Right-sidebar (file explorer) toggle — top-right. */}
      <div className="flex items-center h-full" style={titlebarNoDragStyle}>
        <CliSessionPanelToggleButton />
        <FileExplorerToggleButton />
      </div>
    </div>
  )
}

export default function WorkspaceLayout(): React.JSX.Element {
  const { t } = useTranslation('workspace')
  const location = useLocation()
  const navigate = useNavigate()
  const [isNewProjectModalOpen, setIsNewProjectModalOpen] = useState(false)
  const [isImportEditorOpen, setIsImportEditorOpen] = useState(false)
  const [importEditorGroupId, setImportEditorGroupId] = useState<string | null>(null)

  useEffect(() => {
    setRouterNavigate(navigate)
    return () => setRouterNavigate(null)
  }, [navigate])
  // Agent chat entry point (moved from the pane tab bar to the Activity Rail).
  // The dialogs are owned here so the rail button can open them globally; the

  const hiddenBrowserTabForModalRef = useRef<string | null>(null)
  const [isCommandPaletteOpen, setIsCommandPaletteOpen] = useState(false)
  const [isTerminalSwitcherOpen, setIsTerminalSwitcherOpen] = useState(false)
  const [isShortcutMenuOpen, setIsShortcutMenuOpen] = useState(false)
  const [isCreateSnapshotModalOpen, setIsCreateSnapshotModalOpen] = useState(false)
  const [closeConfirmTerminal, setCloseConfirmTerminal] = useState<{
    terminalId: string
    tabId: string
  } | null>(null)
  const [closeConfirmLoading, setCloseConfirmLoading] = useState(false)
  const [closeConfirmRememberChoice, setCloseConfirmRememberChoice] = useState(false)
  const [terminateConfirmTerminal, setTerminateConfirmTerminal] = useState<{
    terminalId: string
    tabId?: string
  } | null>(null)
  const [terminateConfirmLoading, setTerminateConfirmLoading] = useState(false)
  const [closingTerminalIds, setClosingTerminalIds] = useState<string[]>([])
  const [dirtyCloseFilePath, setDirtyCloseFilePath] = useState<string | null>(null)
  const [isCommandHistoryOpen, setIsCommandHistoryOpen] = useState(false)
  const [pendingTerminalRoot, setPendingTerminalRoot] = useState<{
    paneId: string
    shellName?: string
  } | null>(null)
  const [isAppCloseDialogOpen, setIsAppCloseDialogOpen] = useState(false)
  // Mobile-only full-width Sheet rendering GitPanel (single-column mobile branch).
  const [gitSheetOpen, setGitSheetOpen] = useState(false)
  const [appCloseDirtyCount, setAppCloseDirtyCount] = useState(0)

  const isLoaded = useProjectsLoaded()

  // Warm custom-agent cache so tab icons resolve before the launcher opens.
  useEffect(() => {
    void loadCustomAgents()
  }, [])

  const confirmTerminalClose = useConfirmTerminalClose()
  const projects = useProjects()
  const activeProject = useActiveProject()
  const activeGroupId = useProjectStore((state) => state.activeGroupId) ?? null
  const activeProjectId = useActiveProjectId()
  const groups = useProjectStore((state) => state.groups) ?? []
  const activeGroup = useMemo(
    () => groups.find((group) => group.id === activeGroupId),
    [activeGroupId, groups]
  )
  const activeGroupProjects = useMemo(() => {
    if (!activeGroup) return []
    const projectsById = new Map(projects.map((project) => [project.id, project]))
    return activeGroup.projectIds.flatMap((projectId) => {
      const project = projectsById.get(projectId)
      return project && project.isArchived !== true && project.path ? [project] : []
    })
  }, [activeGroup, projects])
  const editorPersistenceScopeKey = activeGroupId ? `group-${activeGroupId}` : activeProjectId
  const activeConversationId = useConversationStore((state) => state.activeConversationId)
  const activeConversation = useConversationStore((state) =>
    activeConversationId ? state.summariesById[activeConversationId] : undefined
  )
  const {
    selectProject,
    selectGroup,
    addProject,
    updateProject,
    deleteProject,
    archiveProject,
    restoreProject,
    reorderProjects,
    updateGroup
  } = useProjectActions()

  const terminals = useTerminals()
  const activeTerminal = useActiveTerminal()
  const activeTerminalId = useActiveTerminalId()
  const {
    addTerminal,
    closeTerminalView,
    terminateTerminalResource,
    restartTerminalResource,
    renameTerminal
  } = useTerminalActions()

  // File explorer & editor state
  const isExplorerVisible = useFileExplorerVisible()
  const isCliSessionPanelVisible = useCliSessionPanelVisible()
  const isTerminalListVisible = useTerminalListPanelVisible()
  const isSidebarVisible = useSidebarVisible()
  const [cliRailHeight, setCliRailHeight] = usePersistedPanelSize(
    'termul:cli-session-rail-height',
    { initial: 260, min: 140, max: 720 }
  )
  const isMobileWebShell = useMobileWebShell()

  // SSH state
  const sshProfiles = useSSHProfiles()
  const { loadProfiles: loadSSHProfiles, selectProfile: selectSSHProfile } = useSSHActions()
  const activeSSHProfileId = useActiveSSHProfileId()
  const activeSSHProfile = useActiveSSHProfile()
  const [sshPasswordPrompt, setSSHPasswordPrompt] = useState<{
    profileId: string
    profileName: string
  } | null>(null)
  const [sshPasswordInput, setSSHPasswordInput] = useState('')
  const [sshPromptPasswords, setSSHPromptPasswords] = useState<Record<string, string>>({})

  const sshProfileWithPassword = activeSSHProfile
    ? {
        ...activeSSHProfile,
        password: sshPromptPasswords[activeSSHProfile.id] ?? activeSSHProfile.password
      }
    : null

  const sshConn = useSSHConnection(sshProfileWithPassword)

  const handleSSHMkdir = useCallback(async () => {
    if (!sshConn.connectionId) return
    const name = prompt(runtimeT('ssh', 'files.newFolderPrompt', 'New folder name:'))
    if (!name) return
    const newPath = sshConn.currentPath.endsWith('/')
      ? `${sshConn.currentPath}${name}`
      : `${sshConn.currentPath}/${name}`
    try {
      const r = await sshApi.sftpMkdir(sshConn.connectionId, newPath)
      if (r.success) {
        toast.success(runtimeT('ssh', 'files.created', 'Created: {{name}}', { name }))
        sshConn.loadDirectory(sshConn.currentPath)
      } else toast.error(runtimeT('ssh', 'files.failed', 'Failed: {{error}}', { error: r.error }))
    } catch (error) {
      toast.error(
        runtimeT('ssh', 'files.failed', 'Failed: {{error}}', {
          error: error instanceof Error ? error.message : String(error)
        })
      )
    }
  }, [sshConn.connectionId, sshConn.currentPath, sshConn.loadDirectory])

  const handleSSHCreateFile = useCallback(async () => {
    if (!sshConn.connectionId) return
    const name = prompt(runtimeT('ssh', 'files.newFilePrompt', 'New file name:'))
    if (!name) return
    const newPath = sshConn.currentPath.endsWith('/')
      ? `${sshConn.currentPath}${name}`
      : `${sshConn.currentPath}/${name}`
    try {
      const r = await sshApi.sftpCreateFile(sshConn.connectionId, newPath)
      if (r.success) {
        toast.success(runtimeT('ssh', 'files.created', 'Created: {{name}}', { name }))
        sshConn.loadDirectory(sshConn.currentPath)
      } else toast.error(runtimeT('ssh', 'files.failed', 'Failed: {{error}}', { error: r.error }))
    } catch (error) {
      toast.error(
        runtimeT('ssh', 'files.failed', 'Failed: {{error}}', {
          error: error instanceof Error ? error.message : String(error)
        })
      )
    }
  }, [sshConn.connectionId, sshConn.currentPath, sshConn.loadDirectory])

  const handleSSHDelete = useCallback(
    async (entry: SFTPEntry) => {
      if (!sshConn.connectionId) return
      if (
        !confirm(
          runtimeT('ssh', 'files.deletePrompt', 'Delete {{type}} "{{name}}"?', {
            type: entry.entryType,
            name: entry.name
          })
        )
      )
        return
      try {
        const r = await sshApi.sftpDelete(sshConn.connectionId, entry.path)
        if (r.success) {
          toast.success(runtimeT('ssh', 'files.deleted', 'Deleted: {{name}}', { name: entry.name }))
          sshConn.loadDirectory(sshConn.currentPath)
        } else
          toast.error(
            runtimeT('ssh', 'files.deleteFailed', 'Delete failed: {{error}}', { error: r.error })
          )
      } catch (error) {
        toast.error(
          runtimeT('ssh', 'files.deleteFailed', 'Delete failed: {{error}}', {
            error: error instanceof Error ? error.message : String(error)
          })
        )
      }
    },
    [sshConn.connectionId, sshConn.currentPath, sshConn.loadDirectory]
  )

  const handleSSHRename = useCallback(
    async (entry: SFTPEntry) => {
      if (!sshConn.connectionId) return
      const newName = prompt(
        runtimeT('ssh', 'files.renamePrompt', 'Rename "{{name}}" to:', { name: entry.name }),
        entry.name
      )
      if (!newName || newName === entry.name) return
      const pp = entry.path.substring(0, entry.path.lastIndexOf('/'))
      try {
        const r = await sshApi.sftpRename(sshConn.connectionId, entry.path, `${pp}/${newName}`)
        if (r.success) {
          toast.success(
            runtimeT('ssh', 'files.renamed', 'Renamed: {{name}} → {{newName}}', {
              name: entry.name,
              newName
            })
          )
          sshConn.loadDirectory(sshConn.currentPath)
        } else
          toast.error(
            runtimeT('ssh', 'files.renameFailed', 'Rename failed: {{error}}', { error: r.error })
          )
      } catch (error) {
        toast.error(
          runtimeT('ssh', 'files.renameFailed', 'Rename failed: {{error}}', {
            error: error instanceof Error ? error.message : String(error)
          })
        )
      }
    },
    [sshConn.connectionId, sshConn.currentPath, sshConn.loadDirectory]
  )

  // Load SSH profiles on mount
  useEffect(() => {
    loadSSHProfiles()
  }, [loadSSHProfiles])

  // Reconcile real SSH connection status from the backend (heartbeat,
  // reconnect, failure). Without this the badge can only ever show the
  // optimistic state set at connect time.
  useEffect(() => {
    if (typeof sshApi?.onConnectionStatusChanged !== 'function') return
    const unlisten = sshApi.onConnectionStatusChanged((connectionId, status, error) => {
      useSSHStore.getState().updateConnectionStatus(connectionId, status, error)
    })
    return () => {
      unlisten?.()
    }
  }, [])

  const handleSelectSSHProfile = useCallback(
    (profileId: string) => {
      selectSSHProfile(profileId)
    },
    [selectSSHProfile]
  )

  // Snapshot what was on disk before this session writes anything back, so a
  // crash or an accidental close leaves something the user can look at.
  useEffect(() => {
    if (!isLoaded || projects.length === 0) return
    void useLastSessionStore.getState().capture(projects)
  }, [isLoaded, projects])

  const handleSelectProject = useCallback(
    (id: string) => {
      useWorkspaceStore.getState().hideAgentLauncher()
      selectProject(id)
      selectSSHProfile(null) // Deselect SSH when switching to project
      // Selecting a project is a request to see that project's folder. The
      // explorer root already follows `activeProjectId`; showing the panel is
      // what turns the click into something visible. A project without a path
      // has no folder to show, so it leaves the panel as it found it.
      if (useProjectStore.getState().projects.find((project) => project.id === id)?.path) {
        useFileExplorerStore.getState().setVisible(true)
      }
      projectTerminalDismissedRef.current = false
      if (location.pathname !== '/') {
        navigate('/')
        return
      }
      void restoreProjectWorkspace(id).then((restored) => {
        useConversationStore.getState().setActiveConversationId(null)
        if (!restored) useWorkspaceStore.getState().resetLayout()
      })
    },
    [location.pathname, navigate, selectProject, selectSSHProfile]
  )
  // Explicit terminal launch from the project row. Selects the project first so
  // the terminal lands in the right one, and passes `explicit` so a user who
  // just closed the last terminal can still ask for another.
  const handleOpenProjectTerminal = useCallback(
    (id: string) => {
      if (useProjectStore.getState().activeProjectId !== id) handleSelectProject(id)
      setTimeout(() => ensureVisibleProjectTerminalRef.current(id, { explicit: true }), 0)
    },
    [handleSelectProject]
  )

  const handleSelectGroup = useCallback(
    (groupId: string) => {
      const group = groups.find((candidate) => candidate.id === groupId)
      if (!group) return
      useWorkspaceStore.getState().hideAgentLauncher()
      selectGroup(groupId)
      selectSSHProfile(null)
      projectTerminalDismissedRef.current = false
      if (location.pathname !== '/') {
        navigate('/')
        return
      }
      void restoreProjectGroupWorkspace(groupId, group.projectIds).then((restored) => {
        useConversationStore.getState().setActiveConversationId(null)
        if (!restored) useWorkspaceStore.getState().resetLayout()
      })
    },
    [groups, location.pathname, navigate, selectGroup, selectSSHProfile]
  )
  const activeTab = useActiveTab()
  const paneRoot = usePaneRoot()
  const fullscreenPaneId = useFullscreenPaneId()
  const isAgentLauncherOpen = useWorkspaceStore((s) => s.agentLauncherPaneId !== null)
  const fullscreenPane = useMemo(() => {
    if (!fullscreenPaneId) return null
    const pane = findPaneById(paneRoot, fullscreenPaneId)
    return pane?.type === 'leaf' ? pane : null
  }, [fullscreenPaneId, paneRoot])
  const prevProjectIdRef = useRef<string>('')
  const watchedRootPathRef = useRef<string | null>(null)
  const projectSwitchRequestIdRef = useRef(0)
  const projectTerminalDismissedRef = useRef(false)
  const closingTerminalIdsRef = useRef<string[]>([])
  closingTerminalIdsRef.current = closingTerminalIds
  const ensureVisibleProjectTerminalRef = useRef<
    (projectId: string, options?: { explicit?: boolean }) => void
  >(() => {})
  const locationPathRef = useRef(location.pathname)
  locationPathRef.current = location.pathname

  // Ref for terminal close handler — used inside keydown effect to avoid
  // declaration-order dependency. The ref is updated each render.
  const handleCloseTerminalRef = useRef<((id: string, tabId: string) => void) | null>(null)

  /** Close the active tab (reused by keyboard shortcut and native menu event). */
  const closeActiveTab = useCallback(() => {
    if (!activeTab) return
    if (activeTab.type === 'editor') {
      const fileState = useEditorStore.getState().openFiles.get(activeTab.filePath)
      if (fileState?.isDirty) {
        setDirtyCloseFilePath(activeTab.filePath)
      } else {
        const didClose = useEditorStore.getState().closeFileIfIdle(activeTab.filePath)
        if (didClose) {
          useWorkspaceStore.getState().removeTab(activeTab.id)
        }
      }
    } else if (activeTab.type === 'git' || activeTab.type === 'git-history') {
      useWorkspaceStore.getState().removeTab(activeTab.id)
    } else if (activeTab.type === 'terminal') {
      handleCloseTerminalRef.current?.(activeTab.terminalId, activeTab.id)
    } else if (activeTab.type === 'browser') {
      useBrowserSessionStore.getState().removeTab(activeTab.browserTabId)
      useWorkspaceStore.getState().removeTab(activeTab.id)
    } else if (activeTab.type === 'agent-chat') {
      const acp = useAcpStore.getState()
      const conversationId =
        activeTab.conversationId ??
        (activeTab.sessionId ? acp.sessions[activeTab.sessionId]?.conversationId : undefined)
      if (conversationId) acp.closeChatView(conversationId)
      else if (activeTab.sessionId) useWorkspaceStore.getState().removeTab(activeTab.id)
    }
  }, [activeTab])

  // File watcher hook
  useFileWatcher()

  useEffect(() => {
    const persistBeforeUnload = () => {
      if (!activeProjectId) return
      persistState(editorPersistenceScopeKey)
      const terminalProjectIds =
        activeGroupProjects.length > 0
          ? activeGroupProjects.map((project) => project.id)
          : [activeProjectId]
      for (const projectId of terminalProjectIds) {
        void saveTerminalLayout(projectId).catch((error) => {
          console.warn('Failed to persist terminal layout before reload:', error)
        })
      }
      // R4: force-flush a non-debounced snapshot of every live ACP session's
      // cached payload on refresh unload so the durable copy is at worst one
      // turn behind (never truncated by a live-window trim). Best-effort: a
      // hard refresh may still abort the in-flight async drain (matching
      // `persistSession`'s never-throw contract) — log on failure, never
      // throw on unload.
      try {
        useAcpStore.getState().flushLiveSessionSaves()
      } catch (error) {
        console.warn('Failed to snapshot ACP sessions before reload:', error)
      }
      void flushSessionHistory().catch((error) => {
        console.warn('Failed to flush ACP history before reload:', error)
      })
    }

    window.addEventListener('beforeunload', persistBeforeUnload)
    window.addEventListener('pagehide', persistBeforeUnload)

    return () => {
      window.removeEventListener('beforeunload', persistBeforeUnload)
      window.removeEventListener('pagehide', persistBeforeUnload)
    }
  }, [activeGroupProjects, activeProjectId, editorPersistenceScopeKey])

  // Worktree shortcut handlers
  useWorktreeShortcuts()

  // Sync file explorer root path and register project root watcher when project changes
  useEffect(() => {
    if (activeGroupId) {
      const groupScopeKey = `group:${activeGroupId}:${activeGroupProjects
        .map((project) => `${project.id}:${project.path}`)
        .join('|')}`
      if (groupScopeKey === prevProjectIdRef.current) return
      if (watchedRootPathRef.current) {
        filesystemApi.unwatchDirectory(watchedRootPathRef.current)
        watchedRootPathRef.current = null
      }
      useFileExplorerStore.getState().setRoots(
        activeGroupProjects.map((project) => ({
          projectId: project.id,
          name: project.name,
          path: project.path ?? ''
        })),
        activeProject?.path
      )
      prevProjectIdRef.current = groupScopeKey
      return
    }

    const nextRootPathCandidate = activeProject?.path
    if (
      !activeProjectId ||
      typeof nextRootPathCandidate !== 'string' ||
      nextRootPathCandidate === ''
    ) {
      // Project removed or has no path — clear explorer root and unwatch
      useFileExplorerStore.getState().setRootPath('')
      if (watchedRootPathRef.current) {
        filesystemApi.unwatchDirectory(watchedRootPathRef.current)
        watchedRootPathRef.current = null
      }
      prevProjectIdRef.current = activeProjectId
      return
    }
    if (activeProjectId === prevProjectIdRef.current) {
      return
    }

    const nextRootPath = nextRootPathCandidate

    const switchRequestId = ++projectSwitchRequestIdRef.current
    const previousWatchedRoot = watchedRootPathRef.current

    let cancelled = false

    async function applyProjectSwitch(): Promise<void> {
      try {
        const watchResult = await filesystemApi.watchDirectory(nextRootPath)

        if (cancelled || switchRequestId !== projectSwitchRequestIdRef.current) {
          filesystemApi.unwatchDirectory(nextRootPath)
          return
        }

        if (!watchResult.success) {
          useFileExplorerStore.getState().setRootPath(nextRootPath)
          if (watchResult.code === 'WEB_UNSUPPORTED') {
            // Web client: directory watching is unavailable. Treat as a soft
            // no-op — the project switch still completes (file explorer
            // works, just no live change events) without surfacing a load
            // error to the user.
            if (previousWatchedRoot && previousWatchedRoot !== nextRootPath) {
              filesystemApi.unwatchDirectory(previousWatchedRoot)
            }
            watchedRootPathRef.current = nextRootPath
            prevProjectIdRef.current = activeProjectId
            return
          }
          useFileExplorerStore.getState().setRootLoadError({
            message: watchResult.error,
            code: watchResult.code
          })
          return
        }

        useFileExplorerStore.getState().setRootPath(nextRootPath)

        if (previousWatchedRoot && previousWatchedRoot !== nextRootPath) {
          filesystemApi.unwatchDirectory(previousWatchedRoot)
        }

        watchedRootPathRef.current = nextRootPath
        prevProjectIdRef.current = activeProjectId
      } catch (error) {
        if (cancelled || switchRequestId !== projectSwitchRequestIdRef.current) {
          return
        }

        const message =
          error instanceof Error
            ? error.message
            : runtimeT(
                'projects',
                'filesystemErrors.watchProjectDirectory',
                'Failed to watch project directory'
              )
        useFileExplorerStore.getState().setRootPath(nextRootPath)
        useFileExplorerStore.getState().setRootLoadError({
          message,
          code: 'WATCH_FAILED'
        })
      }
    }

    void applyProjectSwitch()

    return () => {
      cancelled = true
    }
  }, [activeGroupId, activeGroupProjects, activeProject?.path, activeProjectId])

  // The file tree follows the open Conversation's workspace directory while in
  // the Conversation area, and the active project elsewhere. Project switches
  // keep their dedicated effect above; this one only owns scope transitions.
  useEffect(() => {
    const inConversationScope = location.pathname.startsWith('/c/')
    if (!inConversationScope && activeGroupId) {
      useFileExplorerStore.getState().setRoots(
        activeGroupProjects.map((project) => ({
          projectId: project.id,
          name: project.name,
          path: project.path ?? ''
        })),
        activeProject?.path
      )
      watchedRootPathRef.current = null
      return
    }
    const desiredRoot = inConversationScope
      ? (activeConversation?.workspaceCwd ?? '')
      : (activeProject?.path ?? '')
    if (!desiredRoot || desiredRoot === watchedRootPathRef.current) return
    const previousWatchedRoot = watchedRootPathRef.current
    let cancelled = false
    void filesystemApi.watchDirectory(desiredRoot).then((watchResult) => {
      if (cancelled) return
      if (!watchResult.success && watchResult.code !== 'WEB_UNSUPPORTED') {
        useFileExplorerStore.getState().setRootLoadError({
          message: watchResult.error ?? 'Failed to watch directory',
          code: watchResult.code ?? 'WATCH_FAILED'
        })
        return
      }
      useFileExplorerStore.getState().setRootPath(desiredRoot)
      if (previousWatchedRoot && previousWatchedRoot !== desiredRoot) {
        filesystemApi.unwatchDirectory(previousWatchedRoot)
      }
      watchedRootPathRef.current = desiredRoot
    })
    return () => {
      cancelled = true
    }
  }, [
    location.pathname,
    activeConversation?.workspaceCwd,
    activeGroupId,
    activeGroupProjects,
    activeProject?.path
  ])

  // Editor state persistence
  useEditorPersistence(
    isConversationAreaPath(location.pathname) || activeConversationId
      ? ''
      : editorPersistenceScopeKey,
    activeGroupId
      ? {
          projectIds: activeGroupProjects.map((project) => project.id),
          rootPaths: activeGroupProjects.flatMap((project) => (project.path ? [project.path] : [])),
          manifestProjectId: null,
          notificationProjectId: activeProjectId
        }
      : {
          projectIds: activeProjectId ? [activeProjectId] : [],
          rootPaths: activeProject?.path ? [activeProject.path] : [],
          manifestProjectId: activeProjectId || null,
          notificationProjectId: activeProjectId
        }
  )

  // SessionWorkspace is keyed only by canonical ConversationId. The legacy
  // project manifest remains a read-only migration input and receives no live writes.
  useSessionWorkspaceSync(activeConversationId)

  useEffect(() => {
    return () => {
      if (watchedRootPathRef.current) {
        filesystemApi.unwatchDirectory(watchedRootPathRef.current)
      }
    }
  }, [])

  // Ensure tabs exist for currently visible project terminals.
  // Project workspace loading/removal is owned by persistence + restore flows.
  // Debounce: rapid terminal store mutations (addTerminal → setTerminalPtyId →
  // addTabToPane) settle before syncTerminalTabs runs, preventing the MOUNT/UNMOUNT
  // cascade where intermediate states look like "orphaned" tabs.
  const ensureCallCountRef = useRef(0)
  const lastEnsuredTerminalIdsRef = useRef<string[]>([])
  const lastEnsuredProjectIdRef = useRef<string>('')
  const syncDebounceTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    const terminalIds = terminals.filter(isOpenTerminalView).map((terminal) => terminal.id)

    // Clear any pending debounce — only the latest mutation triggers a sync.
    if (syncDebounceTimerRef.current) {
      clearTimeout(syncDebounceTimerRef.current)
      syncDebounceTimerRef.current = null
    }

    // If we switched projects, we should wait for the persistence layer (useEditorPersistence)
    // to finish its job of replacing the entire workspace tree.
    // Forcing a sync on the WRONG tree (the old project's tree) causes "leaking" tabs.
    if (activeProjectId !== lastEnsuredProjectIdRef.current) {
      lastEnsuredTerminalIdsRef.current = terminalIds
      lastEnsuredProjectIdRef.current = activeProjectId
      // We skip the sync here because useEditorPersistence will handle the initial layout.
      return
    }

    const prevIds = lastEnsuredTerminalIdsRef.current
    if (terminalIds.length === prevIds.length && terminalIds.every((id, i) => id === prevIds[i])) {
      return
    }

    // Debounce: wait for store mutations to settle before syncing tabs.
    // This prevents the cascade where syncTerminalTabs runs between addTerminal
    // and addTabToPane, sees a tab as orphaned, removes it, triggering
    // ConnectedTerminal unmount and a restore re-trigger.
    syncDebounceTimerRef.current = setTimeout(() => {
      lastEnsuredTerminalIdsRef.current = terminalIds
      const ensureId = `ensure-${ensureCallCountRef.current++}-${Date.now().toString().slice(-6)}`

      console.log(`[WorkspaceLayout] syncTerminalTabs CALL [${ensureId}]`, {
        projectId: activeProjectId,
        terminalCount: terminalIds.length,
        terminalIds,
        prevCount: prevIds.length,
        callCount: ensureCallCountRef.current
      })

      const workspaceStore = useWorkspaceStore.getState()
      workspaceStore.syncTerminalTabs(terminalIds)
    }, 100)

    return () => {
      if (syncDebounceTimerRef.current) {
        clearTimeout(syncDebounceTimerRef.current)
        syncDebounceTimerRef.current = null
      }
    }
  }, [terminals, activeProjectId])

  // Sync legacy stores (activeTerminalId, activeFilePath) from workspace pane tree
  useEffect(() => {
    return useWorkspaceStore.subscribe((state, prevState) => {
      if (state.root === prevState.root && state.activePaneId === prevState.activePaneId) return

      const terminalId = getActiveTerminalIdFromTree(state)
      if (terminalId !== null) {
        const termStore = useTerminalStore.getState()
        if (termStore.activeTerminalId !== terminalId) {
          termStore.selectTerminal(terminalId)
        }
      }

      const filePath = getActiveFilePathFromTree(state)
      const editorStore = useEditorStore.getState()
      if (editorStore.activeFilePath !== filePath) {
        editorStore.setActiveFilePath(filePath)
      }
    })
  }, [])

  const closeAppWithPersistenceFlush = useCallback(async () => {
    try {
      const [
        pendingAppSettingsResult,
        pendingPersistenceResult,
        pendingSessionIndexResult,
        historyFlushResult
      ] = await Promise.allSettled([
        waitForPendingAppSettingsPersistence(),
        persistenceApi.flushPendingWrites(),
        waitForPendingSessionIndexWrite(),
        flushSessionHistory()
      ])

      if (pendingAppSettingsResult.status === 'rejected') {
        console.error(
          'Failed to wait for app settings persistence before close:',
          pendingAppSettingsResult.reason
        )
      }

      // Note: waitForPendingSessionIndexWrite swallows rejections internally
      // (trackPendingIndexWrite catches and logs them), so this branch is
      // effectively dead code — kept as a defensive guard in case the
      // swallowing behavior changes.
      if (pendingSessionIndexResult.status === 'rejected') {
        console.error(
          'Failed to wait for session index persistence before close:',
          pendingSessionIndexResult.reason
        )
      }

      if (historyFlushResult.status === 'rejected') {
        console.error('Failed to flush ACP history before close:', historyFlushResult.reason)
      }

      if (pendingPersistenceResult.status === 'fulfilled') {
        if (!pendingPersistenceResult.value.success) {
          console.error(
            'Failed to flush pending persistence writes before close:',
            pendingPersistenceResult.value.error
          )
        }
      } else {
        console.error(
          'Failed to flush pending persistence writes before close:',
          pendingPersistenceResult.reason
        )
      }
    } finally {
      windowApi.respondToClose('close')
      setIsAppCloseDialogOpen(false)
    }
  }, [])

  // Intercept app close to check for unsaved files
  useEffect(() => {
    return windowApi.onCloseRequested(() => {
      const dirtyCount = useEditorStore.getState().getDirtyFileCount()
      if (dirtyCount > 0) {
        setAppCloseDirtyCount(dirtyCount)
        setIsAppCloseDialogOpen(true)
      } else {
        void closeAppWithPersistenceFlush()
      }

      return Promise.resolve(false)
    })
  }, [closeAppWithPersistenceFlush])

  // Tray Quit is an explicit app-quit request. It reuses the renderer's
  // existing dirty-file prompt and persistence flush instead of bypassing it
  // with a native app.exit(0).
  useEffect(() => {
    let unlisten: UnlistenFn | undefined
    let disposed = false
    listen<void>('tray:quit-requested', () => {
      const dirtyCount = useEditorStore.getState().getDirtyFileCount()
      if (dirtyCount > 0) {
        setAppCloseDirtyCount(dirtyCount)
        setIsAppCloseDialogOpen(true)
      } else {
        void closeAppWithPersistenceFlush()
      }
    })
      .then((fn) => {
        if (disposed) {
          fn()
        } else {
          unlisten = fn
        }
      })
      .catch((error) => {
        console.error('Failed to register tray quit listener:', error)
      })
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [closeAppWithPersistenceFlush])

  // Listen for native menu "Close Tab" event (macOS Cmd+W intercepted by menu bar)
  useEffect(() => {
    let unlisten: UnlistenFn | undefined
    listen<void>('menu:close-tab', () => {
      // Skip when Agent Launcher is open to avoid accidental tab closure
      if (!isAgentLauncherOpen) {
        closeActiveTab()
      }
    })
      .then((fn) => {
        unlisten = fn
      })
      .catch(() => {
        // Not in Tauri context — ignore
      })
    return () => {
      unlisten?.()
    }
  }, [closeActiveTab, isAgentLauncherOpen])

  // Load snapshots when project changes
  useSnapshotLoader()
  // Load recent commands for command palette
  useRecentCommandsLoader()
  // Load pinned commands for command palette
  usePinnedCommandsLoader()
  // Load command history for current project
  useCommandHistoryLoader(activeProjectId)
  const commandHistory = useCommandHistory(activeProjectId)
  const allCommandHistory = useAllCommandHistory()
  const createSnapshot = useCreateSnapshot()

  const handleCreateSnapshot = useCallback(
    async (name: string, description?: string) => {
      await createSnapshot(name, description)
    },
    [createSnapshot]
  )

  const handleOpenSnapshotModal = useCallback(() => {
    setIsCommandPaletteOpen(false)
    setIsCreateSnapshotModalOpen(true)
  }, [])

  // Keyboard shortcuts
  const shortcuts = useKeyboardShortcutsStore((state) => state.shortcuts)
  const handleOpenProjectSettings = useCallback(() => {
    setIsCommandPaletteOpen(false)
    navigate('/settings')
  }, [navigate])

  const handleOpenAppPreferences = useCallback(() => {
    setIsCommandPaletteOpen(false)
    if (useThemePickerStore.getState().isOpen) {
      useThemePickerStore.getState().cancel()
    }
    navigate('/preferences')
  }, [navigate])

  const handleOpenCommandHistory = useCallback(() => {
    setIsCommandPaletteOpen(false)
    setIsCommandHistoryOpen(true)
  }, [])

  const handleOpenShortcutMenu = useCallback(() => {
    setIsCommandPaletteOpen(false)
    setIsShortcutMenuOpen(true)
  }, [])

  // SSH - just select profile (SSH workspace handles its own connect/terminal)
  const handleSSHConnect = useCallback(
    (profileId: string) => {
      const profile = sshProfiles.find((p) => p.id === profileId)
      if (!profile) return

      if (profile.authMethod === 'password' && !profile.hasStoredPassword) {
        // No password in OS keychain — show password prompt
        setSSHPasswordPrompt({ profileId, profileName: profile.name })
        setSSHPasswordInput('')
      } else {
        // Select profile → SSH workspace handles connect
        selectSSHProfile(profileId)
      }
    },
    [sshProfiles, selectSSHProfile]
  )

  const handleSSHPasswordSubmit = useCallback(() => {
    if (!sshPasswordPrompt) return
    const password = sshPasswordInput
    setSSHPromptPasswords((prev) => ({
      ...prev,
      [sshPasswordPrompt.profileId]: password
    }))
    setSSHPasswordPrompt(null)
    setSSHPasswordInput('')
    selectSSHProfile(sshPasswordPrompt.profileId)
  }, [sshPasswordPrompt, sshPasswordInput, selectSSHProfile])

  const getShortcutLabel = useCallback(
    (id: string): string | undefined => {
      const shortcut = shortcuts[id]
      return shortcut ? (shortcut.customKey ?? shortcut.defaultKey) : undefined
    },
    [shortcuts]
  )

  const getProjectShortcutLabel = useCallback(
    (index: number): string | undefined => {
      const shortcut = shortcuts[`project-${index + 1}`]
      return shortcut ? (shortcut.customKey ?? shortcut.defaultKey) : undefined
    },
    [shortcuts]
  )

  const uiZoomLevel = useUiZoomLevel()
  const colorTheme = useColorTheme()
  const appearanceMode = useAppearanceMode()

  const isThemePickerOpen = useThemePickerOpen()

  const closeThemePickerPeerOverlays = useCallback(() => {
    setIsCommandPaletteOpen(false)
    setIsShortcutMenuOpen(false)
    setIsCommandHistoryOpen(false)
  }, [])

  const handleToggleThemePicker = useCallback(() => {
    if (document.activeElement instanceof HTMLElement) {
      document.activeElement.blur()
    }
    if (location.pathname === '/preferences') {
      navigate('/')
    }
    closeThemePickerPeerOverlays()
    useThemePickerStore.getState().toggle(getEffectiveThemeId(colorTheme, appearanceMode))
  }, [appearanceMode, closeThemePickerPeerOverlays, colorTheme, location.pathname, navigate])

  const handleOpenThemePicker = useCallback(() => {
    if (document.activeElement instanceof HTMLElement) {
      document.activeElement.blur()
    }
    if (location.pathname === '/preferences') {
      navigate('/')
    }
    closeThemePickerPeerOverlays()
    const store = useThemePickerStore.getState()
    if (!store.isOpen) {
      store.open(getEffectiveThemeId(colorTheme, appearanceMode))
    }
  }, [appearanceMode, closeThemePickerPeerOverlays, colorTheme, location.pathname, navigate])

  const appDefaultShell = useDefaultShell()
  const maxTerminals = useMaxTerminalsPerProject()
  const updateAppSetting = useUpdateAppSetting()
  const updatePanelVisibility = useUpdatePanelVisibility()

  const handleToggleTerminalList = useCallback((): void => {
    void updatePanelVisibility('terminalListPanelVisible', !isTerminalListVisible).catch(
      (error) => {
        toast.error(
          error instanceof Error
            ? error.message
            : runtimeT(
                'workspace',
                'errors.updateTerminalListVisibility',
                'Failed to update terminal list visibility'
              )
        )
      }
    )
  }, [updatePanelVisibility, isTerminalListVisible])

  // Helper to get active key for a shortcut
  const getActiveKey = useCallback(
    (id: string): string => {
      const shortcut = shortcuts[id]
      return shortcut?.customKey ?? shortcut?.defaultKey ?? ''
    },
    [shortcuts]
  )

  // Shared whole-UI zoom action used by both the DOM keydown path and the
  // keyboardApi.onShortcut callback so behavior stays identical.
  const applyZoomAction = useCallback(
    (action: 'zoomIn' | 'zoomOut' | 'zoomReset'): void => {
      const next =
        action === 'zoomIn'
          ? Math.min(uiZoomLevel + UI_ZOOM_STEP, UI_ZOOM_MAX)
          : action === 'zoomOut'
            ? Math.max(uiZoomLevel - UI_ZOOM_STEP, UI_ZOOM_MIN)
            : UI_ZOOM_DEFAULT
      if (next !== uiZoomLevel) updateAppSetting('uiZoomLevel', next)
    },
    [uiZoomLevel, updateAppSetting]
  )

  // Desktop: the independent Conversation area (list dashboard + open conversations) is
  // entered from the Activity Rail chat toggle; the portable index route owns the regular
  // project workspace with terminal panes, so the two areas never render side by side.
  // The phone shell keeps its own navigation and still owns the dashboard at its root.
  const isConversationListRoute = location.pathname === '/conversations'
  const isOpenConversationRoute = location.pathname.startsWith('/c/')
  // File explorer visibility follows the Conversation workspace in the
  // Conversation area and the active project elsewhere.
  const explorerRootVisible = location.pathname.startsWith('/c/')
    ? Boolean(activeConversation?.workspaceCwd)
    : Boolean(activeProject?.path)
  const isConversationRoute =
    isConversationListRoute ||
    location.pathname.startsWith('/c/') ||
    (isMobileWebShell && location.pathname === '/')
  const isWorkspaceRoute =
    location.pathname.startsWith('/c/') ||
    (!isMobileWebShell && (location.pathname === '/' || location.pathname === '/terminal'))

  useEffect(() => {
    if (isOpenConversationRoute) {
      useWorkspaceStore.getState().hideAgentLauncher()
    }
  }, [isOpenConversationRoute])

  // Unified tab cycling - cycles through ALL workspace tabs in active pane
  const cycleTab = useCallback(
    (direction: 'next' | 'prev') => {
      if (!isWorkspaceRoute) return
      const store = useWorkspaceStore.getState()
      const nextTabId = store.getNextTabId(direction === 'next' ? 1 : -1)
      if (nextTabId) {
        store.setActiveTab(store.activePaneId, nextTabId)
      }
    },
    [isWorkspaceRoute]
  )

  // Terminal creation callbacks - defined before keyboard shortcut useEffect
  const handleCreateTerminalInPane = useCallback(
    async (paneId: string, shellName?: string, targetProjectId?: string) => {
      // Terminals are Conversation-scoped only inside an open Conversation;
      // the regular project workspace keeps scope-less project terminals.
      const inConversationScope =
        location.pathname.startsWith('/c/') && Boolean(activeConversationId)
      if (!inConversationScope && activeGroupProjects.length > 1 && !targetProjectId) {
        setPendingTerminalRoot({ paneId, shellName })
        return
      }
      const terminalProjectId = targetProjectId ?? activeProjectId
      const terminalProject = projects.find((project) => project.id === terminalProjectId)
      const cwd = inConversationScope
        ? (activeConversation?.workspaceCwd ?? getDefaultCwdForProject(activeProjectId))
        : getDefaultCwdForProject(terminalProjectId)

      const result = await spawnTerminalInPane(paneId, terminalProjectId, cwd, {
        shell: shellName || terminalProject?.defaultShell || appDefaultShell || undefined,
        envVars: terminalProject?.envVars,
        maxTerminalsPerProject: maxTerminals,
        conversationId: inConversationScope ? (activeConversationId ?? undefined) : undefined
      })
      if (!result.success) {
        toast.error(
          result.error ||
            runtimeT('workspace', 'errors.createTerminal', 'Failed to create terminal')
        )
      }
    },
    [
      activeConversation?.workspaceCwd,
      activeConversationId,
      activeGroupProjects,
      activeProjectId,
      appDefaultShell,
      location.pathname,
      maxTerminals,
      projects
    ]
  )

  // ADR-004.5: command-bar "Launch Agent" entry. Launches the default agent's
  // TUI in the active pane with no seed prompt so the user composes inside the
  // agent UI; the empty-pane launcher offers the full prompt+picker flow.
  const handleLaunchAgent = useCallback(async () => {
    const paneId = useWorkspaceStore.getState().activePaneId
    if (!paneId || !activeProjectId) return
    const cwd = getDefaultCwdForProject(activeProjectId)
    const result = await launchAgentInPane(
      paneId,
      activeProjectId,
      cwd,
      BUILT_IN_AGENTS[0],
      undefined,
      {
        envVars: activeProject?.envVars,
        maxTerminalsPerProject: maxTerminals
      }
    )
    if (!result.success) {
      toast.error(
        result.error || runtimeT('workspace', 'errors.launchAgent', 'Failed to launch agent')
      )
    }
  }, [activeProjectId, activeProject?.envVars, maxTerminals])

  const handleAddTerminal = useCallback(
    (paneId: string | undefined, shell?: ShellInfo) => {
      const targetPaneId = paneId ?? useWorkspaceStore.getState().activePaneId
      if (!targetPaneId) return
      if (shell) {
        handleCreateTerminalInPane(targetPaneId, shell.path)
      } else {
        handleCreateTerminalInPane(targetPaneId)
      }
    },
    [handleCreateTerminalInPane]
  )

  useEffect(() => {
    if (location.pathname !== '/terminal') return
    const workspace = useWorkspaceStore.getState()
    const existing = getAllLeafPanes(workspace.root).flatMap((leaf) =>
      (leaf.tabs ?? [])
        .filter((tab) => tab.type === 'terminal')
        .map((tab) => ({ paneId: leaf.id, tabId: tab.id }))
    )
    const first = existing[0]
    if (first) {
      if (workspace.activePaneId !== first.paneId) {
        workspace.setActivePane(first.paneId)
      }
      workspace.setActiveTab(first.paneId, first.tabId)
      return
    }
    handleAddTerminal(undefined)
  }, [handleAddTerminal, location.pathname])

  const ensureVisibleProjectTerminal = useCallback(
    (projectId: string, options?: { explicit?: boolean }) => {
      if (locationPathRef.current !== '/') return
      if (applyPendingTerminalFocus(projectId)) return
      // The user just hid the last project terminal. Do not undo that close
      // by spawning or reopening from the restore path. An explicit click on
      // the row's terminal button is the user asking again, so it overrides.
      if (projectTerminalDismissedRef.current && !options?.explicit) {
        void logFrontendError({
          level: 'warn',
          source: 'workspace-layout.project-terminal',
          message: `skip auto-open after user close projectId=${projectId}`
        })
        return
      }
      const workspace = useWorkspaceStore.getState()
      const terminals = useTerminalStore
        .getState()
        .terminals.filter((terminal) => terminal.projectId === projectId)
      const hasVisibleProjectTerminal = getAllLeafPanes(workspace.root).some((leaf) =>
        leaf.tabs.some((tab) => {
          if (tab.type !== 'terminal') return false
          return terminals.some((terminal) => terminal.id === tab.terminalId)
        })
      )
      if (hasVisibleProjectTerminal) return
      const hiddenLive = terminals.find(
        (terminal) => terminal.ptyId && !isOpenTerminalView(terminal)
      )
      if (hiddenLive) {
        useWorkspaceStore.getState().reopenTerminalView(hiddenLive.id)
        return
      }
      const liveVisible = terminals.find(
        (terminal) => terminal.ptyId && isOpenTerminalView(terminal)
      )
      if (liveVisible) {
        useWorkspaceStore.getState().ensureTerminalTab(liveVisible.id, undefined, true)
        return
      }
      const hasOtherProjectContent = getAllLeafPanes(workspace.root).some((leaf) =>
        leaf.tabs.some(
          (tab) =>
            tab.type === 'editor' ||
            tab.type === 'browser' ||
            tab.type === 'git' ||
            tab.type === 'git-history'
        )
      )
      if (!hasOtherProjectContent) {
        workspace.resetLayout()
      }
      useWorkspaceStore.getState().hideAgentLauncher()
      handleAddTerminal(undefined)
    },
    [handleAddTerminal]
  )
  ensureVisibleProjectTerminalRef.current = ensureVisibleProjectTerminal

  // Entering a project (sidebar click or startup with a restored active project)
  // always lands in the regular project workspace: leave the independent
  // Conversation area and open one terminal when the project has none yet, so
  // the project section is entered instead of the chat launcher empty state.
  const conversationWorkspaceActiveRef = useRef(isConversationAreaPath(location.pathname))

  // Opening a conversation replaces the pane tree. Persist the project layout
  // when entering the conversation area (list or an open chat), and restore it
  // only when leaving that area for the project workspace — not when closing
  // or deleting a chat back to /conversations.
  useEffect(() => {
    const inConversationWorkspace = isConversationAreaPath(location.pathname)
    const wasInConversationWorkspace = conversationWorkspaceActiveRef.current
    conversationWorkspaceActiveRef.current = inConversationWorkspace
    if (!wasInConversationWorkspace && inConversationWorkspace && activeProjectId) {
      persistState(activeProjectId)
      return
    }
    if (!wasInConversationWorkspace || inConversationWorkspace || isMobileWebShell) return
    const projectId = activeProjectId
    let cancelled = false
    void (async () => {
      const restored = projectId ? await restoreProjectWorkspace(projectId) : false
      if (cancelled) return
      useConversationStore.getState().setActiveConversationId(null)
      if (!restored) useWorkspaceStore.getState().resetLayout()
      if (projectId) {
        // Layout only. Launching a terminal is an explicit act now — the row's
        // terminal button — so returning from the conversation area restores
        // what was open and adds nothing the user did not ask for.
        projectTerminalDismissedRef.current = false
      }
    })()
    return () => {
      cancelled = true
    }
  }, [activeProjectId, isMobileWebShell, location.pathname])
  useEffect(() => {
    if (location.pathname !== '/') return
    const pending = peekPendingTerminalFocus()
    if (!pending || pending.projectId !== activeProjectId) return
    const timer = setTimeout(() => {
      ensureVisibleProjectTerminalRef.current(pending.projectId)
    }, 400)
    return () => clearTimeout(timer)
  }, [activeProjectId, location.pathname])

  // Entering a project used to spawn a terminal here after 400ms, which made
  // every project click a terminal launch. Selecting a project now means "show
  // me this folder" — the file tree follows the selection — and a terminal is
  // launched from the row's terminal button. Nothing is left for this effect to
  // do: `handleSelectProject` and `handleSelectGroup` already clear the
  // dismissed flag on the paths that used to rely on it.

  useEffect(() => {
    return subscribeProjectWorkspaceRestored((projectId) => {
      if (locationPathRef.current !== '/') return
      if (useProjectStore.getState().activeProjectId !== projectId) return
      ensureVisibleProjectTerminal(projectId)
    })
  }, [ensureVisibleProjectTerminal])

  const handleNewBrowserTab = useCallback((paneId?: string) => {
    const resolvedPaneId = paneId ?? useWorkspaceStore.getState().activePaneId
    if (resolvedPaneId) {
      const browserTabId = randomUUID()
      useBrowserSessionStore.getState().createTab(browserTabId)
      useWorkspaceStore.getState().addBrowserTab(browserTabId, resolvedPaneId)
    }
  }, [])

  const handleOpenAgentChat = useCallback(() => {
    const open = (): void => {
      const paneId = useWorkspaceStore.getState().activePaneId
      if (paneId) useWorkspaceStore.getState().showAgentLauncher(paneId)
    }
    if (!isMobileWebShell && isConversationRoute) {
      if (location.pathname !== '/conversations') navigate('/conversations')
      requestAnimationFrame(open)
      return
    }
    // The launcher overlay only renders on the workspace route; navigate there
    // first when invoked from a child route (e.g. preferences/settings).
    if (location.pathname !== '/') {
      navigate('/')
      requestAnimationFrame(open)
    } else {
      open()
    }
  }, [isConversationRoute, isMobileWebShell, location.pathname, navigate])

  const handleAddGitTab = useCallback(
    (paneId?: string) => {
      const resolvedPaneId = paneId ?? useWorkspaceStore.getState().activePaneId
      if (resolvedPaneId && activeProject?.path) {
        useWorkspaceStore.getState().addTabToPane(resolvedPaneId, {
          type: 'git',
          id: `git-${randomUUID()}`,
          cwd: activeProject.path
        })
      }
    },
    [activeProject?.path]
  )

  // If the active project loses its path (switched/deleted) while the mobile
  // Git Changes sheet is open, close it so it never lingers empty.
  useEffect(() => {
    if (gitSheetOpen && !activeProject?.path) {
      setGitSheetOpen(false)
    }
  }, [gitSheetOpen, activeProject?.path])

  const handleAddGitHistoryTab = useCallback(
    (paneId?: string) => {
      const resolvedPaneId = paneId ?? useWorkspaceStore.getState().activePaneId
      if (!resolvedPaneId) return
      // Resolve the worktree-aware cwd (same helper terminal creation uses) so
      // the history reflects the active worktree, not just the project root.
      const resolvedCwd = getDefaultCwdForProject(activeProjectId)
      if (!resolvedCwd) return
      useWorkspaceStore.getState().addTabToPane(resolvedPaneId, {
        type: 'git-history',
        id: `git-history-${randomUUID()}`,
        cwd: resolvedCwd
      })
    },
    [activeProjectId]
  )

  // Capture-phase save so WebView/editors cannot block Ctrl+S before it reaches us.
  useEffect(() => {
    const handleSaveShortcut = (e: KeyboardEvent): void => {
      if (!isSaveFileShortcut(e)) return
      e.preventDefault()
      e.stopPropagation()
      const path =
        activeTab?.type === 'editor' ? activeTab.filePath : useEditorStore.getState().activeFilePath
      if (path) {
        void requestSaveEditorFile(path)
      }
    }

    window.addEventListener('keydown', handleSaveShortcut, { capture: true })
    return () => window.removeEventListener('keydown', handleSaveShortcut, { capture: true })
  }, [activeTab])

  // biome-ignore lint/correctness/useExhaustiveDependencies: handler reads latest values via closure; deps intentionally narrow
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (isSaveFileShortcut(e)) return

      // Safety net: skip workspace handling when an earlier handler has already
      // processed this event by calling preventDefault() — e.g. xterm clipboard
      // ops or ConnectedTerminal's customKeyEventHandler for terminal-owned keys.
      if (e.defaultPrevented) return

      const { isInEditor, isInTerminal, isInInput } = getShortcutTargetContext(e.target)

      // Close Tab (Ctrl+W / ⌘+W)
      // On macOS: ⌘+W closes tab, Ctrl+W is forwarded to shell (backward-kill-word)
      // On Windows/Linux: Ctrl+W closes tab
      // Always preventDefault to suppress OS/webview close behavior; only
      // close the tab when the Agent Launcher is not open.
      if (matchesShortcut(e, getActiveKey('closeTab'))) {
        e.preventDefault()
        if (!isAgentLauncherOpen) {
          closeActiveTab()
        }
        return
      }

      // Toggle File Explorer (Ctrl+B / ⌘+B) — skip when in editor/input/terminal
      if (matchesShortcut(e, getActiveKey('toggleFileExplorer'))) {
        if (!isInEditor && !isInInput && !isInTerminal) {
          e.preventDefault()
          void updatePanelVisibility('fileExplorerVisible', !isExplorerVisible).catch((error) => {
            toast.error(
              error instanceof Error
                ? error.message
                : runtimeT(
                    'workspace',
                    'errors.updateFileExplorerVisibility',
                    'Failed to update file explorer visibility'
                  )
            )
          })
        }
        return
      }

      if (matchesShortcut(e, getActiveKey('toggleCliSessionPanel'))) {
        if (!isInEditor && !isInInput && !isInTerminal) {
          e.preventDefault()
          void updatePanelVisibility('cliSessionPanelVisible', !isCliSessionPanelVisible).catch(
            (error) => {
              toast.error(
                error instanceof Error
                  ? error.message
                  : runtimeT(
                      'shell',
                      'titleBar.failedCliSessions',
                      'Failed to update CLI sessions visibility'
                    )
              )
            }
          )
        }
        return
      }

      if (matchesShortcut(e, getActiveKey('sidebarToggle'))) {
        if (!isInEditor && !isInInput) {
          e.preventDefault()
          e.stopPropagation()
          void updatePanelVisibility('sidebarVisible', !isSidebarVisible).catch((error) => {
            toast.error(
              error instanceof Error
                ? error.message
                : runtimeT(
                    'workspace',
                    'errors.updateSidebarVisibility',
                    'Failed to update sidebar visibility'
                  )
            )
          })
        }
        return
      }

      // ── Global shortcuts — work from any focus context ────────────────
      // These must be checked before the isInInput/isInEditor guard.
      // They open overlays or perform workspace actions that should be
      // reachable while typing in the editor, browser, or terminal.

      // Command palette (Ctrl+K / Ctrl+Shift+P)
      if (
        matchesShortcut(e, getActiveKey('commandPalette')) ||
        matchesShortcut(e, getActiveKey('commandPaletteAlt'))
      ) {
        e.preventDefault()
        e.stopPropagation()
        if (document.activeElement instanceof HTMLElement) {
          document.activeElement.blur()
        }
        setIsCommandPaletteOpen(true)
        return
      }

      // Command history (Ctrl+R)
      if (matchesShortcut(e, getActiveKey('commandHistory'))) {
        e.preventDefault()
        e.stopPropagation()
        if (activeProjectId) {
          if (document.activeElement instanceof HTMLElement) {
            document.activeElement.blur()
          }
          setIsCommandHistoryOpen(true)
        }
        return
      }

      // Color theme picker (Ctrl+Alt+T)
      if (matchesShortcut(e, getActiveKey('colorThemePicker'))) {
        e.preventDefault()
        e.stopPropagation()
        handleOpenThemePicker()
        return
      }

      // New project (Ctrl+N)
      if (matchesShortcut(e, getActiveKey('newProject'))) {
        e.preventDefault()
        e.stopPropagation()
        if (document.activeElement instanceof HTMLElement) {
          document.activeElement.blur()
        }
        setIsNewProjectModalOpen(true)
        return
      }

      // Ctrl+T: show the agent launcher prompt overlay in the active pane.
      // The launcher is an overlay — existing tabs are preserved underneath.
      // When the agent is launched, a new tab is added to the same pane.
      if (matchesShortcut(e, getActiveKey('newTerminal'))) {
        if (!isWorkspaceRoute) return
        e.preventDefault()
        e.stopPropagation()
        const paneId = useWorkspaceStore.getState().activePaneId
        if (paneId) {
          const current = useWorkspaceStore.getState().agentLauncherPaneId
          // Toggle: if already showing on this pane, hide it; otherwise show it.
          if (current === paneId) {
            useWorkspaceStore.getState().hideAgentLauncher()
          } else {
            useWorkspaceStore.getState().showAgentLauncher(paneId)
          }
        }
        return
      }

      // New browser tab (Ctrl+Shift+N) - workspace only
      if (matchesShortcut(e, getActiveKey('newBrowserTab'))) {
        if (!isWorkspaceRoute) return
        e.preventDefault()
        e.stopPropagation()
        handleNewBrowserTab()
        return
      }

      // Tab cycling (Ctrl+PageDown / Ctrl+PageUp)
      if (matchesShortcut(e, getActiveKey('nextTerminal'))) {
        e.preventDefault()
        e.stopPropagation()
        cycleTab('next')
        return
      }
      if (matchesShortcut(e, getActiveKey('prevTerminal'))) {
        e.preventDefault()
        e.stopPropagation()
        cycleTab('prev')
        return
      }

      // Unbounded counterpart to the bounded switcher row above the panes.
      if (matchesShortcut(e, getActiveKey('terminalSwitcher'))) {
        e.preventDefault()
        e.stopPropagation()
        setIsTerminalSwitcherOpen((open) => !open)
        return
      }

      // Jump straight back to the terminal you came from. Unlike the tab
      // cycling above this is not positional — it follows the MRU stack, so it
      // reaches a terminal in another pane or another project too.
      if (matchesShortcut(e, getActiveKey('lastTerminal'))) {
        e.preventDefault()
        e.stopPropagation()
        const store = useTerminalStore.getState()
        const previousId = store.getPreviousTerminalId()
        if (previousId) {
          const previous = store.terminals.find((terminal) => terminal.id === previousId)
          openBoardTerminal({
            projectId: previous?.projectId ?? null,
            terminalId: previousId,
            navigate
          })
          store.selectTerminal(previousId)
        }
        return
      }

      // Zoom in/out/reset — whole-UI zoom (VS Code style)
      if (matchesShortcut(e, getActiveKey('zoomIn'))) {
        e.preventDefault()
        e.stopPropagation()
        applyZoomAction('zoomIn')
        return
      }
      if (matchesShortcut(e, getActiveKey('zoomOut'))) {
        e.preventDefault()
        e.stopPropagation()
        applyZoomAction('zoomOut')
        return
      }
      if (matchesShortcut(e, getActiveKey('zoomReset'))) {
        e.preventDefault()
        e.stopPropagation()
        applyZoomAction('zoomReset')
        return
      }

      // Cmd/Ctrl + 1-9 for project switching
      if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && /^[1-9]$/.test(e.key)) {
        e.preventDefault()
        const index = parseInt(e.key, 10) - 1
        if (projects[index]) selectProject(projects[index].id)
        return
      }

      // ── Below this: only runs when NOT in input/editor ────────────────
      // Terminal search (Ctrl+F) - handled at pane level
      if (matchesShortcut(e, getActiveKey('terminalSearch'))) {
        if (isWorkspaceRoute) {
          e.preventDefault()
          e.stopPropagation()
        }
        return
      }
    }

    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [
    projects,
    selectProject,
    addTerminal,
    terminals,
    activeProjectId,
    activeProject,
    activeTerminalId,
    activeTerminal,
    getActiveKey,
    applyZoomAction,
    appDefaultShell,
    maxTerminals,
    isWorkspaceRoute,
    cycleTab,
    activeTab,
    handleCreateTerminalInPane,
    handleNewBrowserTab,
    updatePanelVisibility,
    isExplorerVisible,
    isCliSessionPanelVisible,
    isSidebarVisible,
    handleOpenThemePicker,
    closeActiveTab,
    isAgentLauncherOpen,
    navigate
  ])

  useEffect(() => {
    // Hide the active browser webview while a modal/overlay is open, since native
    // child webviews paint above the DOM and would otherwise obscure it. Covers
    // the New Project modal and the agent launcher overlay.
    const modalOpen = isNewProjectModalOpen || isAgentLauncherOpen
    if (modalOpen) {
      if (activeTab?.type === 'browser') {
        hiddenBrowserTabForModalRef.current = activeTab.browserTabId
        browserTabHide(activeTab.browserTabId).catch(console.error)
      }
      return
    }

    const hiddenBrowserTabId = hiddenBrowserTabForModalRef.current
    if (hiddenBrowserTabId) {
      browserTabShow(hiddenBrowserTabId).catch(console.error)
      hiddenBrowserTabForModalRef.current = null
    }
  }, [isNewProjectModalOpen, isAgentLauncherOpen, activeTab])

  // Listen for optional backend shortcut callbacks. In current Tauri fallback mode this is effectively a future-compat shim.
  useEffect(() => {
    return keyboardApi.onShortcut((shortcut) => {
      switch (shortcut) {
        case 'nextTerminal':
          cycleTab('next')
          break
        case 'prevTerminal':
          cycleTab('prev')
          break
        case 'zoomIn':
          applyZoomAction('zoomIn')
          break
        case 'zoomOut':
          applyZoomAction('zoomOut')
          break
        case 'zoomReset':
          applyZoomAction('zoomReset')
          break
        case 'sidebarToggle':
          void updatePanelVisibility('sidebarVisible', !isSidebarVisible).catch((error) => {
            toast.error(
              error instanceof Error
                ? error.message
                : runtimeT(
                    'workspace',
                    'errors.updateSidebarVisibility',
                    'Failed to update sidebar visibility'
                  )
            )
          })
          break
        case 'colorThemePicker':
          handleOpenThemePicker()
          break
      }
    })
  }, [cycleTab, applyZoomAction, handleOpenThemePicker, updatePanelVisibility, isSidebarVisible])

  const requestTerminateTerminal = useCallback((terminalId: string, tabId?: string) => {
    setCloseConfirmTerminal(null)
    setTerminateConfirmTerminal({ terminalId, tabId })
  }, [])

  const terminateTerminalByRecordId = useCallback(
    async (terminalId: string): Promise<boolean> => {
      const didTerminate = await terminateTerminalResource(terminalId)
      if (!didTerminate) {
        toast.error(
          runtimeT('terminal', 'lifecycle.terminateFailed', 'Failed to terminate terminal process.')
        )
        return false
      }
      useWorkspaceStore.getState().closeTerminalView(terminalId)
      return true
    },
    [terminateTerminalResource]
  )

  const closeTerminalViewByRecordId = useCallback(
    async (terminalRecordId: string): Promise<boolean> => {
      if (closingTerminalIdsRef.current.includes(terminalRecordId)) return false
      setClosingTerminalIds((current) => [...current, terminalRecordId])
      try {
        const didClose = await closeTerminalView(terminalRecordId)
        if (!didClose) {
          toast.error(
            runtimeT(
              'terminal',
              'lifecycle.closeViewFailed',
              'Failed to close terminal view. The process is still running.'
            )
          )
          return false
        }
        useWorkspaceStore.getState().closeTerminalView(terminalRecordId)
        const projectId = useProjectStore.getState().activeProjectId
        const stillHasVisibleProjectTab = getAllLeafPanes(useWorkspaceStore.getState().root).some(
          (leaf) =>
            leaf.tabs.some((tab) => {
              if (tab.type !== 'terminal') return false
              const record = useTerminalStore
                .getState()
                .terminals.find((terminal) => terminal.id === tab.terminalId)
              return record?.projectId === projectId && isOpenTerminalView(record)
            })
        )
        if (!stillHasVisibleProjectTab) {
          projectTerminalDismissedRef.current = true
        }
        return true
      } finally {
        setClosingTerminalIds((current) => current.filter((id) => id !== terminalRecordId))
      }
    },
    [closeTerminalView]
  )

  const handleCloseTerminal = useCallback(
    (id: string, tabId: string) => {
      if (closingTerminalIdsRef.current.includes(id)) return
      const terminal = useTerminalStore
        .getState()
        .terminals.find((candidate) => candidate.id === id)
      // Project shells are not conversation resources. Closing the tab must
      // stop the PTY; otherwise the process keeps consuming memory with no tab.
      if (!terminal || !isConversationScopedTerminal(terminal)) {
        if (confirmTerminalClose) {
          requestTerminateTerminal(id, tabId)
          return
        }
        void terminateTerminalByRecordId(id)
        return
      }
      if (!confirmTerminalClose) {
        void closeTerminalViewByRecordId(id)
        return
      }
      setCloseConfirmRememberChoice(false)
      setCloseConfirmTerminal({ terminalId: id, tabId })
    },
    [
      closeTerminalViewByRecordId,
      confirmTerminalClose,
      requestTerminateTerminal,
      terminateTerminalByRecordId
    ]
  )

  const handleConfirmCloseTerminal = useCallback(async () => {
    if (!closeConfirmTerminal) return
    setCloseConfirmLoading(true)
    try {
      if (closeConfirmRememberChoice) {
        await updateAppSetting('confirmTerminalClose', false)
      }
      const didClose = await closeTerminalViewByRecordId(closeConfirmTerminal.terminalId)
      if (didClose) {
        setCloseConfirmTerminal(null)
        setCloseConfirmRememberChoice(false)
      }
    } finally {
      setCloseConfirmLoading(false)
    }
  }, [
    closeConfirmRememberChoice,
    closeConfirmTerminal,
    closeTerminalViewByRecordId,
    updateAppSetting
  ])

  const handleConfirmTerminateTerminal = useCallback(async () => {
    if (!terminateConfirmTerminal) return
    setTerminateConfirmLoading(true)
    try {
      const didTerminate = await terminateTerminalByRecordId(terminateConfirmTerminal.terminalId)
      if (didTerminate) {
        setTerminateConfirmTerminal(null)
      }
    } finally {
      setTerminateConfirmLoading(false)
    }
  }, [terminateConfirmTerminal, terminateTerminalByRecordId])

  const handleCancelCloseTerminal = useCallback(() => {
    if (closeConfirmLoading) return
    setCloseConfirmRememberChoice(false)
    setCloseConfirmTerminal(null)
  }, [closeConfirmLoading])

  // Keep ref in sync so the keydown effect can call it without declaration-order issues
  handleCloseTerminalRef.current = handleCloseTerminal

  // Dirty file close handlers
  const handleCloseEditorTab = useCallback((filePath: string) => {
    const fileState = useEditorStore.getState().openFiles.get(filePath)
    if (fileState?.operationStatus === 'saving' || fileState?.operationStatus === 'reloading') {
      return
    }
    if (fileState?.isDirty) {
      setDirtyCloseFilePath(filePath)
    } else {
      useEditorStore.getState().closeFileIfIdle(filePath)
      useWorkspaceStore.getState().removeTab(editorTabId(filePath))
    }
  }, [])

  const handleSaveThenClose = useCallback(async () => {
    if (dirtyCloseFilePath) {
      const saved = await useEditorStore.getState().saveFile(dirtyCloseFilePath)
      if (!saved) {
        toast.error(t('failedSaveFile'))
        setDirtyCloseFilePath(null)
        return
      }
      useEditorStore.getState().closeFileIfIdle(dirtyCloseFilePath)
      useWorkspaceStore.getState().removeTab(editorTabId(dirtyCloseFilePath))
      setDirtyCloseFilePath(null)
    }
  }, [dirtyCloseFilePath, t])

  const handleDiscardAndClose = useCallback(() => {
    if (dirtyCloseFilePath) {
      useEditorStore.getState().closeFileIfIdle(dirtyCloseFilePath)
      useWorkspaceStore.getState().removeTab(editorTabId(dirtyCloseFilePath))
      setDirtyCloseFilePath(null)
    }
  }, [dirtyCloseFilePath])

  const handleCancelDirtyClose = useCallback(() => {
    setDirtyCloseFilePath(null)
  }, [])

  // App close dialog handlers
  const handleSaveAllAndClose = useCallback(async () => {
    await useEditorStore.getState().saveAllDirty()
    const remaining = useEditorStore.getState().getDirtyFileCount()
    if (remaining > 0) {
      toast.error(t('someFilesFailedSave'))
      return
    }
    await closeAppWithPersistenceFlush()
  }, [closeAppWithPersistenceFlush, t])

  const handleDiscardAllAndClose = useCallback(() => {
    void closeAppWithPersistenceFlush()
  }, [closeAppWithPersistenceFlush])

  const handleCancelAppClose = useCallback(() => {
    windowApi.respondToClose('cancel')
    setIsAppCloseDialogOpen(false)
  }, [])

  // Command history handlers
  const handleInsertCommand = useCallback(
    (command: string) => {
      // TODO: Route to active terminal pane via context
      if (activeTerminal?.ptyId) {
        terminalApi.write(activeTerminal.ptyId, command)
      }
    },
    [activeTerminal]
  )

  const handleClearCommandHistory = useCallback(async () => {
    if (!activeProjectId) return
    // Persist empty array first, then clear in-memory on success
    const result = await persistenceApi.write(`projects/${activeProjectId}/command-history`, [])
    if (!result.success) {
      toast.error(
        runtimeT('shell', 'commandHistory.clearFailed', 'Failed to clear history: {{error}}', {
          error: result.error
        })
      )
      throw new Error(result.error)
    }
    // Only clear in-memory state after successful persistence
    const { clearHistory } = useCommandHistoryStore.getState()
    clearHistory(activeProjectId)
  }, [activeProjectId])

  const terminalToClose = useTerminalStore
    .getState()
    .terminals.find(
      (terminal) =>
        terminal.id === (terminateConfirmTerminal?.terminalId ?? closeConfirmTerminal?.terminalId)
    )
  const hiddenRunningTerminals = terminals.filter((terminal) =>
    isHiddenRunningTerminal(terminal, {
      conversationId: activeConversationId,
      projectId: activeProjectId
    })
  )

  // Show loading state while projects are being loaded
  if (!isLoaded) {
    if (isMobileWebShell) {
      return (
        <div className="flex h-full min-h-0 flex-col overflow-hidden bg-background">
          <div className="flex flex-1 items-center justify-center">
            <div className="text-sm text-muted-foreground">{t('loading')}</div>
          </div>
        </div>
      )
    }
    return (
      <div className="flex h-full min-h-0 flex-col overflow-hidden bg-background">
        <ResizeEdges />
        <div className="flex-1 flex flex-col overflow-hidden min-h-0 h-full">
          <MacOsTitlebarStrip />
          <div className="flex-1 flex overflow-hidden min-h-0">
            <ActivityRail
              isShortcutsOpen={isShortcutMenuOpen}
              onShortcutsOpenChange={setIsShortcutMenuOpen}
              canOpenGitChanges={false}
            />
            <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
              <TitleBar />
              <div className="flex-1 flex items-center justify-center">
                <div className="text-muted-foreground text-sm">{t('loading')}</div>
              </div>
            </div>
          </div>
        </div>
      </div>
    )
  }

  const workspaceMain = (
    <>
      {activeSSHProfile ? (
        <Suspense fallback={<ShellSkeleton />}>
          <SSHWorkspace profile={sshProfileWithPassword!} conn={sshConn} />
        </Suspense>
      ) : (
        <>
          {isWorkspaceRoute ? (
            <>
              <Outlet />
              <TerminalSwitcherBar
                isListOpen={isTerminalListVisible}
                onToggleList={handleToggleTerminalList}
              />
              {hiddenRunningTerminals.length > 0 && (
                <div className="flex h-8 shrink-0 items-center justify-end border-b border-border/70 bg-sidebar px-2">
                  <HiddenTerminalPopover
                    terminals={hiddenRunningTerminals}
                    onReopen={(terminalId) =>
                      useWorkspaceStore.getState().reopenTerminalView(terminalId)
                    }
                    onStop={requestTerminateTerminal}
                  />
                </div>
              )}
              <motion.div
                key={fullscreenPaneId ? 'fullscreen' : 'normal'}
                initial={{ opacity: 0.88 }}
                animate={{ opacity: 1 }}
                transition={{ duration: 0.14, ease: 'easeOut' }}
                className="flex h-full min-h-0 flex-1 overflow-hidden"
              >
                {/* min-w-0 so the pane group yields width to the list instead of
                    overflowing it off-screen. */}
                <div className="min-w-0 flex-1">
                  <PaneRenderer
                    node={fullscreenPane ?? paneRoot}
                    onAddTerminal={handleAddTerminal}
                    onAddBrowserTab={handleNewBrowserTab}
                    onCloseTerminal={handleCloseTerminal}
                    onRenameTerminal={renameTerminal}
                    onCloseEditorTab={handleCloseEditorTab}
                    closingTerminalIds={closingTerminalIds}
                    defaultShell={activeProject?.defaultShell || appDefaultShell}
                  />
                </div>
                {isTerminalListVisible && <TerminalListPanel />}
              </motion.div>
            </>
          ) : (
            <div className="relative flex-1 overflow-hidden bg-background">
              <div className="h-full w-full">
                <Outlet />
              </div>
            </div>
          )}
          {!isMobileWebShell && <StatusBar project={activeProject} />}
        </>
      )}
    </>
  )

  const appModals = (
    <>
      <NewProjectModal
        isOpen={isNewProjectModalOpen}
        onClose={() => setIsNewProjectModalOpen(false)}
        onImportFromEditor={() => {
          setIsNewProjectModalOpen(false)
          setImportEditorGroupId(null)
          setIsImportEditorOpen(true)
        }}
        onCreateProject={addProject}
      />
      <ImportEditorWorkspacesDialog
        isOpen={isImportEditorOpen}
        targetGroupId={importEditorGroupId}
        onClose={() => {
          setIsImportEditorOpen(false)
          setImportEditorGroupId(null)
        }}
      />

      <Dialog
        open={pendingTerminalRoot !== null}
        onOpenChange={(open) => {
          if (!open) setPendingTerminalRoot(null)
        }}
      >
        <DialogContent className="max-w-xl">
          <DialogHeader>
            <DialogTitle>
              {runtimeT('workspace', 'multiRoot.terminalTitle', 'Choose terminal project')}
            </DialogTitle>
            <DialogDescription>
              {runtimeT(
                'workspace',
                'multiRoot.terminalDescription',
                'Choose a project to create the terminal immediately.'
              )}
            </DialogDescription>
          </DialogHeader>
          <div className="max-h-[min(420px,60vh)] space-y-2 overflow-y-auto pr-1">
            {activeGroupProjects.map((project) => {
              const color = getColorClasses(project.color)
              return (
                <button
                  key={project.id}
                  type="button"
                  className="group flex w-full items-center gap-3 rounded-lg border border-border/70 bg-secondary/20 px-3 py-3 text-left transition-colors hover:border-primary/45 hover:bg-secondary/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  aria-label={`${project.name}: ${project.path}`}
                  onClick={() => {
                    const pending = pendingTerminalRoot
                    if (!pending) return
                    if (activeGroupId) {
                      updateGroup(activeGroupId, { preferredProjectId: project.id })
                    }
                    setPendingTerminalRoot(null)
                    void handleCreateTerminalInPane(pending.paneId, pending.shellName, project.id)
                  }}
                >
                  <span
                    className={cn(
                      'flex size-9 shrink-0 items-center justify-center rounded-md bg-background/70',
                      color.text
                    )}
                    aria-hidden="true"
                  >
                    <FolderGit2 size={18} />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm font-medium text-foreground">
                      {project.name}
                    </span>
                    <span className="mt-0.5 block truncate text-xs text-muted-foreground">
                      {project.path}
                    </span>
                  </span>
                  <SquareTerminal
                    size={17}
                    className="shrink-0 text-muted-foreground transition-colors group-hover:text-primary"
                    aria-hidden="true"
                  />
                </button>
              )
            })}
          </div>
        </DialogContent>
      </Dialog>

      {isThemePickerOpen && (
        <Suspense fallback={null}>
          <ThemePicker />
        </Suspense>
      )}

      {isTerminalSwitcherOpen && (
        <Suspense fallback={null}>
          <TerminalQuickSwitcher
            isOpen={isTerminalSwitcherOpen}
            onClose={() => setIsTerminalSwitcherOpen(false)}
          />
        </Suspense>
      )}

      {isCommandPaletteOpen && (
        <Suspense fallback={null}>
          <CommandPalette
            isOpen={isCommandPaletteOpen}
            onClose={() => setIsCommandPaletteOpen(false)}
            projects={projects}
            onSwitchProject={selectProject}
            onAddTerminal={() => handleAddTerminal(undefined)}
            onShowAgentLauncher={() => {
              const paneId = useWorkspaceStore.getState().activePaneId
              if (paneId) {
                useWorkspaceStore.getState().showAgentLauncher(paneId)
              }
            }}
            onLaunchAgent={handleLaunchAgent}
            onNewBrowserTab={handleNewBrowserTab}
            onSaveSnapshot={handleOpenSnapshotModal}
            onOpenProjectSettings={handleOpenProjectSettings}
            onOpenAppPreferences={handleOpenAppPreferences}
            onOpenCommandHistory={activeProjectId ? handleOpenCommandHistory : undefined}
            onOpenShortcutMenu={handleOpenShortcutMenu}
            onOpenThemePicker={handleOpenThemePicker}
            onToggleCliSessionPanel={() => {
              void updatePanelVisibility('cliSessionPanelVisible', !isCliSessionPanelVisible).catch(
                (error) => {
                  toast.error(
                    error instanceof Error
                      ? error.message
                      : runtimeT(
                          'shell',
                          'titleBar.failedCliSessions',
                          'Failed to update CLI sessions visibility'
                        )
                  )
                }
              )
            }}
            onSSHConnect={handleSSHConnect}
            sshProfiles={sshProfiles.map((p) => ({
              id: p.id,
              name: p.name,
              host: p.host,
              username: p.username
            }))}
            getShortcutLabel={getShortcutLabel}
            getProjectShortcutLabel={getProjectShortcutLabel}
          />
        </Suspense>
      )}

      <CreateSnapshotModal
        isOpen={isCreateSnapshotModalOpen}
        onClose={() => setIsCreateSnapshotModalOpen(false)}
        onCreateSnapshot={handleCreateSnapshot}
      />

      {isCommandHistoryOpen && (
        <Suspense fallback={null}>
          <CommandHistoryModal
            isOpen={isCommandHistoryOpen}
            onClose={() => setIsCommandHistoryOpen(false)}
            entries={commandHistory}
            allEntries={allCommandHistory}
            onSelectCommand={handleInsertCommand}
            onClearHistory={handleClearCommandHistory}
          />
        </Suspense>
      )}

      {/* SSH Password Prompt */}
      {sshPasswordPrompt && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
          <div className="bg-background border border-border rounded-lg shadow-lg w-[360px] p-4">
            <h3 className="text-sm font-semibold mb-1">{t('sshPassword.title')}</h3>
            <p className="text-xs text-muted-foreground mb-3">
              {t('sshPassword.prompt', { name: sshPasswordPrompt.profileName })}
            </p>
            <input
              type="password"
              value={sshPasswordInput}
              onChange={(e) => setSSHPasswordInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') handleSSHPasswordSubmit()
                if (e.key === 'Escape') {
                  setSSHPasswordPrompt(null)
                  setSSHPasswordInput('')
                }
              }}
              placeholder={runtimeT('ssh', 'profile.password', 'Password')}
              autoFocus
              className="w-full px-3 py-1.5 text-sm bg-muted border border-border rounded focus:outline-none focus:ring-1 focus:ring-ring"
            />
            <div className="flex justify-end gap-2 mt-3">
              <button
                type="button"
                onClick={() => {
                  setSSHPasswordPrompt(null)
                  setSSHPasswordInput('')
                }}
                className="px-3 py-1.5 text-xs rounded border border-border hover:bg-accent"
              >
                {runtimeT('ssh', 'actions.cancel', 'Cancel')}
              </button>
              <button
                type="button"
                onClick={handleSSHPasswordSubmit}
                className="px-3 py-1.5 text-xs rounded bg-primary text-primary-foreground hover:bg-primary/90"
              >
                {runtimeT('ssh', 'connection.connect', 'Connect')}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Close Terminal Confirmation */}
      <ConfirmDialog
        isOpen={closeConfirmTerminal !== null}
        title={t('closeTerminal.title')}
        message={t('closeTerminal.message', {
          name: terminalToClose?.name || t('closeTerminal.thisTerminal')
        })}
        confirmLabel={runtimeT('terminal', 'lifecycle.closeView', 'Close view')}
        cancelLabel={t('cancel')}
        secondaryAction={{
          label: runtimeT('terminal', 'lifecycle.terminateProcess', 'Terminate process'),
          onClick: () => {
            if (closeConfirmTerminal) {
              requestTerminateTerminal(closeConfirmTerminal.terminalId, closeConfirmTerminal.tabId)
            }
          }
        }}
        isLoading={closeConfirmLoading}
        onConfirm={handleConfirmCloseTerminal}
        onCancel={handleCancelCloseTerminal}
      >
        <label className="flex items-center gap-2 text-xs text-muted-foreground select-none">
          <input
            type="checkbox"
            checked={closeConfirmRememberChoice}
            onChange={(e) => setCloseConfirmRememberChoice(e.target.checked)}
            disabled={closeConfirmLoading}
            className="rounded border-border bg-background"
          />
          {t('closeTerminal.dontAsk')}
        </label>
      </ConfirmDialog>

      <ConfirmDialog
        isOpen={terminateConfirmTerminal !== null}
        title={runtimeT('terminal', 'lifecycle.terminateTitle', 'Terminate terminal process?')}
        message={runtimeT(
          'terminal',
          'lifecycle.terminateDescription',
          'This stops the live process and removes only its terminal resource. Conversation chat and history are preserved.'
        )}
        confirmLabel={runtimeT('terminal', 'lifecycle.terminateProcess', 'Terminate process')}
        cancelLabel={t('cancel')}
        variant="danger"
        isLoading={terminateConfirmLoading}
        onConfirm={handleConfirmTerminateTerminal}
        onCancel={() => {
          if (!terminateConfirmLoading) setTerminateConfirmTerminal(null)
        }}
      />

      {/* Dirty File Close Confirmation */}
      <ConfirmDialog
        isOpen={dirtyCloseFilePath !== null}
        title={t('unsaved.title')}
        message={t('unsaved.saveChangesBeforeClose', {
          name: dirtyCloseFilePath?.split(/[\\/]/).pop() ?? ''
        })}
        confirmLabel={t('unsaved.save')}
        cancelLabel={t('cancel')}
        secondaryAction={{ label: t('unsaved.discard'), onClick: handleDiscardAndClose }}
        onConfirm={handleSaveThenClose}
        onCancel={handleCancelDirtyClose}
      />

      {/* App Close Unsaved Files Confirmation */}
      <ConfirmDialog
        isOpen={isAppCloseDialogOpen}
        title={t('unsaved.title')}
        message={t('unsaved.unsavedFiles', { count: appCloseDirtyCount })}
        confirmLabel={t('unsaved.saveAll')}
        cancelLabel={t('cancel')}
        secondaryAction={{
          label: t('unsaved.dontSave'),
          onClick: handleDiscardAllAndClose
        }}
        onConfirm={handleSaveAllAndClose}
        onCancel={handleCancelAppClose}
      />
    </>
  )

  if (isMobileWebShell) {
    return (
      <div className="flex h-full min-h-0 flex-col overflow-hidden bg-background">
        <Suspense fallback={<ShellSkeleton />}>
          <MobileChatShell
            onNewChat={handleOpenAgentChat}
            canNewChat
            onOpenCommandPalette={() => setIsCommandPaletteOpen(true)}
            onOpenGitChanges={() => setGitSheetOpen(true)}
            onOpenGitHistory={() => handleAddGitHistoryTab()}
            onNewTerminal={() => handleAddTerminal(undefined)}
            onCloseTerminal={handleCloseTerminal}
            onTerminateTerminal={(terminalId, tabId) => requestTerminateTerminal(terminalId, tabId)}
            onRenameTerminal={renameTerminal}
            onRestartTerminal={(terminalId) => {
              void restartTerminalResource(terminalId).then((restarted) => {
                if (!restarted) {
                  toast.error(
                    runtimeT(
                      'terminal',
                      'lifecycle.restartFailed',
                      'Failed to restart terminal process.'
                    )
                  )
                }
              })
            }}
          >
            <PaneDndProvider>
              <main className="flex h-full min-h-0 flex-col overflow-hidden bg-background">
                <WorkspaceConflictBanner conversationId={activeConversationId} />
                {workspaceMain}
              </main>
            </PaneDndProvider>
          </MobileChatShell>
        </Suspense>

        {/* Mobile-only full-width Git Changes sheet. GitPanel branches on
            useMobileWebShell() internally to render a single-column stacked
            layout (file list → diff + back). Only mounted in the mobile path
            so the desktop two-column GitPanel (a workspace tab) is untouched.
            The `open` prop is gated on `activeProject?.path` in addition to
            `gitSheetOpen` so the sheet can never be open during the
            empty-content race when the active project loses its path; the
            `useEffect` below also resets `gitSheetOpen` to keep state honest. */}
        <Sheet open={gitSheetOpen && Boolean(activeProject?.path)} onOpenChange={setGitSheetOpen}>
          <SheetContent side="bottom" className="h-full p-0" aria-label={t('gitChangesSheet')}>
            {activeProject?.path ? (
              <Suspense fallback={<ShellSkeleton />}>
                <GitPanel cwd={activeProject.path} isVisible={gitSheetOpen} />
              </Suspense>
            ) : null}
          </SheetContent>
        </Sheet>

        {appModals}
      </div>
    )
  }

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden bg-background">
      <ResizeEdges />
      <div className="flex-1 flex flex-col overflow-hidden min-h-0 h-full">
        <MacOsTitlebarStrip />
        <div className="flex-1 flex overflow-hidden min-h-0">
          <ActivityRail
            isShortcutsOpen={isShortcutMenuOpen}
            onShortcutsOpenChange={setIsShortcutMenuOpen}
            onOpenGitChanges={() => handleAddGitTab()}
            canOpenGitChanges={Boolean(activeProject?.path)}
            onOpenGitHistory={() => handleAddGitHistoryTab()}
            canOpenGitHistory={Boolean(activeProject?.path)}
            isThemePickerOpen={isThemePickerOpen}
            onToggleThemePicker={handleToggleThemePicker}
          />
          <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
            <TitleBar />

            <div className="flex-1 flex overflow-hidden min-h-0 h-full">
              {/* Sidebar */}
              <PanelFade
                open={isSidebarVisible}
                data-testid="sidebar-panel-fade"
                className="h-full min-h-0 overflow-hidden border-r border-sidebar-border/70"
              >
                <ResizableRail
                  side="left"
                  storageKey="termul:sidebar-width"
                  initial={240}
                  min={180}
                  max={420}
                  resizeTitle={t('sidePanels.resizeSidebar')}
                  resizeLabel={t('sidePanels.resizeSidebarAria')}
                >
                  {isConversationRoute ? (
                    <ConversationSidebar onNewChat={handleOpenAgentChat} />
                  ) : (
                    <ProjectSidebar
                      projects={projects}
                      activeProjectId={activeProjectId}
                      onSelectProject={handleSelectProject}
                      onOpenProjectTerminal={handleOpenProjectTerminal}
                      onSelectGroup={handleSelectGroup}
                      onNewProject={() => setIsNewProjectModalOpen(true)}
                      onImportFromEditor={(groupId) => {
                        setImportEditorGroupId(groupId ?? null)
                        setIsImportEditorOpen(true)
                      }}
                      onUpdateProject={updateProject}
                      onDeleteProject={deleteProject}
                      onArchiveProject={archiveProject}
                      onRestoreProject={restoreProject}
                      onReorderProjects={reorderProjects}
                      onSSHConnect={handleSSHConnect}
                      onSelectSSHProfile={handleSelectSSHProfile}
                      activeSSHProfileId={activeSSHProfileId}
                    />
                  )}
                </ResizableRail>
              </PanelFade>

              {/* Main Content and File Explorer Container */}
              <PaneDndProvider>
                <div className="flex-1 flex min-h-0 h-full gap-0 overflow-hidden min-w-0">
                  {/* Main Content Area */}
                  <main className="relative flex-1 flex flex-col min-w-0 bg-background overflow-hidden">
                    <WorkspaceConflictBanner conversationId={activeConversationId} />
                    {workspaceMain}
                    {isConversationListRoute && isAgentLauncherOpen ? (
                      <div
                        className="absolute inset-0 z-30 flex flex-col bg-background/95 backdrop-blur-sm"
                        role="dialog"
                        aria-modal="true"
                        aria-label={runtimeT('workspace', 'pane.agentLauncher', 'Agent launcher')}
                      >
                        <button
                          type="button"
                          className="absolute right-3 top-3 z-10 flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-muted/60 hover:text-foreground"
                          aria-label={runtimeT(
                            'workspace',
                            'pane.closeAgentLauncher',
                            'Close agent launcher'
                          )}
                          onClick={() => useWorkspaceStore.getState().hideAgentLauncher()}
                        >
                          <X className="h-4 w-4" aria-hidden="true" />
                        </button>
                        <AgentLauncher
                          paneId={useWorkspaceStore.getState().activePaneId}
                          onLaunched={(conversationId) => {
                            useWorkspaceStore.getState().hideAgentLauncher()
                            navigate(`/c/${conversationId}`)
                          }}
                        />
                      </div>
                    ) : null}
                    <LastSessionNotice onRestore={handleOpenProjectTerminal} />
                  </main>

                  {/* File Explorer / CLI sessions / SSH - integrated right rail */}
                  <PanelFade
                    open={Boolean(
                      (isExplorerVisible && explorerRootVisible) ||
                        isCliSessionPanelVisible ||
                        activeSSHProfile
                    )}
                    data-testid="explorer-rail-fade"
                    className="h-full min-h-0 border-l border-border/70"
                  >
                    <ResizableRail
                      side="right"
                      storageKey="termul:file-explorer-width"
                      initial={256}
                      min={220}
                      max={560}
                      className="bg-sidebar"
                      resizeTitle={t('sidePanels.resizeRightRail')}
                      resizeLabel={t('sidePanels.resizeRightRailAria')}
                    >
                      <div className="flex h-full min-h-0 flex-col">
                        <PanelFade
                          open={isCliSessionPanelVisible}
                          data-testid="cli-session-panel-fade"
                          className={
                            (isExplorerVisible && explorerRootVisible) || activeSSHProfile
                              ? 'relative min-h-0 shrink-0'
                              : 'min-h-0 flex-1'
                          }
                        >
                          <div
                            className="h-full min-h-0"
                            style={
                              (isExplorerVisible && explorerRootVisible) || activeSSHProfile
                                ? { height: cliRailHeight }
                                : undefined
                            }
                          >
                            <Suspense fallback={<ShellSkeleton />}>
                              <CliSessionPanel />
                            </Suspense>
                          </div>
                          {(isExplorerVisible && explorerRootVisible) || activeSSHProfile ? (
                            <PanelResizeHandle
                              axis="y"
                              value={cliRailHeight}
                              min={140}
                              max={720}
                              onChange={setCliRailHeight}
                              title={t('sidePanels.resizeCliSplit')}
                              label={t('sidePanels.resizeCliSplitAria')}
                              className="bottom-0"
                            />
                          ) : null}
                        </PanelFade>
                        <PanelFade
                          open={Boolean(isExplorerVisible && explorerRootVisible)}
                          data-testid="file-explorer-panel-fade"
                          className={
                            activeSSHProfile || isCliSessionPanelVisible
                              ? 'min-h-0 flex-1'
                              : 'h-full'
                          }
                        >
                          <Suspense fallback={<ShellSkeleton />}>
                            <FileExplorer side="right" fillContainer />
                          </Suspense>
                        </PanelFade>
                        {activeSSHProfile && (
                          <div className="flex min-h-0 flex-1 flex-col overflow-hidden border-t border-border/70 bg-sidebar">
                            <Suspense fallback={<ShellSkeleton />}>
                              <SSHFileExplorer
                                connectionId={sshConn.connectionId ?? ''}
                                isConnected={sshConn.isConnected}
                                sftpReady={sshConn.sftpReady}
                                entries={sshConn.entries}
                                currentPath={sshConn.currentPath}
                                expandedDirs={sshConn.expandedDirs}
                                childEntries={sshConn.childEntries}
                                loadingDirs={sshConn.loadingDirs}
                                isLoadingRoot={sshConn.isLoadingRoot}
                                profileName={activeSSHProfile.name}
                                onConnect={sshConn.handleConnect}
                                onBrowseFiles={sshConn.handleBrowseFiles}
                                onToggleDir={sshConn.toggleDirectory}
                                onLoadDir={sshConn.loadDirectory}
                                onMkdir={handleSSHMkdir}
                                onCreateFile={handleSSHCreateFile}
                                onDelete={handleSSHDelete}
                                onRename={handleSSHRename}
                              />
                            </Suspense>
                          </div>
                        )}
                      </div>
                    </ResizableRail>
                  </PanelFade>
                </div>
              </PaneDndProvider>
            </div>
          </div>
        </div>
      </div>

      {/* Modals */}
      {appModals}
    </div>
  )
}
