import { render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useProjectStore } from '@/stores/project-store'
import { CONTEXT_BAR_SETTINGS_KEY } from '@/types/settings'
import TauriApp from './TauriApp'

const {
  mockPersistenceRead,
  mockSessionWorkspaceBootstrap,
  mockConversationHostBootstrap,
  mockConversationLifecycle,
  mockTerminalResourceLifecycle,
  mockPreventDevToolsShortcuts,
  mockIsWindowStateReady,
  mockShowWindow
} = vi.hoisted(() => ({
  mockPersistenceRead: vi.fn(),
  mockSessionWorkspaceBootstrap: vi.fn(),
  mockConversationHostBootstrap: vi.fn(),
  mockConversationLifecycle: vi.fn(),
  mockTerminalResourceLifecycle: vi.fn(),
  mockPreventDevToolsShortcuts: vi.fn(),
  mockIsWindowStateReady: vi.fn(() => false),
  mockShowWindow: vi.fn(() => Promise.resolve())
}))

vi.mock('@/lib/api', () => ({
  filesystemApi: {
    watchDirectory: vi.fn(async () => ({ success: true, data: undefined })),
    unwatchDirectory: vi.fn(),
    onFileChanged: vi.fn(() => vi.fn()),
    onFileCreated: vi.fn(() => vi.fn()),
    onFileDeleted: vi.fn(() => vi.fn()),
    onSearchFileNamesBatch: vi.fn(() => vi.fn()),
    onSearchFileNamesDone: vi.fn(() => vi.fn())
  },
  keyboardApi: { onShortcut: vi.fn(() => vi.fn()) },
  persistenceApi: {
    read: mockPersistenceRead,
    write: vi.fn(async () => ({ success: true, data: undefined })),
    flushPendingWrites: vi.fn(async () => ({ success: true, data: undefined }))
  },
  sshApi: { onConnectionStatusChanged: vi.fn(() => vi.fn()) },
  terminalApi: {
    onData: vi.fn(() => vi.fn()),
    write: vi.fn(async () => ({ success: true, data: undefined }))
  },
  windowApi: {
    onCloseRequested: vi.fn(() => vi.fn()),
    respondToClose: vi.fn()
  },
  sessionApi: {
    hasSession: vi.fn(async () => ({ success: true, data: false })),
    restore: vi.fn(async () => ({
      success: false,
      error: 'No session',
      code: 'SESSION_NOT_FOUND'
    })),
    save: vi.fn(),
    clear: vi.fn(),
    flush: vi.fn()
  }
}))

vi.mock('@/hooks/use-session-workspace-sync', () => ({
  useSessionWorkspaceBootstrap: mockSessionWorkspaceBootstrap,
  useSessionWorkspaceSync: vi.fn(),
  loadSessionWorkspace: vi.fn(async () => false),
  resolveSessionWorkspaceConflict: vi.fn(async () => undefined),
  resolveSessionWorkspaceRecovery: vi.fn(async () => undefined),
  performSessionWorkspaceWrite: vi.fn(async () => 'skipped')
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

vi.mock('@/hooks/use-prevent-devtools-shortcuts', () => ({
  usePreventDevToolsShortcuts: mockPreventDevToolsShortcuts
}))

vi.mock('@/hooks/use-window-state', () => ({
  useWindowState: mockIsWindowStateReady
}))

vi.mock('@/lib/tauri-window', () => ({
  getCurrentWindow: () => ({ show: mockShowWindow })
}))

vi.mock('@/components/ActivityRail', () => ({ ActivityRail: () => null }))

vi.mock('@/components/conversation/ConversationSidebar', () => ({
  ConversationSidebar: ({ onNewChat }: { onNewChat: () => void }) => (
    <button type="button" aria-label="New Chat" onClick={onNewChat}>
      New Chat
    </button>
  )
}))

vi.mock('@/components/ProjectSidebar', () => ({ ProjectSidebar: () => null }))
vi.mock('@/components/ResizeEdges', () => ({ ResizeEdges: () => null }))
vi.mock('@/components/StatusBar', () => ({ StatusBar: () => null }))
vi.mock('@/components/TitleBar', () => ({ TitleBar: () => null }))

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

vi.mock('./pages/ProjectSettings', () => ({
  default: () => null
}))

vi.mock('./pages/AppPreferences', () => ({
  default: () => null
}))

vi.mock('./pages/WorkspaceSnapshots', () => ({
  default: () => null
}))

vi.mock('./pages/NotFound', () => ({
  default: () => null
}))

vi.mock('./hooks/useTerminalAutoSave', () => ({
  useTerminalAutoSave: () => undefined
}))

vi.mock('./hooks/use-terminal-restore', () => ({
  useTerminalRestore: () => undefined
}))

vi.mock('./hooks/use-terminal-detached-output', () => ({
  useTerminalDetachedOutput: () => undefined
}))

vi.mock('./hooks/use-cwd', () => ({
  useCwd: () => undefined,
  useHomeDirectory: () => '/home/user'
}))

vi.mock('./hooks/use-git-branch', () => ({
  useGitBranch: () => undefined
}))

vi.mock('./hooks/use-git-status', () => ({
  useGitStatus: () => undefined
}))

vi.mock('./hooks/use-exit-code', () => ({
  useExitCode: () => undefined
}))

vi.mock('./hooks/use-app-settings', () => ({
  useAppSettingsLoader: () => undefined,
  useUpdateAppSetting: () => vi.fn(),
  useUpdatePanelVisibility: () => vi.fn(async () => undefined),
  waitForPendingAppSettingsPersistence: vi.fn(async () => undefined)
}))

vi.mock('./hooks/use-keyboard-shortcuts', () => ({
  useKeyboardShortcutsLoader: () => undefined,
  useUpdateShortcut: () => vi.fn(),
  useResetShortcut: () => vi.fn()
}))

vi.mock('./hooks/use-projects-persistence', () => ({
  useProjectsLoader: () => undefined,
  useProjectsAutoSave: () => undefined
}))

vi.mock('./hooks/use-menu-updater-listener', () => ({
  useMenuUpdaterListener: () => undefined
}))

vi.mock('./hooks/use-updater', () => ({
  useUpdateCheck: () => undefined
}))

vi.mock('./components/UpdateAvailableToast', () => ({
  useUpdateToast: () => undefined
}))

const { mockUseVisibilityState } = vi.hoisted(() => ({
  mockUseVisibilityState: vi.fn(() => undefined)
}))

vi.mock('./hooks/use-visibility-state', () => ({
  useVisibilityState: mockUseVisibilityState
}))

vi.mock('./hooks/use-terminal-exit-notification', () => ({
  useTerminalExitNotification: () => undefined
}))

vi.mock('./hooks/use-host-terminal-catalog', () => ({
  useHostTerminalCatalog: () => undefined
}))

vi.mock('./hooks/use-remote-projects', () => ({
  useRemoteProjects: () => undefined
}))

vi.mock('./hooks/use-acp-listeners', () => ({ useAcpListeners: () => undefined }))
vi.mock('./hooks/use-acp-agents', () => ({ useAcpAgents: () => undefined }))
vi.mock('./hooks/use-acp-history', () => ({ useAcpHistory: () => undefined }))
vi.mock('./hooks/use-acp-session-resume', () => ({ useAcpSessionResume: () => undefined }))
vi.mock('./hooks/use-acp-mcp', () => ({ useAcpMcp: () => undefined }))

vi.mock('@/lib/tauri-notification-api', () => ({
  initNotificationPermissions: () => Promise.resolve(),
  sendDesktopNotification: () => Promise.resolve()
}))

beforeEach(() => {
  vi.clearAllMocks()
  mockIsWindowStateReady.mockReturnValue(false)
  window.location.hash = '#/'
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__
  mockPersistenceRead.mockResolvedValue({
    success: false,
    error: 'Key not found',
    code: 'KEY_NOT_FOUND'
  })
  useProjectStore.setState({ projects: [], groups: [], activeProjectId: '', isLoaded: true })
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

describe('TauriApp', () => {
  it('loads context bar settings on mount', async () => {
    render(<TauriApp />)

    await waitFor(() => {
      expect(mockPersistenceRead).toHaveBeenCalledWith(CONTEXT_BAR_SETTINGS_KEY)
    })
  })

  it('wires app visibility tracking at app scope', () => {
    render(<TauriApp />)
    expect(mockUseVisibilityState).toHaveBeenCalled()
  })

  it('keeps native-only devtools protection around the shared portable shell', () => {
    render(<TauriApp />)

    expect(mockPreventDevToolsShortcuts).toHaveBeenCalled()
    expect(mockSessionWorkspaceBootstrap).toHaveBeenCalled()
  })

  it('renders the regular native project workspace through the production WorkspaceLayout at root', async () => {
    render(<TauriApp />)

    expect(await screen.findByTestId('pane-renderer')).toBeVisible()
    expect(
      screen.queryByRole('heading', { name: 'Your Conversation workspace' })
    ).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'New Chat' })).not.toBeInTheDocument()
    expectSingleRecoveryOwner()
  })

  it('shows the native window after window state restoration', async () => {
    mockIsWindowStateReady.mockReturnValue(true)
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {}

    render(<TauriApp />)

    await waitFor(() => {
      expect(mockShowWindow).toHaveBeenCalledTimes(1)
    })
  })

  it('mounts the portable SessionWorkspace bootstrap at the desktop root', () => {
    render(<TauriApp />)
    expect(mockSessionWorkspaceBootstrap).toHaveBeenCalled()
  })

  it('mounts shared Conversation creation and recovery wiring at the desktop root', () => {
    render(<TauriApp />)
    expect(mockConversationHostBootstrap).toHaveBeenCalled()
    expect(document.querySelector('[data-testid="conversation-host-status"]')).not.toBeNull()
    expectSingleRecoveryOwner()
  })

  it('mounts Conversation lifecycle reconciliation at the desktop root', () => {
    render(<TauriApp />)
    expect(mockConversationLifecycle).toHaveBeenCalled()
  })

  it('mounts terminal resource reconciliation at the desktop root', () => {
    render(<TauriApp />)
    expect(mockTerminalResourceLifecycle).toHaveBeenCalled()
  })

  it('registers the canonical Conversation route in the desktop root', async () => {
    window.location.hash = '#/c/018f7a1c-1b4d-7c8a-9f01-0123456789ab'
    render(<TauriApp />)

    await waitFor(() => {
      expect(document.querySelector('[data-testid="canonical-conversation-route"]')).not.toBeNull()
    })
  })

  it.each([
    ['session', 'legacyAgentSessionId'],
    ['storage', 'legacyStorageKey'],
    ['history', 'legacyChatHistoryId']
  ])('registers the legacy %s resolver route in the desktop root', async (route, sourceKind) => {
    window.location.hash = `#/legacy/${route}/opaque-value`
    render(<TauriApp />)

    await waitFor(() => {
      expect(document.querySelector('[data-testid="legacy-conversation-route"]')).toHaveAttribute(
        'data-source-kind',
        sourceKind
      )
    })
  })
})
