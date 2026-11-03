import { cleanup, render } from '@testing-library/react'
import { StrictMode } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useProjectStore } from '@/stores/project-store'
import { PortableAppEffects } from './PortableAppEffects'

const { subscribe, dispose } = vi.hoisted(() => ({
  subscribe: vi.fn<() => () => void>(),
  dispose: vi.fn()
}))

vi.mock('@/stores/plugins-store', () => ({
  usePluginsStore: { getState: () => ({ subscribe }) }
}))

vi.mock('@/components/UpdateAvailableToast', () => ({ useUpdateToast: vi.fn() }))
vi.mock('@/hooks/use-acp-agents', () => ({ useAcpAgents: vi.fn() }))
vi.mock('@/hooks/use-acp-history', () => ({ useAcpHistory: vi.fn() }))
vi.mock('@/hooks/use-acp-listeners', () => ({ useAcpListeners: vi.fn() }))
vi.mock('@/hooks/use-acp-mcp', () => ({ useAcpMcp: vi.fn() }))
vi.mock('@/hooks/use-acp-session-resume', () => ({ useAcpSessionResume: vi.fn() }))
vi.mock('@/hooks/use-agent-session-tracker', () => ({ useAgentSessionTracker: vi.fn() }))
vi.mock('@/hooks/use-agent-terminal-state', () => ({ useAgentTerminalState: vi.fn() }))
vi.mock('@/hooks/use-app-settings', () => ({ useAppSettingsLoader: vi.fn() }))
vi.mock('@/hooks/use-color-theme', () => ({ useAppliedColorThemeSync: vi.fn() }))
vi.mock('@/hooks/use-context-bar-settings', () => ({ useContextBarSettings: vi.fn() }))
vi.mock('@/hooks/use-conversation-host-bootstrap', () => ({
  useConversationHostBootstrap: vi.fn()
}))
vi.mock('@/hooks/use-conversation-lifecycle', () => ({ useConversationLifecycle: vi.fn() }))
vi.mock('@/hooks/use-crash-recovery', () => ({ useCrashRecovery: vi.fn() }))
vi.mock('@/hooks/use-cwd', () => ({ useCwd: vi.fn() }))
vi.mock('@/hooks/use-exit-code', () => ({ useExitCode: vi.fn() }))
vi.mock('@/hooks/use-git-branch', () => ({ useGitBranch: vi.fn() }))
vi.mock('@/hooks/use-git-status', () => ({ useGitStatus: vi.fn() }))
vi.mock('@/hooks/use-host-terminal-catalog', () => ({ useHostTerminalCatalog: vi.fn() }))
vi.mock('@/hooks/use-keyboard-shortcuts', () => ({ useKeyboardShortcutsLoader: vi.fn() }))
vi.mock('@/hooks/use-language', () => ({ useAppliedLanguageSync: vi.fn() }))
vi.mock('@/hooks/use-menu-select-all', () => ({ useMenuSelectAll: vi.fn() }))
vi.mock('@/hooks/use-menu-updater-listener', () => ({ useMenuUpdaterListener: vi.fn() }))
vi.mock('@/hooks/use-prevent-file-drop-navigation', () => ({
  usePreventFileDropNavigation: vi.fn()
}))
vi.mock('@/hooks/use-prevent-native-context-menu', () => ({
  usePreventNativeContextMenu: vi.fn()
}))
vi.mock('@/hooks/use-projects-persistence', () => ({
  useProjectsAutoSave: vi.fn(),
  useProjectsLoader: vi.fn()
}))
vi.mock('@/hooks/use-remote-projects', () => ({ useRemoteProjects: vi.fn() }))
vi.mock('@/hooks/use-session-workspace-sync', () => ({ useSessionWorkspaceBootstrap: vi.fn() }))
vi.mock('@/hooks/use-sidebar-font-size', () => ({ useAppliedSidebarFontSizeSync: vi.fn() }))
vi.mock('@/hooks/use-terminal-detached-output', () => ({ useTerminalDetachedOutput: vi.fn() }))
vi.mock('@/hooks/use-terminal-exit-notification', () => ({ useTerminalExitNotification: vi.fn() }))
vi.mock('@/hooks/use-terminal-recovery', () => ({ useAdoptLiveTerminalsOnBoot: vi.fn() }))
vi.mock('@/hooks/use-terminal-resource-lifecycle', () => ({
  useTerminalResourceLifecycle: vi.fn()
}))
vi.mock('@/hooks/use-terminal-restore', () => ({ useTerminalRestore: vi.fn() }))
vi.mock('@/hooks/use-ui-zoom', () => ({ useAppliedUiZoomSync: vi.fn() }))
vi.mock('@/hooks/use-updater', () => ({ useUpdateCheck: vi.fn() }))
vi.mock('@/hooks/use-visibility-state', () => ({ useVisibilityState: vi.fn() }))
vi.mock('@/hooks/useTerminalAutoSave', () => ({ useTerminalAutoSave: vi.fn() }))
vi.mock('@/lib/tauri-notification-api', () => ({ initNotificationPermissions: vi.fn() }))

beforeEach(() => {
  vi.clearAllMocks()
  subscribe.mockReturnValue(dispose)
  useProjectStore.setState({
    projects: [],
    groups: [],
    activeGroupId: null,
    activeProjectId: '',
    isLoaded: true
  })
})

afterEach(cleanup)

describe('PortableAppEffects plugin subscription lifetime', () => {
  it('subscribes once, does not resubscribe on rerender, and disposes on unmount', () => {
    const view = render(<PortableAppEffects />)
    expect(subscribe).toHaveBeenCalledExactlyOnceWith()
    expect(dispose).not.toHaveBeenCalled()

    view.rerender(<PortableAppEffects />)
    expect(subscribe).toHaveBeenCalledTimes(1)
    expect(dispose).not.toHaveBeenCalled()

    view.unmount()
    expect(dispose).toHaveBeenCalledExactlyOnceWith()
  })

  it('creates and cleans up a fresh subscription after remount', () => {
    const firstDispose = vi.fn()
    const secondDispose = vi.fn()
    subscribe.mockReturnValueOnce(firstDispose).mockReturnValueOnce(secondDispose)

    const first = render(<PortableAppEffects />)
    first.unmount()
    expect(firstDispose).toHaveBeenCalledTimes(1)

    const second = render(<PortableAppEffects />)
    expect(subscribe).toHaveBeenCalledTimes(2)
    expect(secondDispose).not.toHaveBeenCalled()
    second.unmount()
    expect(firstDispose).toHaveBeenCalledTimes(1)
    expect(secondDispose).toHaveBeenCalledTimes(1)
  })

  it('balances setup and disposal during StrictMode effect replay', () => {
    const view = render(
      <StrictMode>
        <PortableAppEffects />
      </StrictMode>
    )
    expect(subscribe).toHaveBeenCalledTimes(2)
    expect(dispose).toHaveBeenCalledTimes(1)

    view.unmount()
    expect(dispose).toHaveBeenCalledTimes(2)
  })
})
