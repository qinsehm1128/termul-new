import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { type MutableRefObject, useEffect } from 'react'
import { MemoryRouter, Route, Routes, useNavigate } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import { persistState, restoreProjectWorkspace } from '@/hooks/use-editor-persistence'
import WorkspaceDashboard from '@/pages/WorkspaceDashboard'
import { useConversationStore } from '@/stores/conversation-store'
import { useFileExplorerStore } from '@/stores/file-explorer-store'
import { useSessionWorkspaceSyncStore } from '@/stores/session-workspace-sync-store'
import { useSidebarStore } from '@/stores/sidebar-store'
import { useThemePickerStore } from '@/stores/theme-picker-store'
import { useWorkspaceStore } from '@/stores/workspace-store'
import type { Project, ProjectColor, Terminal } from '@/types/project'
import WorkspaceLayout from './WorkspaceLayout'

const { platformState, tauriRef } = vi.hoisted(() => ({
  platformState: { isMac: false },
  tauriRef: { current: true as boolean }
}))

vi.mock('@/lib/tauri-runtime', async () => {
  const actual = await vi.importActual<typeof import('@/lib/tauri-runtime')>('@/lib/tauri-runtime')
  return { ...actual, isTauriContext: () => tauriRef.current }
})

vi.mock('@/lib/platform', async () => {
  const actual = await vi.importActual<typeof import('@/lib/platform')>('@/lib/platform')
  return {
    ...actual,
    get isMac() {
      return platformState.isMac
    }
  }
})

function createProject(id: string, path: string, color: ProjectColor): Project {
  return {
    id,
    name: id.toUpperCase(),
    color,
    path,
    gitBranch: 'main',
    isActive: true
  }
}

// Mock the store hooks
const mockUseProjectsLoaded = vi.fn(() => true)
const mockUseProjects = vi.fn((): Project[] => [])
const mockUseActiveProject = vi.fn((): Project | null => null)
const mockUseActiveProjectId = vi.fn((): string => '')
const mockUseProjectActions = vi.fn(() => ({
  selectProject: vi.fn(),
  addProject: vi.fn(),
  updateProject: vi.fn(),
  deleteProject: vi.fn(),
  archiveProject: vi.fn(),
  restoreProject: vi.fn(),
  reorderProjects: vi.fn()
}))

const mockUseTerminals = vi.fn((): Terminal[] => [])
const mockUseAllTerminals = vi.fn((): Terminal[] => [])
const mockUseActiveTerminal = vi.fn((): Terminal | null => null)
const mockUseActiveTerminalId = vi.fn((): string => '')
const mockCloseTerminalView = vi.fn(async () => true)
const mockTerminateTerminalResource = vi.fn(async () => true)
const mockUseTerminalActions = vi.fn(() => ({
  selectTerminal: vi.fn(),
  addTerminal: vi.fn(),
  closeTerminal: vi.fn(),
  closeTerminalView: mockCloseTerminalView,
  reopenTerminalView: vi.fn(),
  terminateTerminalResource: mockTerminateTerminalResource,
  restartTerminalResource: vi.fn(async () => true),
  renameTerminal: vi.fn(),
  reorderTerminals: vi.fn(),
  setTerminalPtyId: vi.fn(),
  clearTerminalPtyId: vi.fn()
}))

vi.mock('@/stores/project-store', () => ({
  useProjectStore: Object.assign(
    vi.fn((selector) => {
      const state = {
        projects: [],
        activeProjectId: '',
        groups: [],
        activeGroupId: null,
        isLoaded: true,
        isWorktreeOperationLocked: false
      }
      return selector ? selector(state) : state
    }),
    {
      getState: vi.fn(() => ({
        projects: [],
        activeProjectId: '',
        groups: [],
        activeGroupId: null,
        isLoaded: true,
        isWorktreeOperationLocked: false,
        removeWorktree: vi.fn(),
        updateProject: vi.fn()
      }))
    }
  ),
  useProjectsLoaded: () => mockUseProjectsLoaded(),
  useProjects: () => mockUseProjects(),
  useActiveProject: () => mockUseActiveProject(),
  useActiveProjectId: () => mockUseActiveProjectId(),
  useProjectActions: () => mockUseProjectActions()
}))

vi.mock('@/stores/terminal-store', () => ({
  useTerminalStore: Object.assign(
    vi.fn((selector) =>
      selector({
        terminals: mockUseTerminals(),
        activeTerminalId: mockUseActiveTerminalId(),
        // Present in the real store's initial state and non-optional in its
        // type. Omitting it here made every consumer that reads the MRU stack
        // crash on `undefined` rather than exercise the branch.
        recentTerminalIds: []
      })
    ),
    {
      getState: () => ({
        terminals: mockUseTerminals(),
        activeTerminalId: mockUseActiveTerminalId(),
        recentTerminalIds: [],
        findTerminalByPtyId: (ptyId: string) =>
          mockUseTerminals().find((terminal) => terminal.ptyId === ptyId),
        selectTerminal: vi.fn(),
        isTerminalLimitReached: vi.fn(() => false)
      })
    }
  ),
  useTerminals: () => mockUseTerminals(),
  useAllTerminals: () => mockUseAllTerminals(),
  useActiveTerminal: () => mockUseActiveTerminal(),
  useActiveTerminalId: () => mockUseActiveTerminalId(),
  useTerminalActions: () => mockUseTerminalActions(),
  useProjectsWithActivity: () => [],
  useProjectsWithErrors: () => new Set<string>(),
  cleanupProjectTerminals: vi.fn()
}))

vi.mock('@/stores/app-settings-store', () => ({
  useAppSettingsStore: vi.fn(
    (selector?: (state: { settings: { remoteBindMode: 'localhost' } }) => unknown) => {
      const state = { settings: { remoteBindMode: 'localhost' as const } }
      return selector ? selector(state) : state
    }
  ),
  useTerminalFontSize: vi.fn(() => 14),
  useUiZoomLevel: vi.fn(() => 1),
  useTerminalFontFamily: vi.fn(() => 'monospace'),
  useTerminalSymbolFontFamily: vi.fn(() => ''),
  useTerminalBufferSize: vi.fn(() => 10000),
  useDefaultShell: vi.fn(() => 'bash'),
  useMaxTerminalsPerProject: vi.fn(() => 10),
  useConfirmTerminalClose: vi.fn(() => true),
  useUpdateAppSetting: vi.fn(() => vi.fn()),
  useDefaultProjectColor: vi.fn(() => 'blue'),
  useColorTheme: vi.fn(() => 'termul'),
  useAppearanceMode: vi.fn(() => 'dark')
}))

vi.mock('@/stores/remote-status-store', () => ({
  useRemoteStatus: vi.fn(() => null),
  useRemoteRestoreError: vi.fn(() => null),
  useRemoteStatusStore: Object.assign(vi.fn(), {
    getState: vi.fn(() => ({ setStatus: vi.fn(), setRestoreError: vi.fn() }))
  })
}))

vi.mock('@/lib/api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/api')>()
  return {
    ...actual,
    remoteServerApi: {
      start: vi.fn(),
      stop: vi.fn(),
      status: vi.fn(),
      intent: vi.fn(() =>
        Promise.resolve({ success: true, data: { wanted: false, publishMode: 'tunnel' } })
      ),
      setIntent: vi.fn(),
      rotateCredential: vi.fn()
    },
    openerApi: {
      openUrlWithSystemBrowser: vi.fn(() => Promise.resolve({ success: true, data: undefined }))
    }
  }
})

vi.mock('@/stores/keyboard-shortcuts-store', async () => {
  const actual = await vi.importActual<typeof import('@/stores/keyboard-shortcuts-store')>(
    '@/stores/keyboard-shortcuts-store'
  )

  const shortcuts = {
    commandPalette: { customKey: 'ctrl+k', defaultKey: 'ctrl+k' },
    commandPaletteAlt: { customKey: 'ctrl+shift+p', defaultKey: 'ctrl+shift+p' },
    terminalSearch: { customKey: 'ctrl+f', defaultKey: 'ctrl+f' },
    commandHistory: { customKey: 'ctrl+r', defaultKey: 'ctrl+r' },
    newProject: { customKey: 'ctrl+n', defaultKey: 'ctrl+n' },
    newTerminal: { customKey: 'ctrl+t', defaultKey: 'ctrl+t' },
    nextTerminal: { customKey: 'ctrl+pagedown', defaultKey: 'ctrl+pagedown' },
    prevTerminal: { customKey: 'ctrl+pageup', defaultKey: 'ctrl+pageup' },
    zoomIn: { customKey: 'ctrl+=', defaultKey: 'ctrl+=' },
    zoomOut: { customKey: 'ctrl+-', defaultKey: 'ctrl+-' },
    zoomReset: { customKey: 'ctrl+0', defaultKey: 'ctrl+0' },
    sidebarToggle: { customKey: 'ctrl+shift+b', defaultKey: 'ctrl+shift+b' },
    closeTab: { customKey: 'ctrl+w', defaultKey: 'ctrl+w' },
    saveFile: { customKey: 'ctrl+s', defaultKey: 'ctrl+s' },
    toggleFileExplorer: { customKey: 'ctrl+b', defaultKey: 'ctrl+b' },
    toggleCliSessionPanel: { customKey: 'ctrl+shift+h', defaultKey: 'ctrl+shift+h' },
    newBrowserTab: { customKey: 'ctrl+shift+n', defaultKey: 'ctrl+shift+n' },
    colorThemePicker: { customKey: 'ctrl+alt+t', defaultKey: 'ctrl+alt+t' }
  }

  return {
    ...actual,
    useKeyboardShortcutsStore: vi.fn(
      (selector?: (state: { shortcuts: typeof shortcuts }) => unknown) => {
        const state = { shortcuts }
        return selector ? selector(state) : state
      }
    ),
    matchesShortcut: actual.matchesShortcut
  }
})

// Mock hooks
vi.mock('@/hooks/use-snapshots', () => ({
  useCreateSnapshot: vi.fn(() => vi.fn().mockResolvedValue(undefined)),
  useSnapshotLoader: vi.fn()
}))

vi.mock('@/hooks/use-recent-commands', () => ({
  useRecentCommandsLoader: vi.fn(),
  useRecentCommandIds: vi.fn(() => []),
  useSaveRecentCommand: vi.fn()
}))

vi.mock('@/hooks/use-pinned-commands', () => ({
  usePinnedCommandsLoader: vi.fn(),
  usePinnedCommandIds: vi.fn(() => []),
  useTogglePinnedCommand: vi.fn()
}))

vi.mock('@/hooks/use-command-history', () => ({
  useCommandHistoryLoader: vi.fn(),
  useAddCommand: vi.fn(() => vi.fn()),
  useCommandHistory: vi.fn(() => []),
  useAllCommandHistory: vi.fn(() => [])
}))

const { mockUpdatePanelVisibility, mockWaitForPendingAppSettingsPersistence } = vi.hoisted(() => ({
  mockUpdatePanelVisibility: vi.fn(() => Promise.resolve()),
  mockWaitForPendingAppSettingsPersistence: vi.fn(() => Promise.resolve())
}))

vi.mock('@/hooks/use-app-settings', () => ({
  useUpdateAppSetting: vi.fn(() => vi.fn()),
  useUpdateAppSettings: vi.fn(() => vi.fn()),
  useUpdatePanelVisibility: vi.fn(() => mockUpdatePanelVisibility),
  waitForPendingAppSettingsPersistence: mockWaitForPendingAppSettingsPersistence
}))

vi.mock('@/hooks/use-file-watcher', () => ({
  useFileWatcher: vi.fn()
}))

vi.mock('@/hooks/use-editor-persistence', () => ({
  useEditorPersistence: vi.fn(),
  persistState: vi.fn(),
  restoreProjectWorkspace: vi.fn().mockResolvedValue(false),
  subscribeProjectWorkspaceRestored: vi.fn(() => () => {})
}))

// P17: shared canonical mock shape for the Story 6 sync hook + banner —
// identical inline factories across the three WorkspaceLayout suites so a
// future export-surface refactor breaks all three with the same error.
vi.mock('@/hooks/use-workspace-manifest-sync', () => ({
  useWorkspaceManifestSync: vi.fn(),
  loadWorkspaceManifest: vi.fn().mockResolvedValue(false),
  resolveManifestConflict: vi.fn().mockResolvedValue(undefined),
  performManifestWrite: vi.fn().mockResolvedValue(undefined)
}))
vi.mock('@/hooks/use-session-workspace-sync', () => ({
  useSessionWorkspaceSync: vi.fn(),
  useSessionWorkspaceBootstrap: vi.fn(),
  loadSessionWorkspace: vi.fn().mockResolvedValue(false),
  resolveSessionWorkspaceConflict: vi.fn().mockResolvedValue(undefined),
  resolveSessionWorkspaceRecovery: vi.fn().mockResolvedValue(undefined),
  performSessionWorkspaceWrite: vi.fn().mockResolvedValue('skipped')
}))
vi.mock('@/components/workspace/WorkspaceConflictBanner', () => ({
  WorkspaceConflictBanner: () => <div data-testid="workspace-conflict-banner" />
}))

const { mockSaveTerminalLayout } = vi.hoisted(() => ({
  mockSaveTerminalLayout: vi.fn(() => Promise.resolve())
}))
vi.mock('@/hooks/useTerminalAutoSave', () => ({
  saveTerminalLayout: mockSaveTerminalLayout,
  useTerminalAutoSave: vi.fn(),
  loadPersistedTerminals: vi.fn(),
  setTerminalRestoreInProgress: vi.fn(),
  syncScrollbackToStore: vi.fn(),
  serializeTerminalsForProject: vi.fn()
}))

vi.mock('@/components/cli-sessions/CliSessionPanel', () => ({
  CliSessionPanel: () => <div>cli-session-panel</div>
}))
vi.mock('@/components/file-explorer/FileExplorer', () => ({
  FileExplorer: () => <div data-testid="file-explorer" />
}))

vi.mock('@/components/workspace/PaneRenderer', () => ({
  PaneRenderer: () => <div data-testid="pane-renderer" />
}))

// Mock the active Tauri API seam used by WorkspaceLayout and nested components.
const { mockApi } = vi.hoisted(() => ({
  mockApi: {
    keyboard: {
      onShortcut: vi.fn((_callback: (shortcut: string) => void) => vi.fn())
    },
    shell: {
      getAvailableShells: vi
        .fn()
        .mockResolvedValue({ success: true, data: { default: null, available: [] } })
    },
    terminal: {
      getGitBranch: vi.fn().mockResolvedValue({ success: true, data: 'main' }),
      getGitStatus: vi.fn().mockResolvedValue({ success: true, data: { hasChanges: false } }),
      onData: vi.fn(() => vi.fn()),
      onTitleChange: vi.fn(() => vi.fn()),
      onExit: vi.fn(() => vi.fn()),
      spawn: vi.fn().mockResolvedValue({ success: true, data: 'mock-pty-id' }),
      resize: vi.fn().mockResolvedValue({ success: true }),
      closeView: vi.fn().mockResolvedValue({ success: true }),
      terminate: vi.fn().mockResolvedValue({ success: true }),
      kill: vi.fn().mockResolvedValue({ success: true }),
      write: vi.fn().mockResolvedValue({ success: true })
    },
    filesystem: {
      onFileChanged: vi.fn(() => vi.fn()),
      onFileCreated: vi.fn(() => vi.fn()),
      onFileDeleted: vi.fn(() => vi.fn()),
      onSearchFileNamesBatch: vi.fn(() => vi.fn()),
      onSearchFileNamesDone: vi.fn(() => vi.fn()),
      watchDirectory: vi.fn().mockResolvedValue({ success: true }),
      unwatchDirectory: vi.fn().mockResolvedValue({ success: true }),
      readDirectory: vi.fn().mockResolvedValue({ success: true, data: [] })
    },
    system: {
      getHomeDirectory: vi.fn().mockResolvedValue({ success: true, data: '/home/user' }),
      getAvailableShells: vi.fn().mockResolvedValue({ success: true, data: [] })
    },
    persistence: {
      writeDebounced: vi.fn(() => Promise.resolve({ success: true, data: undefined })),
      read: vi.fn(() => Promise.resolve({ success: true, data: null })),
      write: vi.fn(() => Promise.resolve({ success: true, data: undefined })),
      delete: vi.fn(() => Promise.resolve({ success: true, data: undefined })),
      flushPendingWrites: vi.fn(() => Promise.resolve({ success: true, data: undefined }))
    },
    window: {
      minimize: vi.fn(),
      toggleMaximize: vi.fn().mockResolvedValue({ success: true, data: false }),
      close: vi.fn(),
      onMaximizeChange: vi.fn(() => vi.fn()),
      onCloseRequested: vi.fn<(callback: () => Promise<boolean>) => () => void>((_callback) =>
        vi.fn()
      ),
      respondToClose: vi.fn()
    },
    clipboard: {
      readText: vi.fn().mockResolvedValue({ success: true, data: '' }),
      writeText: vi.fn().mockResolvedValue({ success: true })
    },
    dialog: {
      selectDirectory: vi.fn(),
      selectFile: vi.fn(),
      saveFile: vi.fn(),
      showConfirm: vi.fn(),
      showMessage: vi.fn()
    },
    visibility: {
      setVisibilityState: vi.fn()
    },
    session: {
      save: vi.fn(),
      restore: vi.fn(),
      clear: vi.fn(),
      flush: vi.fn(),
      hasSession: vi.fn()
    },
    dataMigration: {
      getVersion: vi.fn(),
      getSchemaInfo: vi.fn(),
      getHistory: vi.fn(),
      getRegistered: vi.fn(),
      runMigration: vi.fn(),
      rollback: vi.fn()
    },
    addRendererRef: vi.fn(),
    removeRendererRef: vi.fn(),
    hasActiveTerminalSessions: vi.fn()
  }
}))

vi.mock('@/lib/api', () => ({
  keyboardApi: mockApi.keyboard,
  shellApi: mockApi.shell,
  terminalApi: mockApi.terminal,
  filesystemApi: mockApi.filesystem,
  systemApi: mockApi.system,
  persistenceApi: mockApi.persistence,
  windowApi: mockApi.window,
  clipboardApi: mockApi.clipboard,
  dialogApi: mockApi.dialog,
  visibilityApi: mockApi.visibility,
  sessionApi: mockApi.session,
  dataMigrationApi: mockApi.dataMigration,
  addRendererRef: mockApi.addRendererRef,
  removeRendererRef: mockApi.removeRendererRef,
  hasActiveTerminalSessions: mockApi.hasActiveTerminalSessions,
  sshApi: { onConnectionStatusChanged: vi.fn(() => vi.fn()) },
  tauriUpdaterApi: {},
  tauriVersionSkipService: {},
  remoteServerApi: {
    start: vi.fn(),
    stop: vi.fn(),
    status: vi.fn(),
    intent: vi.fn(() =>
      Promise.resolve({ success: true, data: { wanted: false, publishMode: 'tunnel' } })
    ),
    setIntent: vi.fn(),
    rotateCredential: vi.fn()
  },
  openerApi: {
    openUrlWithSystemBrowser: vi.fn(() => Promise.resolve({ success: true, data: undefined }))
  },
  editorWorkspaceApi: {
    list: vi.fn(() => Promise.resolve({ success: true, data: { candidates: [] } })),
    parseFile: vi.fn(() => Promise.resolve({ success: true, data: { candidates: [] } }))
  }
}))

beforeEach(() => {
  platformState.isMac = false
  vi.stubGlobal('api', mockApi)
  if (!HTMLElement.prototype.scrollIntoView) {
    HTMLElement.prototype.scrollIntoView = vi.fn()
  }
  // Reset mocks
  mockUseProjectsLoaded.mockReturnValue(true)
  mockUseProjects.mockReturnValue([])
  mockUseActiveProject.mockReturnValue(null)
  mockUseActiveProjectId.mockReturnValue('')
  mockUseTerminals.mockReturnValue([])
  mockUseAllTerminals.mockReturnValue([])
  mockUseActiveTerminal.mockReturnValue(null)
  mockUseActiveTerminalId.mockReturnValue('')
  mockCloseTerminalView.mockReset()
  mockCloseTerminalView.mockResolvedValue(true)
  mockTerminateTerminalResource.mockReset()
  mockTerminateTerminalResource.mockResolvedValue(true)
  mockUpdatePanelVisibility.mockReset()
  mockWaitForPendingAppSettingsPersistence.mockReset()
  useConversationStore.getState().reset()
  useFileExplorerStore.setState({ isVisible: true })
  useSessionWorkspaceSyncStore.setState({ activeConversationId: null })
  useSidebarStore.setState({ isVisible: true })
  useThemePickerStore.getState().close()
  mockApi.filesystem.watchDirectory.mockReset()
  mockApi.filesystem.unwatchDirectory.mockReset()
  mockApi.filesystem.watchDirectory.mockResolvedValue({ success: true })
  mockApi.keyboard.onShortcut.mockReset()
  mockApi.keyboard.onShortcut.mockImplementation((_callback: (shortcut: string) => void) => vi.fn())
  mockApi.persistence.flushPendingWrites.mockReset()
  mockApi.persistence.flushPendingWrites.mockResolvedValue({ success: true, data: undefined })
  mockApi.window.onCloseRequested.mockReset()
  mockApi.window.onCloseRequested.mockImplementation(() => vi.fn())
  mockApi.window.respondToClose.mockReset()
})

afterEach(() => {
  vi.clearAllTimers()
})

// Helper to render with router
const renderWithRouter = (initialEntries = ['/c/018f7a1c-1b4d-7c8a-9f01-0123456789ab']) => {
  return render(
    <TooltipProvider>
      <MemoryRouter initialEntries={initialEntries}>
        <WorkspaceLayout />
      </MemoryRouter>
    </TooltipProvider>
  )
}

function WorkspaceLayoutWithNavigate({
  navigateRef
}: {
  navigateRef: MutableRefObject<((to: string) => void) | null>
}) {
  const navigate = useNavigate()
  useEffect(() => {
    navigateRef.current = navigate
  }, [navigate, navigateRef])
  return <WorkspaceLayout />
}

describe('WorkspaceLayout - Empty States', () => {
  it('locks the desktop shell to the root height without viewport overflow', () => {
    const view = renderWithRouter()

    expect(view.container.firstElementChild).toHaveClass('h-full', 'min-h-0', 'overflow-hidden')
    expect(view.container.firstElementChild).not.toHaveClass('h-screen')
  })

  it('renders the regular project workspace at root without the conversation area', async () => {
    render(
      <TooltipProvider>
        <MemoryRouter initialEntries={['/']}>
          <Routes>
            <Route path="/" element={<WorkspaceLayout />}>
              <Route path="conversations" element={<WorkspaceDashboard />} />
            </Route>
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    )

    expect(await screen.findByTestId('pane-renderer')).toBeVisible()
    expect(
      screen.queryByRole('heading', { name: 'Your Conversation workspace' })
    ).not.toBeInTheDocument()
    // Exclusive sidebars: root shows the project sidebar, never the conversation one.
    expect(screen.queryByLabelText('Search conversations')).not.toBeInTheDocument()
  })

  it('renders the independent conversation area only on the conversations route', async () => {
    render(
      <TooltipProvider>
        <MemoryRouter initialEntries={['/conversations']}>
          <Routes>
            <Route path="/" element={<WorkspaceLayout />}>
              <Route path="conversations" element={<WorkspaceDashboard />} />
            </Route>
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    )

    expect(
      await screen.findByRole('heading', { name: 'Your Conversation workspace' })
    ).toBeVisible()
    expect(screen.getByRole('button', { name: 'New Chat' })).toBeEnabled()
    expect(screen.getByLabelText('Search conversations')).toBeVisible()
    expect(screen.queryByTestId('pane-renderer')).not.toBeInTheDocument()
  })

  it('renders a full-width macOS titlebar zone above workspace chrome', () => {
    platformState.isMac = true

    renderWithRouter()

    const strip = document.querySelector('[data-testid="macos-titlebar-strip"]')
    expect(strip).not.toBeNull()
    expect(strip?.className).toContain('h-8')
    // Panel-visibility toggles were relocated into the macOS titlebar strip.
    expect(strip?.querySelector('button[title="Toggle sidebar"]')).not.toBeNull()
    expect(strip?.querySelector('button[title="Toggle file explorer"]')).not.toBeNull()
    // Patch 18: the Story 6 conflict banner is mounted at the workspace root.
    expect(screen.getByTestId('workspace-conflict-banner')).toBeInTheDocument()
  })

  it('renders active project name in macOS titlebar strip when a project is active', () => {
    platformState.isMac = true
    mockUseActiveProject.mockReturnValue(createProject('my-app', '/workspace/my-app', 'blue'))

    renderWithRouter()

    expect(screen.getByText('MY-APP')).toBeInTheDocument()
  })

  it('does not render project name in macOS titlebar strip when no project is active', () => {
    platformState.isMac = true

    renderWithRouter()

    const strip = document.querySelector('[data-testid="macos-titlebar-strip"]')
    expect(strip).not.toBeNull()
    expect(strip?.querySelector('span')).not.toBeTruthy()
  })

  it('persists terminal layout before unload when a project is active', async () => {
    mockUseActiveProjectId.mockReturnValue('project-1')
    mockUseActiveProject.mockReturnValue(createProject('project-1', '/workspace/project-1', 'blue'))

    renderWithRouter()

    const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
    act(() => useConversationStore.getState().setActiveConversationId(conversationId))
    const { useSessionWorkspaceSync } = await import('@/hooks/use-session-workspace-sync')
    expect(useSessionWorkspaceSync).toHaveBeenCalledWith(conversationId)

    window.dispatchEvent(new Event('beforeunload'))

    await waitFor(() => {
      expect(mockSaveTerminalLayout).toHaveBeenCalledWith('project-1')
    })
  })

  describe('Zero-project Conversation state', () => {
    beforeEach(() => {
      mockUseProjects.mockReturnValue([])
      mockUseActiveProject.mockReturnValue(null)
      mockUseActiveProjectId.mockReturnValue('')
    })

    it('keeps the workspace and global Conversation navigation usable with no projects', () => {
      renderWithRouter()

      expect(screen.getByText('Conversations')).toBeInTheDocument()
      expect(screen.getByRole('button', { name: 'New Chat' })).toBeEnabled()
      expect(screen.getByTestId('pane-renderer')).toBeInTheDocument()
    })

    it('keeps project-only file explorer and Git controls gated', () => {
      renderWithRouter()

      expect(screen.queryByTestId('file-explorer')).not.toBeInTheDocument()
      expect(screen.getByRole('button', { name: 'Open git changes' })).toBeDisabled()
    })

    it('keeps project-less New Chat enabled on the web root', () => {
      tauriRef.current = false
      renderWithRouter()

      expect(screen.getByRole('button', { name: 'New Chat' })).toBeEnabled()
    })
  })

  describe('Empty Workspace Pane State', () => {
    beforeEach(() => {
      // Set up a project but no terminals/tabs
      mockUseProjects.mockReturnValue([
        {
          id: '1',
          name: 'Test Project',
          color: 'blue',
          path: '/test/project',
          gitBranch: 'main',
          isActive: true
        }
      ])
      mockUseActiveProject.mockReturnValue({
        id: '1',
        name: 'Test Project',
        color: 'blue',
        path: '/test/project',
        gitBranch: 'main',
        isActive: true
      })
      mockUseActiveProjectId.mockReturnValue('1')
      mockUseTerminals.mockReturnValue([])
      mockUseAllTerminals.mockReturnValue([])
      mockUseActiveTerminal.mockReturnValue(null)
      mockUseActiveTerminalId.mockReturnValue('')
    })

    it.skip('should render empty pane hint when project exists but has no tabs', () => {
      renderWithRouter()

      expect(screen.getByText('Drag a tab or file here')).toBeInTheDocument()
    })

    it.skip('should show pane-level new terminal action', () => {
      renderWithRouter()

      expect(screen.getByTitle('Open terminal menu')).toBeInTheDocument()
    })

    it('should not show legacy terminal empty-state CTA', () => {
      renderWithRouter()

      expect(screen.queryByText('No Terminals Yet')).not.toBeInTheDocument()
      expect(screen.queryByText('Create Your First Terminal')).not.toBeInTheDocument()
    })

    it('should not show no projects empty state when project exists', () => {
      renderWithRouter()

      expect(screen.queryByText('No Projects Yet')).not.toBeInTheDocument()
    })
  })

  describe('Zero-project styling', () => {
    it('keeps the Conversation sidebar and workspace pane visible', () => {
      mockUseProjects.mockReturnValue([])
      mockUseActiveProject.mockReturnValue(null)
      mockUseActiveProjectId.mockReturnValue('')

      renderWithRouter()

      expect(screen.getByText('Conversations')).toBeVisible()
      expect(screen.getByTestId('pane-renderer')).toBeVisible()
    })
  })

  describe('Transitions Between States', () => {
    it('keeps project-less New Chat and the workspace pane visible', () => {
      mockUseProjects.mockReturnValue([])
      mockUseActiveProject.mockReturnValue(null)
      mockUseActiveProjectId.mockReturnValue('')

      renderWithRouter()

      expect(screen.getByRole('button', { name: 'New Chat' })).toBeEnabled()
      expect(screen.getByTestId('pane-renderer')).toBeInTheDocument()
    })

    it.skip('should show empty pane hint when project exists but has no tabs', () => {
      mockUseProjects.mockReturnValue([
        {
          id: '1',
          name: 'Test Project',
          color: 'blue',
          path: '/test/project',
          gitBranch: 'main',
          isActive: true
        }
      ])
      mockUseActiveProject.mockReturnValue({
        id: '1',
        name: 'Test Project',
        color: 'blue',
        path: '/test/project',
        gitBranch: 'main',
        isActive: true
      })
      mockUseActiveProjectId.mockReturnValue('1')
      mockUseTerminals.mockReturnValue([])
      mockUseAllTerminals.mockReturnValue([])
      mockUseActiveTerminal.mockReturnValue(null)
      mockUseActiveTerminalId.mockReturnValue('')

      renderWithRouter()

      expect(screen.queryByText('No Projects Yet')).not.toBeInTheDocument()
      expect(screen.getByText('Drag a tab or file here')).toBeInTheDocument()
    })

    it.skip('should not show empty states when terminals exist', () => {
      mockUseProjects.mockReturnValue([
        {
          id: '1',
          name: 'Test Project',
          color: 'blue',
          path: '/test/project',
          gitBranch: 'main',
          isActive: true
        }
      ])
      mockUseActiveProject.mockReturnValue({
        id: '1',
        name: 'Test Project',
        color: 'blue',
        path: '/test/project',
        gitBranch: 'main',
        isActive: true
      })
      mockUseActiveProjectId.mockReturnValue('1')
      mockUseTerminals.mockReturnValue([
        {
          id: 'terminal-1',
          projectId: '1',
          name: 'Terminal 1',
          shell: 'bash',
          cwd: '/test/project'
        }
      ])
      mockUseAllTerminals.mockReturnValue([
        {
          id: 'terminal-1',
          projectId: '1',
          name: 'Terminal 1',
          shell: 'bash',
          cwd: '/test/project'
        }
      ])
      mockUseActiveTerminal.mockReturnValue({
        id: 'terminal-1',
        projectId: '1',
        name: 'Terminal 1',
        shell: 'bash',
        cwd: '/test/project'
      })
      mockUseActiveTerminalId.mockReturnValue('terminal-1')

      renderWithRouter()

      expect(screen.queryByText('No Projects Yet')).not.toBeInTheDocument()
      expect(screen.queryByText('No Terminals Yet')).not.toBeInTheDocument()
    })
  })

  describe('Keyboard panel visibility shortcuts', () => {
    beforeEach(() => {
      const project = createProject('a', '/workspace/a', 'blue')
      mockUseProjects.mockReturnValue([project])
      mockUseActiveProject.mockReturnValue(project)
      mockUseActiveProjectId.mockReturnValue('a')
      mockUseTerminals.mockReturnValue([])
      mockUseAllTerminals.mockReturnValue([])
      mockUseActiveTerminal.mockReturnValue(null)
      mockUseActiveTerminalId.mockReturnValue('')
    })

    it.skip('keeps Ctrl+B toggling file explorer and persists globally', () => {
      renderWithRouter()

      fireEvent.keyDown(window, { key: 'b', ctrlKey: true })

      expect(mockUpdatePanelVisibility).toHaveBeenCalledWith('fileExplorerVisible', false)
    })

    it.skip('toggles sidebar with configured sidebar shortcut and persists globally', () => {
      renderWithRouter()

      fireEvent.keyDown(window, { key: 'B', ctrlKey: true, shiftKey: true })

      expect(mockUpdatePanelVisibility).toHaveBeenCalledWith('sidebarVisible', false)
    })

    it.skip('does not toggle panel shortcuts when focus is in input', () => {
      renderWithRouter()

      const input = document.createElement('input')
      document.body.appendChild(input)
      input.focus()

      fireEvent.keyDown(input, { key: 'b', ctrlKey: true })
      fireEvent.keyDown(input, { key: 'B', ctrlKey: true, shiftKey: true })

      expect(useFileExplorerStore.getState().isVisible).toBe(true)
      expect(useSidebarStore.getState().isVisible).toBe(true)
      expect(mockUpdatePanelVisibility).not.toHaveBeenCalled()

      document.body.removeChild(input)
    })

    it.skip('treats xterm textarea focus as terminal focus for sidebar shortcuts', () => {
      renderWithRouter()

      const terminalRoot = document.createElement('div')
      terminalRoot.className = 'xterm'
      const textarea = document.createElement('textarea')
      terminalRoot.appendChild(textarea)
      document.body.appendChild(terminalRoot)
      textarea.focus()

      fireEvent.keyDown(textarea, { key: 'B', ctrlKey: true, shiftKey: true })

      expect(mockUpdatePanelVisibility).toHaveBeenCalledTimes(1)
      expect(mockUpdatePanelVisibility).toHaveBeenCalledWith('sidebarVisible', false)

      document.body.removeChild(terminalRoot)
    })

    it.skip('does not suppress global non-sidebar shortcuts when focus is in xterm', () => {
      // Verify that a global shortcut whose active element is xterm's textarea
      // fires the appropriate handler rather than being suppressed.
      // Ctrl+N opens the new project modal — it is global and must work from terminal focus.
      renderWithRouter()

      const terminalRoot = document.createElement('div')
      terminalRoot.className = 'xterm'
      const textarea = document.createElement('textarea')
      terminalRoot.appendChild(textarea)
      document.body.appendChild(terminalRoot)
      textarea.focus()

      // File explorer toggle (Ctrl+B) is intentionally blocked in terminal — use
      // sidebar toggle (Ctrl+Shift+B) which IS global from terminal focus.
      // The sidebar was already tested above; here we confirm the panel
      // visibility mock is called exactly once (not zero, as a plain
      // textarea would produce).
      mockUpdatePanelVisibility.mockClear()
      fireEvent.keyDown(textarea, { key: 'B', ctrlKey: true, shiftKey: true })

      expect(mockUpdatePanelVisibility).toHaveBeenCalledTimes(1)
      expect(mockUpdatePanelVisibility).toHaveBeenCalledWith('sidebarVisible', false)

      document.body.removeChild(terminalRoot)
    })

    it.skip('opens the command palette when Ctrl+K is pressed from terminal focus', () => {
      renderWithRouter()

      const terminalRoot = document.createElement('div')
      terminalRoot.className = 'xterm'
      const textarea = document.createElement('textarea')
      terminalRoot.appendChild(textarea)
      document.body.appendChild(terminalRoot)
      textarea.focus()

      fireEvent.keyDown(textarea, { key: 'k', ctrlKey: true })

      expect(screen.getByPlaceholderText('Type a command or search...')).toBeInTheDocument()

      document.body.removeChild(terminalRoot)
    })

    it.skip('opens command history when Ctrl+R is pressed from terminal focus', () => {
      renderWithRouter()

      const terminalRoot = document.createElement('div')
      terminalRoot.className = 'xterm'
      const textarea = document.createElement('textarea')
      terminalRoot.appendChild(textarea)
      document.body.appendChild(terminalRoot)
      textarea.focus()

      fireEvent.keyDown(textarea, { key: 'r', ctrlKey: true })

      expect(screen.getByText('Command History')).toBeInTheDocument()

      document.body.removeChild(terminalRoot)
    })

    it('opens the color theme picker from backend shortcut callbacks', async () => {
      let backendShortcut: ((shortcut: string) => void) | undefined
      // Prefer mockImplementation over Once: Strict Mode remounts / sibling
      // subscribers can consume a one-shot mock before the layout effect runs.
      mockApi.keyboard.onShortcut.mockImplementation((callback: (shortcut: string) => void) => {
        backendShortcut = callback
        return vi.fn()
      })

      renderWithRouter()

      // The onShortcut subscription registers in a layout effect; under
      // full-suite contention this can slip past the default 1s waitFor.
      await waitFor(() => expect(backendShortcut).toBeDefined(), { timeout: 10000 })
      act(() => backendShortcut?.('colorThemePicker'))
      // ThemePicker is React.lazy — allow extra time for the chunk to resolve
      // under full-suite resource contention (passes instantly in isolation).
      expect(
        await screen.findByRole('dialog', { name: 'Color theme picker' }, { timeout: 10000 })
      ).toBeInTheDocument()
    }, 15_000)
  })

  describe('Close flow persistence coordination', () => {
    it.skip('waits for pending app-settings persistence before responding to close with no dirty files', async () => {
      let closeRequestedCallback: (() => Promise<boolean>) | undefined
      mockApi.window.onCloseRequested.mockImplementation((callback: () => Promise<boolean>) => {
        closeRequestedCallback = callback
        return vi.fn()
      })

      const project = createProject('a', '/workspace/a', 'blue')
      mockUseProjects.mockReturnValue([project])
      mockUseActiveProject.mockReturnValue(project)
      mockUseActiveProjectId.mockReturnValue('a')

      const deferred = new Promise<void>((resolve) => {
        mockWaitForPendingAppSettingsPersistence.mockImplementationOnce(async () => {
          await new Promise<void>((r) => setTimeout(r, 0))
          resolve()
        })
      })

      renderWithRouter()
      expect(closeRequestedCallback).toBeDefined()
      if (!closeRequestedCallback) throw new Error('close callback missing')

      await expect(closeRequestedCallback()).resolves.toBe(false)

      expect(mockApi.window.respondToClose).not.toHaveBeenCalled()
      await deferred
      await waitFor(() => {
        expect(mockApi.window.respondToClose).toHaveBeenCalledWith('close')
      })
    })

    it.skip('waits for pending app-settings persistence before confirm-dialog discard close', async () => {
      let closeRequestedCallback: (() => Promise<boolean>) | undefined
      mockApi.window.onCloseRequested.mockImplementation((callback: () => Promise<boolean>) => {
        closeRequestedCallback = callback
        return vi.fn()
      })

      const project = createProject('a', '/workspace/a', 'blue')
      mockUseProjects.mockReturnValue([project])
      mockUseActiveProject.mockReturnValue(project)
      mockUseActiveProjectId.mockReturnValue('a')

      const dirtyEditorState = {
        activeFilePath: '/workspace/a/src/file.ts',
        openFiles: new Map([
          [
            '/workspace/a/src/file.ts',
            {
              filePath: '/workspace/a/src/file.ts',
              isDirty: true
            }
          ]
        ]),
        getDirtyFileCount: vi.fn(() => 1),
        saveAllDirty: vi.fn().mockResolvedValue(undefined),
        closeFile: vi.fn(),
        saveFile: vi.fn().mockResolvedValue(true)
      }

      const useEditorStoreModule = await import('@/stores/editor-store')
      const getStateSpy = vi
        .spyOn(useEditorStoreModule.useEditorStore, 'getState')
        .mockReturnValue(
          dirtyEditorState as unknown as ReturnType<
            typeof useEditorStoreModule.useEditorStore.getState
          >
        )

      renderWithRouter()
      expect(closeRequestedCallback).toBeDefined()
      if (!closeRequestedCallback) throw new Error('close callback missing')

      await expect(closeRequestedCallback()).resolves.toBe(false)

      const dontSaveButton = await screen.findByRole('button', { name: "Don't Save" })

      let resolveWait: (() => void) | undefined
      mockWaitForPendingAppSettingsPersistence.mockImplementationOnce(
        () =>
          new Promise<void>((resolve) => {
            resolveWait = resolve
          })
      )

      await act(async () => {
        fireEvent.click(dontSaveButton)
      })

      expect(mockApi.window.respondToClose).not.toHaveBeenCalled()
      resolveWait?.()
      await waitFor(() => {
        expect(mockApi.window.respondToClose).toHaveBeenCalledWith('close')
      })

      getStateSpy.mockRestore()
    })

    it.skip('still closes when waiting for app-settings persistence rejects', async () => {
      let closeRequestedCallback: (() => Promise<boolean>) | undefined
      mockApi.window.onCloseRequested.mockImplementation((callback: () => Promise<boolean>) => {
        closeRequestedCallback = callback
        return vi.fn()
      })

      const consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => undefined)
      const project = createProject('a', '/workspace/a', 'blue')
      mockUseProjects.mockReturnValue([project])
      mockUseActiveProject.mockReturnValue(project)
      mockUseActiveProjectId.mockReturnValue('a')

      mockWaitForPendingAppSettingsPersistence.mockRejectedValueOnce(
        new Error('settings flush failed')
      )

      renderWithRouter()
      expect(closeRequestedCallback).toBeDefined()
      if (!closeRequestedCallback) throw new Error('close callback missing')

      await expect(closeRequestedCallback()).resolves.toBe(false)

      await waitFor(() => {
        expect(mockApi.window.respondToClose).toHaveBeenCalledWith('close')
      })
      expect(consoleErrorSpy).toHaveBeenCalledWith(
        'Failed to wait for app settings persistence before close:',
        expect.any(Error)
      )

      consoleErrorSpy.mockRestore()
    })
  })

  describe('Project switch watcher orchestration', () => {
    it.skip('watches unrelated roots across project switches and unwatches old root', async () => {
      const projects = [
        createProject('a', '/workspace/a', 'blue'),
        createProject('b', '/outside/b', 'green')
      ]

      mockUseProjects.mockReturnValue(projects)
      mockUseTerminals.mockReturnValue([])
      mockUseAllTerminals.mockReturnValue([])
      mockUseActiveTerminal.mockReturnValue(null)
      mockUseActiveTerminalId.mockReturnValue('')

      mockUseActiveProject.mockReturnValue(projects[0])
      mockUseActiveProjectId.mockReturnValue('a')

      const view = renderWithRouter()

      await waitFor(() => {
        expect(mockApi.filesystem.watchDirectory).toHaveBeenCalledWith('/workspace/a')
      })

      mockUseActiveProject.mockReturnValue(projects[1])
      mockUseActiveProjectId.mockReturnValue('b')
      view.rerender(
        <TooltipProvider>
          <MemoryRouter initialEntries={['/']}>
            <WorkspaceLayout />
          </MemoryRouter>
        </TooltipProvider>
      )

      await waitFor(() => {
        expect(mockApi.filesystem.watchDirectory).toHaveBeenCalledWith('/outside/b')
      })

      expect(mockApi.filesystem.unwatchDirectory).toHaveBeenCalledWith('/workspace/a')
    })

    it.skip('ignores stale async watch completion from older project switch', async () => {
      let resolveFirstWatch: (value: { success: boolean }) => void = () => undefined
      mockApi.filesystem.watchDirectory
        .mockImplementationOnce(
          () =>
            new Promise<{ success: boolean }>((resolve) => {
              resolveFirstWatch = resolve
            })
        )
        .mockResolvedValueOnce({ success: true })

      const projects = [
        createProject('a', '/workspace/a', 'blue'),
        createProject('c', '/workspace/c', 'purple')
      ]

      mockUseProjects.mockReturnValue(projects)
      mockUseTerminals.mockReturnValue([])
      mockUseAllTerminals.mockReturnValue([])
      mockUseActiveTerminal.mockReturnValue(null)
      mockUseActiveTerminalId.mockReturnValue('')

      mockUseActiveProject.mockReturnValue(projects[0])
      mockUseActiveProjectId.mockReturnValue('a')
      const view = renderWithRouter()

      mockUseActiveProject.mockReturnValue(projects[1])
      mockUseActiveProjectId.mockReturnValue('c')
      view.rerender(
        <TooltipProvider>
          <MemoryRouter initialEntries={['/']}>
            <WorkspaceLayout />
          </MemoryRouter>
        </TooltipProvider>
      )

      await waitFor(() => {
        expect(mockApi.filesystem.watchDirectory).toHaveBeenCalledWith('/workspace/c')
      })

      resolveFirstWatch({ success: true })

      await waitFor(() => {
        expect(mockApi.filesystem.unwatchDirectory).toHaveBeenCalledWith('/workspace/a')
      })
    })

    it('surfaces hidden live Conversation terminals as reopenable on desktop', () => {
      const projects = [createProject('a', '/workspace/a', 'blue')]
      const terminal = {
        id: 'terminal-hidden',
        conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
        projectId: 'a',
        name: 'Hidden shell',
        shell: 'bash',
        ptyId: 'pty-hidden',
        claim: 'memory-only',
        viewState: 'hidden',
        healthStatus: 'running'
      } as Terminal
      useConversationStore
        .getState()
        .setActiveConversationId('018f7a1c-1b4d-7c8a-9f01-0123456789ab')
      useSessionWorkspaceSyncStore.setState({
        activeConversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
      })
      mockUseProjects.mockReturnValue(projects)
      mockUseTerminals.mockReturnValue([terminal])
      mockUseAllTerminals.mockReturnValue([terminal])
      mockUseActiveProject.mockReturnValue(projects[0])
      mockUseActiveProjectId.mockReturnValue('a')

      renderWithRouter()

      fireEvent.click(screen.getByRole('button', { name: 'Hidden running terminals' }))
      expect(screen.getByRole('button', { name: 'Reopen Hidden shell' })).toHaveClass('h-9')
      expect(screen.getByRole('button', { name: 'Stop' })).toBeInTheDocument()
    })

    it('surfaces hidden live project terminals so they can be reopened or stopped', () => {
      const projects = [createProject('a', '/workspace/a', 'blue')]
      const terminal = {
        id: 'terminal-project-hidden',
        projectId: 'a',
        name: 'zsh',
        shell: 'zsh',
        ptyId: 'pty-project-hidden',
        viewState: 'hidden',
        healthStatus: 'running'
      } as Terminal
      mockUseProjects.mockReturnValue(projects)
      mockUseTerminals.mockReturnValue([terminal])
      mockUseAllTerminals.mockReturnValue([terminal])
      mockUseActiveProject.mockReturnValue(projects[0])
      mockUseActiveProjectId.mockReturnValue('a')

      renderWithRouter(['/'])

      fireEvent.click(screen.getByRole('button', { name: 'Hidden running terminals' }))
      expect(screen.getByRole('button', { name: 'Reopen zsh' })).toBeInTheDocument()
      fireEvent.click(screen.getByRole('button', { name: 'Stop' }))
      expect(screen.getByText('Terminate terminal process?')).toBeInTheDocument()
    })

    it.skip('does not re-run terminal sync when terminal ids stay unchanged across rerenders', async () => {
      const projects = [createProject('a', '/workspace/a', 'blue')]
      const terminal = {
        id: 'terminal-a',
        projectId: 'a',
        name: 'Terminal A',
        shell: 'bash',
        ptyId: 'pty-a'
      } as Terminal

      mockUseProjects.mockReturnValue(projects)
      mockUseTerminals.mockReturnValue([terminal])
      mockUseAllTerminals.mockReturnValue([terminal])
      mockUseActiveTerminal.mockReturnValue(terminal)
      mockUseActiveTerminalId.mockReturnValue('terminal-a')
      mockUseActiveProject.mockReturnValue(projects[0])
      mockUseActiveProjectId.mockReturnValue('a')

      const view = renderWithRouter()

      await waitFor(() => {
        expect(mockApi.terminal.onData).toHaveBeenCalledTimes(1)
      })

      const consoleLogSpy = vi.spyOn(console, 'log')
      const initialSyncCalls = consoleLogSpy.mock.calls.filter((call) =>
        String(call[0]).includes('[WorkspaceLayout] syncTerminalTabs CALL')
      ).length

      view.rerender(
        <TooltipProvider>
          <MemoryRouter initialEntries={['/']}>
            <WorkspaceLayout />
          </MemoryRouter>
        </TooltipProvider>
      )

      await new Promise((resolve) => setTimeout(resolve, 0))

      const syncCallsAfter = consoleLogSpy.mock.calls.filter((call) =>
        String(call[0]).includes('[WorkspaceLayout] syncTerminalTabs CALL')
      ).length

      expect(syncCallsAfter).toBe(initialSyncCalls)
      consoleLogSpy.mockRestore()
    })

    it('treats watchDirectory WEB_UNSUPPORTED as a soft no-op on web (no rootLoadError)', async () => {
      const prev = tauriRef.current
      tauriRef.current = false
      useFileExplorerStore.setState({ rootLoadError: null })
      mockApi.filesystem.watchDirectory.mockResolvedValue({
        success: false,
        code: 'WEB_UNSUPPORTED',
        error: 'Directory watching is not available in the web client'
      })
      try {
        const projects = [createProject('a', '/workspace/a', 'blue')]
        mockUseProjects.mockReturnValue(projects)
        mockUseTerminals.mockReturnValue([])
        mockUseAllTerminals.mockReturnValue([])
        mockUseActiveTerminal.mockReturnValue(null)
        mockUseActiveTerminalId.mockReturnValue('')
        mockUseActiveProject.mockReturnValue(projects[0])
        mockUseActiveProjectId.mockReturnValue('a')

        renderWithRouter()

        await waitFor(
          () => {
            expect(mockApi.filesystem.watchDirectory).toHaveBeenCalledWith('/workspace/a')
          },
          { timeout: 10000 }
        )

        // Give the async project-switch effect a tick to settle.
        await new Promise((resolve) => setTimeout(resolve, 10))

        // WEB_UNSUPPORTED must NOT set rootLoadError — the project switch
        // completes (file explorer works, just no live change events).
        expect(useFileExplorerStore.getState().rootLoadError).toBeNull()
      } finally {
        tauriRef.current = prev
        mockApi.filesystem.watchDirectory.mockResolvedValue({ success: true })
      }
    }, 15_000)
  })
})

describe('WorkspaceLayout - conversation area navigation', () => {
  afterEach(() => {
    useWorkspaceStore.getState().resetLayout()
    vi.mocked(restoreProjectWorkspace).mockReset()
    vi.mocked(restoreProjectWorkspace).mockResolvedValue(false)
    vi.mocked(persistState).mockReset()
  })

  function setupActiveProject(): void {
    const projects = [createProject('a', '/workspace/a', 'blue')]
    mockUseProjects.mockReturnValue(projects)
    mockUseActiveProject.mockReturnValue(projects[0])
    mockUseActiveProjectId.mockReturnValue('a')
  }

  it('does not restore the project workspace when leaving an open chat for the conversation list', async () => {
    setupActiveProject()
    const navigateRef: MutableRefObject<((to: string) => void) | null> = { current: null }
    vi.mocked(restoreProjectWorkspace).mockClear()

    render(
      <TooltipProvider>
        <MemoryRouter initialEntries={['/c/018f7a1c-1b4d-7c8a-9f01-0123456789ab']}>
          <WorkspaceLayoutWithNavigate navigateRef={navigateRef} />
        </MemoryRouter>
      </TooltipProvider>
    )

    await waitFor(() => {
      expect(navigateRef.current).not.toBeNull()
    })

    act(() => {
      navigateRef.current?.('/conversations')
    })

    await waitFor(() => {
      expect(vi.mocked(restoreProjectWorkspace)).not.toHaveBeenCalled()
    })
  })

  it('restores the project workspace only when leaving the conversation area for projects', async () => {
    setupActiveProject()
    const terminal = {
      id: 'terminal-a',
      projectId: 'a',
      name: 'zsh',
      shell: 'zsh',
      ptyId: 'pty-a',
      viewState: 'open',
      healthStatus: 'running'
    } as Terminal
    mockUseTerminals.mockReturnValue([terminal])
    mockUseAllTerminals.mockReturnValue([terminal])
    useWorkspaceStore.getState().addTerminalTab('terminal-a')
    const navigateRef: MutableRefObject<((to: string) => void) | null> = { current: null }
    vi.mocked(restoreProjectWorkspace).mockReset()
    vi.mocked(restoreProjectWorkspace).mockResolvedValue(true)

    render(
      <TooltipProvider>
        <MemoryRouter initialEntries={['/c/018f7a1c-1b4d-7c8a-9f01-0123456789ab']}>
          <WorkspaceLayoutWithNavigate navigateRef={navigateRef} />
        </MemoryRouter>
      </TooltipProvider>
    )

    await waitFor(() => {
      expect(navigateRef.current).not.toBeNull()
    })

    act(() => {
      navigateRef.current?.('/')
    })

    await waitFor(() => {
      expect(vi.mocked(restoreProjectWorkspace)).toHaveBeenCalledWith('a')
    })
  })

  it('persists the project layout when entering the conversation list from the project workspace', async () => {
    setupActiveProject()
    const navigateRef: MutableRefObject<((to: string) => void) | null> = { current: null }
    vi.mocked(persistState).mockClear()

    render(
      <TooltipProvider>
        <MemoryRouter initialEntries={['/']}>
          <WorkspaceLayoutWithNavigate navigateRef={navigateRef} />
        </MemoryRouter>
      </TooltipProvider>
    )

    await waitFor(() => {
      expect(navigateRef.current).not.toBeNull()
    })

    act(() => {
      navigateRef.current?.('/conversations')
    })

    await waitFor(() => {
      expect(vi.mocked(persistState)).toHaveBeenCalledWith('a')
    })
  })
})

describe('WorkspaceLayout panel fade structure', () => {
  it('wraps the project sidebar and file explorer in opacity-only fades', () => {
    const project = createProject('a', '/workspace/a', 'blue')
    mockUseProjects.mockReturnValue([project])
    mockUseActiveProject.mockReturnValue(project)
    mockUseActiveProjectId.mockReturnValue('a')

    render(
      <TooltipProvider>
        <MemoryRouter initialEntries={['/']}>
          <WorkspaceLayout />
        </MemoryRouter>
      </TooltipProvider>
    )

    const sidebar = screen.getByTestId('sidebar-panel-fade')
    const explorer = screen.getByTestId('file-explorer-panel-fade')
    expect(sidebar).toHaveClass('duration-150', 'overflow-hidden', 'opacity-100')
    expect(explorer).toHaveClass('duration-150', 'overflow-hidden', 'opacity-100')
    expect(sidebar.className).not.toMatch(/scale-/)
    expect(explorer.className).not.toMatch(/scale-/)
    expect(screen.getByTestId('file-explorer')).toBeInTheDocument()
  })

  it('unmounts faded rails after the visibility store turns them off', async () => {
    const project = createProject('a', '/workspace/a', 'blue')
    mockUseProjects.mockReturnValue([project])
    mockUseActiveProject.mockReturnValue(project)
    mockUseActiveProjectId.mockReturnValue('a')

    render(
      <TooltipProvider>
        <MemoryRouter initialEntries={['/']}>
          <WorkspaceLayout />
        </MemoryRouter>
      </TooltipProvider>
    )

    expect(screen.getByTestId('sidebar-panel-fade')).toBeInTheDocument()
    expect(screen.getByTestId('file-explorer-panel-fade')).toBeInTheDocument()

    act(() => {
      useSidebarStore.setState({ isVisible: false })
      useFileExplorerStore.setState({ isVisible: false })
    })

    await waitFor(() => {
      expect(screen.queryByTestId('sidebar-panel-fade')).not.toBeInTheDocument()
      expect(screen.queryByTestId('file-explorer-panel-fade')).not.toBeInTheDocument()
    })
  })
})
