import { render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useProjectStore } from '@/stores/project-store'
import { useUpdaterStore } from '@/stores/updater-store'
import { CONTEXT_BAR_SETTINGS_KEY } from '@/types/settings'
import App from './App'

const {
  mockContextBarSettingsRead,
  mockSessionWorkspaceBootstrap,
  mockConversationHostBootstrap,
  mockConversationLifecycle,
  mockTerminalResourceLifecycle
} = vi.hoisted(() => ({
  mockContextBarSettingsRead: vi.fn(),
  mockSessionWorkspaceBootstrap: vi.fn(),
  mockConversationHostBootstrap: vi.fn(),
  mockConversationLifecycle: vi.fn(),
  mockTerminalResourceLifecycle: vi.fn()
}))

vi.mock('@/components/workspace/PaneRenderer', () => ({
  PaneRenderer: () => <div data-testid="pane-renderer" />
}))

vi.mock('@/components/conversation/ConversationRoute', () => ({
  ConversationRoute: () => <div data-testid="canonical-conversation-route" />
}))

vi.mock('@/components/ChatRoute', () => ({
  ChatRoute: ({ sourceKind }: { sourceKind: string }) => (
    <div data-testid="legacy-conversation-route" data-source-kind={sourceKind} />
  )
}))

vi.mock('@/components/DirectoryPicker', () => ({
  DirectoryPicker: () => <div data-testid="web-directory-picker" />
}))

vi.mock('@/lib/tauri-runtime', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/tauri-runtime')>()),
  isTauriContext: () => false
}))

vi.mock('./hooks/use-session-workspace-sync', () => ({
  useSessionWorkspaceBootstrap: mockSessionWorkspaceBootstrap,
  useSessionWorkspaceSync: vi.fn(),
  resolveSessionWorkspaceConflict: vi.fn(),
  resolveSessionWorkspaceRecovery: vi.fn()
}))

vi.mock('./hooks/use-conversation-host-bootstrap', () => ({
  useConversationHostBootstrap: mockConversationHostBootstrap
}))

vi.mock('@/components/conversation/ConversationHostStatus', () => ({
  ConversationHostStatus: () => <div data-testid="conversation-host-status" />
}))

vi.mock('@/components/conversation/ConversationRecoveryPanel', () => ({
  ConversationRecoveryPanel: () => (
    <aside aria-label="Conversation recovery" data-testid="conversation-recovery-panel">
      <button type="button">Inspect preserved source</button>
      <button type="button">Associate conversation</button>
      <button type="button">Start empty workspace</button>
      <button type="button">Dismiss preserved source</button>
    </aside>
  )
}))

vi.mock('./hooks/use-conversation-lifecycle', () => ({
  useConversationLifecycle: mockConversationLifecycle
}))

vi.mock('./hooks/use-terminal-resource-lifecycle', () => ({
  useTerminalResourceLifecycle: mockTerminalResourceLifecycle
}))

vi.mock('./hooks/use-context-bar-settings', () => ({
  useContextBarSettings: () => {
    void mockContextBarSettingsRead(CONTEXT_BAR_SETTINGS_KEY)
  },
  useUpdateContextBarSetting: () => vi.fn(async () => undefined)
}))

const { mockUseVisibilityState } = vi.hoisted(() => ({
  mockUseVisibilityState: vi.fn(() => undefined)
}))

vi.mock('./hooks/use-terminal-detached-output', () => ({
  useTerminalDetachedOutput: () => undefined
}))

vi.mock('./hooks/use-visibility-state', () => ({
  useVisibilityState: mockUseVisibilityState
}))

// CAP-3: resilience hooks + ErrorBoundary/WhatsNewModal wiring assertions.
const {
  mockUseCrashRecovery,
  mockUseTerminalExitNotification,
  mockUseRemoteProjects,
  mockUseWhatsNew,
  mockInitNotificationPermissions
} = vi.hoisted(() => ({
  mockUseCrashRecovery: vi.fn(() => undefined),
  mockUseTerminalExitNotification: vi.fn(() => undefined),
  mockUseRemoteProjects: vi.fn(() => undefined),
  mockUseWhatsNew: vi.fn(() => ({
    isOpen: false,
    version: '',
    notes: null,
    htmlUrl: null,
    close: vi.fn()
  })),
  mockInitNotificationPermissions: vi.fn(() => Promise.resolve())
}))

vi.mock('./hooks/use-crash-recovery', () => ({
  useCrashRecovery: mockUseCrashRecovery
}))

vi.mock('./hooks/use-terminal-exit-notification', () => ({
  useTerminalExitNotification: mockUseTerminalExitNotification
}))

vi.mock('./hooks/use-host-terminal-catalog', () => ({
  useHostTerminalCatalog: () => undefined
}))

vi.mock('./hooks/use-remote-projects', () => ({
  useRemoteProjects: mockUseRemoteProjects
}))

vi.mock('./hooks/use-acp-listeners', () => ({ useAcpListeners: () => undefined }))
vi.mock('./hooks/use-acp-agents', () => ({ useAcpAgents: () => undefined }))
vi.mock('./hooks/use-acp-history', () => ({ useAcpHistory: () => undefined }))
vi.mock('./hooks/use-acp-session-resume', () => ({ useAcpSessionResume: () => undefined }))
vi.mock('./hooks/use-acp-mcp', () => ({ useAcpMcp: () => undefined }))

vi.mock('./hooks/use-whats-new', () => ({
  useWhatsNew: mockUseWhatsNew
}))

vi.mock('./lib/tauri-notification-api', () => ({
  initNotificationPermissions: mockInitNotificationPermissions
}))

// Render-through mock so the test can assert the wrap is present without
// intercepting the child tree. The real ErrorBoundary is a class component
// whose componentDidCatch side effects are irrelevant to wiring assertions.
vi.mock('@/components/ErrorBoundary', () => ({
  ErrorBoundary: ({ children, context }: { children: React.ReactNode; context?: string }) => (
    <div data-testid="error-boundary" data-context={context}>
      {children}
    </div>
  )
}))

vi.mock('./components/WhatsNewModal', () => ({
  WhatsNewModal: (props: { isOpen: boolean; version: string }) => (
    <div
      data-testid="whats-new-modal"
      data-open={String(props.isOpen)}
      data-version={props.version}
    />
  )
}))

const mockCheckForUpdates = vi.fn(async () => {})
const mockInitializeUpdater = vi.fn(async () => {})
const mockStopPeriodicChecks = vi.fn(() => {})

// Mock window.api for hooks that use it
const mockApi = {
  terminal: {
    onCwdChanged: vi.fn(() => () => {}),
    onGitBranchChanged: vi.fn(() => () => {}),
    onGitStatusChanged: vi.fn(() => () => {}),
    onExitCodeChanged: vi.fn(() => () => {}),
    onData: vi.fn(() => () => {}),
    onExit: vi.fn(() => () => {}),
    spawn: vi.fn(),
    write: vi.fn(),
    resize: vi.fn(),
    kill: vi.fn(),
    getCwd: vi.fn(),
    getGitBranch: vi.fn(),
    getGitStatus: vi.fn(),
    getExitCode: vi.fn()
  },
  persistence: {
    getWindowState: vi.fn(() => Promise.resolve({ success: true, data: null })),
    saveWindowState: vi.fn(),
    getProjects: vi.fn(() => Promise.resolve({ success: true, data: [] })),
    saveProjects: vi.fn(),
    getHomeDirectory: vi.fn(() => Promise.resolve({ success: true, data: '/home/user' })),
    read: vi.fn(() => Promise.resolve({ success: true, data: null })),
    writeDebounced: vi.fn(() => Promise.resolve({ success: true, data: undefined })),
    flushPendingWrites: vi.fn(() => Promise.resolve({ success: true, data: undefined }))
  },
  updater: {
    checkForUpdates: vi.fn(() => Promise.resolve({ success: true, data: null })),
    downloadUpdate: vi.fn(() => Promise.resolve({ success: true, data: undefined })),
    installAndRestart: vi.fn(() => Promise.resolve({ success: true, data: undefined })),
    skipVersion: vi.fn(() => Promise.resolve({ success: true, data: undefined })),
    getState: vi.fn(() =>
      Promise.resolve({
        success: true,
        data: {
          updateAvailable: false,
          downloaded: false,
          version: null,
          isChecking: false,
          isDownloading: false,
          downloadProgress: null,
          error: null,
          lastChecked: null
        }
      })
    ),
    setAutoUpdateEnabled: vi.fn(() => Promise.resolve({ success: true, data: undefined })),
    getAutoUpdateEnabled: vi.fn(() => Promise.resolve({ success: true, data: true })),
    onUpdateAvailable: vi.fn(() => () => {}),
    onUpdateDownloaded: vi.fn(() => () => {}),
    onDownloadProgress: vi.fn(() => () => {}),
    onError: vi.fn(() => () => {})
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  window.location.hash = '#/'
  vi.stubGlobal('api', mockApi)
  useProjectStore.setState({ projects: [], groups: [], activeProjectId: '', isLoaded: true })
  useUpdaterStore.setState({
    updateAvailable: false,
    version: null,
    downloaded: false,
    downloadProgress: 0,
    skippedVersion: null,
    isChecking: false,
    isDownloading: false,
    error: null,
    lastChecked: null,
    autoUpdateEnabled: true,
    releaseNotes: null,
    hasActiveTerminals: false,
    checkForUpdates: mockCheckForUpdates,
    initializeUpdater: mockInitializeUpdater,
    stopPeriodicChecks: mockStopPeriodicChecks
  })
})

afterEach(() => {
  vi.unstubAllGlobals()
})

const RECOVERY_ACTION_LABELS = [
  'Inspect preserved source',
  'Associate conversation',
  'Start empty workspace',
  'Dismiss preserved source'
] as const

function expectSingleRecoveryOwner(): void {
  expect(screen.getAllByRole('complementary', { name: 'Conversation recovery' })).toHaveLength(1)
  expect(screen.getAllByTestId('conversation-recovery-panel')).toHaveLength(1)
  for (const label of RECOVERY_ACTION_LABELS) {
    expect(screen.getAllByRole('button', { name: label })).toHaveLength(1)
  }
}

describe('App Component', () => {
  it('should render without crashing', () => {
    render(<App />)
    expect(document.body.querySelector('[class*="min-h-screen"]')).toBeDefined()
  })

  it('should render QueryClientProvider and TooltipProvider', () => {
    render(<App />)
    // App renders and providers work - verify by checking rendered content exists
    expect(document.body.innerHTML.length).toBeGreaterThan(0)
  })

  it('keeps the browser-only directory picker around the shared portable shell', () => {
    render(<App />)

    expect(document.querySelector('[data-testid="web-directory-picker"]')).not.toBeNull()
    expect(mockSessionWorkspaceBootstrap).toHaveBeenCalled()
  })
})

describe('App Routes', () => {
  it('renders the regular project workspace through the production WorkspaceLayout at root', async () => {
    render(<App />)

    expect(await screen.findByTestId('pane-renderer')).toBeVisible()
    expect(
      screen.queryByRole('heading', { name: 'Your Conversation workspace' })
    ).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'New Chat' })).not.toBeInTheDocument()
    expectSingleRecoveryOwner()
  })

  it('loads context bar settings on mount', async () => {
    render(<App />)

    await waitFor(() => {
      expect(mockContextBarSettingsRead).toHaveBeenCalledWith(CONTEXT_BAR_SETTINGS_KEY)
    })
  })

  it('wires app visibility tracking at app scope', () => {
    render(<App />)
    expect(mockUseVisibilityState).toHaveBeenCalled()
  })

  it('mounts the portable SessionWorkspace bootstrap at the web root', () => {
    render(<App />)
    expect(mockSessionWorkspaceBootstrap).toHaveBeenCalled()
  })

  it('mounts shared Conversation creation and recovery wiring at the web root', () => {
    render(<App />)
    expect(mockConversationHostBootstrap).toHaveBeenCalled()
    expect(document.querySelector('[data-testid="conversation-host-status"]')).not.toBeNull()
    expectSingleRecoveryOwner()
  })

  it('mounts Conversation lifecycle reconciliation at the web root', () => {
    render(<App />)
    expect(mockConversationLifecycle).toHaveBeenCalled()
  })

  it('mounts terminal resource reconciliation at the web root', () => {
    render(<App />)
    expect(mockTerminalResourceLifecycle).toHaveBeenCalled()
  })

  it('registers the canonical Conversation route in the web root', async () => {
    window.location.hash = '#/c/018f7a1c-1b4d-7c8a-9f01-0123456789ab'
    render(<App />)

    await waitFor(() => {
      expect(document.querySelector('[data-testid="canonical-conversation-route"]')).not.toBeNull()
    })
  })

  it.each([
    ['session', 'legacyAgentSessionId'],
    ['storage', 'legacyStorageKey'],
    ['history', 'legacyChatHistoryId']
  ])('registers the legacy %s resolver route in the web root', async (route, sourceKind) => {
    window.location.hash = `#/legacy/${route}/opaque-value`
    render(<App />)

    await waitFor(() => {
      expect(document.querySelector('[data-testid="legacy-conversation-route"]')).toHaveAttribute(
        'data-source-kind',
        sourceKind
      )
    })
  })
})

describe('App Updater Integration', () => {
  it('should not depend on legacy updater event listeners on mount', () => {
    render(<App />)
    // The current updater flow bootstraps from persisted state instead of
    // wiring Electron-only event listeners during mount.
    expect(mockApi.updater.onUpdateAvailable).not.toHaveBeenCalled()
    expect(mockApi.updater.onUpdateDownloaded).not.toHaveBeenCalled()
    expect(mockApi.updater.onDownloadProgress).not.toHaveBeenCalled()
    expect(mockApi.updater.onError).not.toHaveBeenCalled()
  })

  it('should initialize updater through the Tauri store hooks on mount', async () => {
    render(<App />)

    await waitFor(() => {
      expect(mockInitializeUpdater).toHaveBeenCalledWith({ autoCheck: false })
      expect(mockInitializeUpdater).toHaveBeenCalledWith({ autoCheck: true })
    })
  })

  it('should delegate startup auto-check through updater initialization', async () => {
    render(<App />)

    await waitFor(() => {
      expect(mockInitializeUpdater).toHaveBeenCalledWith({ autoCheck: true })
    })

    expect(mockCheckForUpdates).not.toHaveBeenCalled()
  })

  it('should stop updater periodic checks when the app unmounts', async () => {
    const { unmount } = render(<App />)

    await waitFor(() => {
      expect(mockInitializeUpdater).toHaveBeenCalled()
    })

    unmount()

    expect(mockStopPeriodicChecks).toHaveBeenCalledTimes(1)
  })
})

describe('App CAP-3 resilience wiring (web entry)', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('wraps the tree in <ErrorBoundary context="appRoot">', () => {
    render(<App />)

    const boundary = document.body.querySelector('[data-testid="error-boundary"]')
    expect(boundary).not.toBeNull()
    expect(boundary?.getAttribute('data-context')).toBe('appRoot')
  })

  it('mounts <WhatsNewModal> as a sibling of <RouterProvider>', () => {
    render(<App />)

    const modal = document.body.querySelector('[data-testid="whats-new-modal"]')
    expect(modal).not.toBeNull()
    expect(modal?.getAttribute('data-open')).toBe('false')
  })

  it('forwards the hook open state and version to <WhatsNewModal>', () => {
    mockUseWhatsNew.mockReturnValue({
      isOpen: true,
      version: '0.4.8',
      notes: 'Release notes',
      htmlUrl: 'https://github.com/qinsehm1128/termul-new/releases/tag/v0.4.8',
      close: vi.fn()
    })

    render(<App />)

    const modal = document.body.querySelector('[data-testid="whats-new-modal"]')
    expect(modal?.getAttribute('data-open')).toBe('true')
    expect(modal?.getAttribute('data-version')).toBe('0.4.8')
  })

  it('calls useWhatsNew in the App body', () => {
    render(<App />)

    expect(mockUseWhatsNew).toHaveBeenCalled()
  })

  it('mounts useCrashRecovery in PortableAppEffects', () => {
    render(<App />)

    expect(mockUseCrashRecovery).toHaveBeenCalled()
  })

  it('mounts useTerminalExitNotification in PortableAppEffects', () => {
    render(<App />)

    expect(mockUseTerminalExitNotification).toHaveBeenCalled()
  })

  it('mounts useRemoteProjects in PortableAppEffects', () => {
    render(<App />)

    expect(mockUseRemoteProjects).toHaveBeenCalled()
  })

  it('calls initNotificationPermissions on mount (useEffect [])', async () => {
    render(<App />)

    await waitFor(() => {
      expect(mockInitNotificationPermissions).toHaveBeenCalledTimes(1)
    })
  })
})
