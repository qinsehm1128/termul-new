import type { PluginInfo } from '@shared/types/plugin.types'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { toast } from 'sonner'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { pluginApi } from '@/features/plugins/plugin-api'
import { i18n } from '@/i18n'
import { logFrontendError } from '@/lib/log-api'
import { setRouterNavigate } from '@/lib/router-navigate'
import { usePluginsStore } from '@/stores/plugins-store'
import type { Project } from '@/types/project'
import { CommandPalette } from './CommandPalette'

window.HTMLElement.prototype.scrollIntoView = vi.fn()

const saveRecentCommand = vi.fn(() => Promise.resolve())
let recentCommandIds: string[] = []

const togglePinnedCommand = vi.fn(() => Promise.resolve())
let pinnedCommandIds: string[] = []

vi.mock('@/hooks/use-recent-commands', () => ({
  useRecentCommandIds: () => recentCommandIds,
  useSaveRecentCommand: () => saveRecentCommand
}))

vi.mock('@/hooks/use-pinned-commands', () => ({
  usePinnedCommandIds: () => pinnedCommandIds,
  useTogglePinnedCommand: () => togglePinnedCommand
}))

vi.mock('@/stores/plugins-store', async () => {
  const { create } = await import('zustand')
  return {
    usePluginsStore: create<{ plugins: PluginInfo[]; connected: boolean }>(() => ({
      plugins: [],
      connected: false
    }))
  }
})

vi.mock('@/features/plugins/plugin-api', () => ({
  pluginApi: { executeCommand: vi.fn() }
}))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn(async () => {}) }))
vi.mock('sonner', () => ({ toast: { error: vi.fn() } }))

function activePlugin(overrides: Partial<PluginInfo> = {}): PluginInfo {
  return {
    id: 'com.example.palette',
    manifest: {
      schemaVersion: 1,
      id: 'com.example.palette',
      name: 'Palette Plugin',
      version: '1.0.0',
      abi: { min: 1, max: 1 },
      services: { provides: [], requires: [] },
      contributes: {
        configuration: { title: 'Palette', properties: {} },
        views: [],
        commands: [
          { id: 'palette.hello', title: 'Plugin Hello', description: 'Say hello' },
          { id: 'palette.goodbye', title: 'Plugin Goodbye' }
        ]
      }
    },
    desiredEnabled: true,
    state: 'active',
    generation: 'generation-1',
    configRevision: 1,
    sha256: '0'.repeat(64),
    lastError: null,
    ...overrides
  }
}

const projects: Project[] = [
  {
    id: 'alpha',
    name: 'Alpha',
    color: 'blue',
    path: '/work/alpha'
  },
  {
    id: 'beta',
    name: 'Beta',
    color: 'green',
    path: '/work/beta'
  }
]

function renderPalette(overrides: Partial<React.ComponentProps<typeof CommandPalette>> = {}) {
  const props: React.ComponentProps<typeof CommandPalette> = {
    isOpen: true,
    onClose: vi.fn(),
    projects,
    onSwitchProject: vi.fn(),
    onAddTerminal: vi.fn(),
    onShowAgentLauncher: vi.fn(),
    onSaveSnapshot: vi.fn(),
    onNewBrowserTab: vi.fn(),
    onOpenProjectSettings: vi.fn(),
    onOpenAppPreferences: vi.fn(),
    onOpenCommandHistory: vi.fn(),
    onOpenShortcutMenu: vi.fn(),
    onOpenThemePicker: vi.fn(),
    getShortcutLabel: (id) => {
      const labels: Record<string, string> = {
        newTerminal: 'Ctrl+T',
        newBrowserTab: 'Ctrl+Shift+N',
        commandHistory: 'Ctrl+R',
        colorThemePicker: 'Ctrl+Alt+T'
      }
      return labels[id]
    },
    getProjectShortcutLabel: (index) => `Ctrl+${index + 1}`,
    ...overrides
  }

  return {
    ...render(<CommandPalette {...props} />),
    props
  }
}

describe('CommandPalette', () => {
  beforeEach(() => {
    recentCommandIds = []
    pinnedCommandIds = []
    saveRecentCommand.mockClear()
    togglePinnedCommand.mockClear()
    usePluginsStore.setState({ plugins: [], connected: false })
    vi.mocked(pluginApi.executeCommand).mockReset()
    vi.mocked(pluginApi.executeCommand).mockResolvedValue({
      success: true,
      data: { schemaVersion: 1, result: null, invalidateViews: [] }
    })
    vi.mocked(logFrontendError).mockClear()
    vi.mocked(toast.error).mockClear()
    window.history.replaceState(null, '', '/')
    Reflect.deleteProperty(window, '__TAURI_INTERNALS__')
  })

  it('navigates to Plugins using the plugins namespace title', async () => {
    const navigate = vi.fn()
    setRouterNavigate(navigate)
    const { props } = renderPalette()
    fireEvent.click(screen.getByText(i18n.t('manager.title', { ns: 'plugins' })))
    await waitFor(() => expect(navigate).toHaveBeenCalledWith('/plugins'))
    setRouterNavigate(null)
    expect(props.onClose).toHaveBeenCalledOnce()
    expect(pluginApi.executeCommand).not.toHaveBeenCalled()
  })

  it('shows each active contribution and executes with the current store generation', async () => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', { configurable: true, value: {} })
    usePluginsStore.setState({ plugins: [activePlugin()], connected: true })
    renderPalette()
    expect(screen.getAllByText('Plugin Hello')).toHaveLength(1)
    expect(screen.getAllByText('Plugin Goodbye')).toHaveLength(1)

    // Return a newer authoritative snapshot without rerendering the captured entry.
    const getState = vi.spyOn(usePluginsStore, 'getState').mockReturnValue({
      ...usePluginsStore.getState(),
      plugins: [activePlugin({ generation: 'generation-2' })]
    })
    fireEvent.click(screen.getByText('Plugin Hello'))
    await waitFor(() =>
      expect(pluginApi.executeCommand).toHaveBeenCalledWith({
        id: 'com.example.palette',
        generation: 'generation-2',
        commandId: 'palette.hello',
        args: null
      })
    )
    getState.mockRestore()
    expect(toast.error).not.toHaveBeenCalled()
  })

  it('removes disabled contributions even from pinned and recent commands', () => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', { configurable: true, value: {} })
    pinnedCommandIds = ['plugin:com.example.palette:palette.hello']
    recentCommandIds = ['plugin:com.example.palette:palette.goodbye']
    usePluginsStore.setState({ plugins: [activePlugin()], connected: true })
    renderPalette()
    expect(screen.getAllByText('Plugin Hello').length).toBeGreaterThan(0)
    act(() => usePluginsStore.setState({ plugins: [activePlugin({ state: 'disabled' })] }))
    expect(screen.queryByText('Plugin Hello')).not.toBeInTheDocument()
    expect(screen.queryByText('Plugin Goodbye')).not.toBeInTheDocument()
    expect(pluginApi.executeCommand).not.toHaveBeenCalled()
  })

  it.each([
    'installed',
    'disabled',
    'pending',
    'activating',
    'reloading',
    'failed',
    'quarantined'
  ] as const)('does not expose contributions in the %s state', (state) => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', { configurable: true, value: {} })
    usePluginsStore.setState({ plugins: [activePlugin({ state })], connected: true })
    renderPalette()
    expect(screen.queryByText('Plugin Hello')).not.toBeInTheDocument()
    expect(pluginApi.executeCommand).not.toHaveBeenCalled()
  })

  it('hides contributions on disconnect or missing generation', () => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', { configurable: true, value: {} })
    usePluginsStore.setState({ plugins: [activePlugin()], connected: true })
    renderPalette()
    act(() => usePluginsStore.setState({ connected: false }))
    expect(screen.queryByText('Plugin Hello')).not.toBeInTheDocument()
    act(() =>
      usePluginsStore.setState({ connected: true, plugins: [activePlugin({ generation: null })] })
    )
    expect(screen.queryByText('Plugin Hello')).not.toBeInTheDocument()
  })

  it.each([
    'disabled',
    'disconnected',
    'missing-command',
    'missing-generation'
  ])('rechecks %s in the current store before executing a stale entry', async (condition) => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', { configurable: true, value: {} })
    usePluginsStore.setState({ plugins: [activePlugin()], connected: true })
    renderPalette()
    const plugin = activePlugin({
      state: condition === 'disabled' ? 'disabled' : 'active',
      generation: condition === 'missing-generation' ? null : 'generation-2'
    })
    if (condition === 'missing-command') plugin.manifest.contributes.commands = []
    const getState = vi.spyOn(usePluginsStore, 'getState').mockReturnValue({
      ...usePluginsStore.getState(),
      connected: condition !== 'disconnected',
      plugins: [plugin]
    })
    fireEvent.click(screen.getByText('Plugin Hello'))
    await act(async () => {
      await Promise.resolve()
    })
    expect(getState).toHaveBeenCalled()
    getState.mockRestore()
    expect(pluginApi.executeCommand).not.toHaveBeenCalled()
  })

  it('never exposes or executes plugin commands in a browser', () => {
    usePluginsStore.setState({ plugins: [activePlugin()], connected: true })
    renderPalette()
    expect(screen.queryByText('Plugin Hello')).not.toBeInTheDocument()
    expect(pluginApi.executeCommand).not.toHaveBeenCalled()
  })

  it('rechecks native runtime before executing a captured command', async () => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', { configurable: true, value: {} })
    usePluginsStore.setState({ plugins: [activePlugin()], connected: true })
    renderPalette()
    Reflect.deleteProperty(window, '__TAURI_INTERNALS__')
    fireEvent.click(screen.getByText('Plugin Hello'))
    await act(async () => {
      await Promise.resolve()
    })
    expect(pluginApi.executeCommand).not.toHaveBeenCalled()
  })

  it.each([
    'result',
    'rejection'
  ])('shows generic feedback and durable sanitized logging for an API %s failure', async (failure) => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', { configurable: true, value: {} })
    usePluginsStore.setState({ plugins: [activePlugin()], connected: true })
    if (failure === 'result') {
      vi.mocked(pluginApi.executeCommand).mockResolvedValueOnce({
        success: false,
        code: 'PLUGIN_COMMAND_FAILED',
        error: 'sensitive plugin payload'
      })
    } else {
      vi.mocked(pluginApi.executeCommand).mockRejectedValueOnce(
        new Error('sensitive plugin payload')
      )
    }
    renderPalette()
    fireEvent.click(screen.getByText('Plugin Hello'))
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(i18n.t('errors.generic', { ns: 'plugins' }))
    )
    expect(logFrontendError).toHaveBeenCalledWith({
      source: 'CommandPalette:plugins',
      message: 'Plugin command execution failed'
    })
    expect(JSON.stringify(vi.mocked(logFrontendError).mock.calls)).not.toContain(
      'sensitive plugin payload'
    )
  })

  it('renders a compact command-center layout with metadata, categories, shortcuts, and footer hints', () => {
    renderPalette()

    expect(
      screen.getByPlaceholderText('Search commands, projects, settings...')
    ).toBeInTheDocument()
    expect(screen.getByText('Workspace')).toBeInTheDocument()
    expect(screen.getByText('Navigation')).toBeInTheDocument()
    expect(screen.getByText('Projects')).toBeInTheDocument()
    expect(screen.getByText('Tools')).toBeInTheDocument()
    expect(screen.getByText('Terminal Board')).toBeInTheDocument()
    expect(screen.getByText('See every terminal by group and project')).toBeInTheDocument()
    expect(screen.getByText('Open a new shell in the active pane')).toBeInTheDocument()
    expect(
      screen.getByText('Show the agent launcher prompt in the active pane')
    ).toBeInTheDocument()
    expect(screen.getByText('Ctrl+T')).toBeInTheDocument()
    expect(screen.getByText('Navigate')).toBeInTheDocument()
    expect(screen.getByText('Select')).toBeInTheDocument()
    expect(screen.getByText('Close')).toBeInTheDocument()
  })

  it('orders the Projects group above Workspace, Navigation, and Tools', () => {
    const { container } = renderPalette()

    const headings = Array.from(container.querySelectorAll('[cmdk-group-heading]')).map(
      (el) => el.textContent
    )

    const projectsIndex = headings.indexOf('Projects')
    expect(projectsIndex).toBeGreaterThanOrEqual(0)
    expect(projectsIndex).toBeLessThan(headings.indexOf('Workspace'))
    expect(projectsIndex).toBeLessThan(headings.indexOf('Navigation'))
    expect(projectsIndex).toBeLessThan(headings.indexOf('Tools'))
  })

  it('uses resolved shortcut labels supplied by the shell', () => {
    renderPalette({
      getShortcutLabel: (id) => {
        const labels: Record<string, string> = {
          newTerminal: 'Alt+T',
          newBrowserTab: 'Alt+B',
          commandHistory: 'Alt+H',
          colorThemePicker: 'Alt+Shift+T'
        }
        return labels[id]
      },
      getProjectShortcutLabel: (index) => `Alt+${index + 1}`
    })

    expect(screen.getByText('Alt+T')).toBeInTheDocument()
    expect(screen.getByText('Alt+B')).toBeInTheDocument()
    expect(screen.getByText('Alt+H')).toBeInTheDocument()
    expect(screen.getByText('Alt+Shift+T')).toBeInTheDocument()
    expect(screen.getByText('Alt+1')).toBeInTheDocument()
    expect(screen.queryByText('Ctrl+T')).not.toBeInTheDocument()
    expect(screen.queryByText('Ctrl+Shift+N')).not.toBeInTheDocument()
    expect(screen.queryByText('Ctrl+R')).not.toBeInTheDocument()
  })

  it('searches settings, prefs, and history keywords', async () => {
    renderPalette()
    const input = screen.getByPlaceholderText('Search commands, projects, settings...')

    fireEvent.change(input, { target: { value: 'settings' } })
    await waitFor(() => {
      expect(screen.getByText('Project Settings')).toBeInTheDocument()
      expect(screen.getByText('App Preferences')).toBeInTheDocument()
    })

    fireEvent.change(input, { target: { value: 'prefs' } })
    await waitFor(() => {
      expect(screen.getByText('App Preferences')).toBeInTheDocument()
    })

    fireEvent.change(input, { target: { value: 'history' } })
    await waitFor(() => {
      expect(screen.getByText('Command History')).toBeInTheDocument()
      expect(screen.getByText('Review and reuse recent terminal commands')).toBeInTheDocument()
    })
  })

  it('switches projects, closes, and records recent command id', async () => {
    const { props } = renderPalette()

    fireEvent.click(screen.getByText('Beta'))

    await waitFor(() => {
      expect(saveRecentCommand).toHaveBeenCalledWith('project-beta')
      expect(props.onClose).toHaveBeenCalled()
      expect(props.onSwitchProject).toHaveBeenCalledWith('beta')
    })
  })

  it('renders project rows with a bare name and no "Switch to Project:" prefix', () => {
    renderPalette()

    expect(screen.getByText('Alpha')).toBeInTheDocument()
    expect(screen.getByText('Beta')).toBeInTheDocument()
    expect(screen.queryByText('Switch to Project: Alpha')).not.toBeInTheDocument()
    expect(screen.queryByText('Switch to Project: Beta')).not.toBeInTheDocument()
  })

  it('executes optional callbacks after closing and records selected command ids', async () => {
    const cases: Array<{
      label: string
      commandId: string
      callback: keyof React.ComponentProps<typeof CommandPalette>
    }> = [
      { label: 'New Terminal', commandId: 'new-terminal', callback: 'onAddTerminal' },
      {
        label: 'Agent Launcher',
        commandId: 'show-agent-launcher',
        callback: 'onShowAgentLauncher'
      },
      { label: 'New Browser Tab', commandId: 'new-browser-tab', callback: 'onNewBrowserTab' },
      { label: 'Save Workspace Snapshot', commandId: 'save-snapshot', callback: 'onSaveSnapshot' },
      {
        label: 'Project Settings',
        commandId: 'open-project-settings',
        callback: 'onOpenProjectSettings'
      },
      {
        label: 'App Preferences',
        commandId: 'open-app-preferences',
        callback: 'onOpenAppPreferences'
      },
      {
        label: 'Command History',
        commandId: 'open-command-history',
        callback: 'onOpenCommandHistory'
      },
      {
        label: 'Open Shortcut Menu',
        commandId: 'open-shortcut-menu',
        callback: 'onOpenShortcutMenu'
      },
      {
        label: 'Change Color Theme',
        commandId: 'change-color-theme',
        callback: 'onOpenThemePicker'
      }
    ]

    for (const testCase of cases) {
      saveRecentCommand.mockClear()
      const { props, unmount } = renderPalette()

      fireEvent.click(screen.getByText(testCase.label))

      await waitFor(() => {
        expect(saveRecentCommand).toHaveBeenCalledWith(testCase.commandId)
        expect(props.onClose).toHaveBeenCalled()
        expect(props[testCase.callback]).toHaveBeenCalled()
      })

      unmount()
    }
  })

  it('omits optional commands when callbacks are unavailable', () => {
    renderPalette({
      onAddTerminal: undefined,
      onShowAgentLauncher: undefined,
      onSaveSnapshot: undefined,
      onNewBrowserTab: undefined,
      onOpenProjectSettings: undefined,
      onOpenAppPreferences: undefined,
      onOpenCommandHistory: undefined,
      onOpenShortcutMenu: undefined,
      onOpenThemePicker: undefined
    })

    expect(screen.queryByText('New Terminal')).not.toBeInTheDocument()
    expect(screen.queryByText('Agent Launcher')).not.toBeInTheDocument()
    expect(screen.queryByText('New Browser Tab')).not.toBeInTheDocument()
    expect(screen.queryByText('Save Workspace Snapshot')).not.toBeInTheDocument()
    expect(screen.queryByText('Project Settings')).not.toBeInTheDocument()
    expect(screen.queryByText('App Preferences')).not.toBeInTheDocument()
    expect(screen.queryByText('Command History')).not.toBeInTheDocument()
    expect(screen.queryByText('Open Shortcut Menu')).not.toBeInTheDocument()
    expect(screen.queryByText('Change Color Theme')).not.toBeInTheDocument()
    expect(screen.getByText('Alpha')).toBeInTheDocument()
  })

  it('keeps recent commands visible when the search is empty', () => {
    recentCommandIds = ['open-command-history', 'project-alpha']

    renderPalette()

    expect(screen.getByText('Recent')).toBeInTheDocument()
    expect(screen.getAllByText('Command History')).toHaveLength(2)
    expect(screen.getAllByText('Alpha')).toHaveLength(2)
  })

  it('renders a Pinned group with pinned commands when the search is empty', () => {
    pinnedCommandIds = ['new-terminal', 'project-beta']

    renderPalette()

    expect(screen.getByText('Pinned')).toBeInTheDocument()
    expect(screen.getAllByText('New Terminal')).toHaveLength(2)
    expect(screen.getAllByText('Beta')).toHaveLength(2)
  })

  it('ignores pinned ids that do not resolve to a current command', () => {
    pinnedCommandIds = ['does-not-exist']

    renderPalette()

    expect(screen.queryByText('Pinned')).not.toBeInTheDocument()
  })

  it('toggles a pin without executing the command or closing the palette', async () => {
    const { props } = renderPalette()

    const pinButton = screen.getByLabelText('Pin New Terminal')
    fireEvent.click(pinButton)

    await waitFor(() => {
      expect(togglePinnedCommand).toHaveBeenCalledWith('new-terminal')
    })
    expect(props.onClose).not.toHaveBeenCalled()
    expect(props.onAddTerminal).not.toHaveBeenCalled()
    expect(saveRecentCommand).not.toHaveBeenCalled()
  })

  it('labels the toggle as Unpin for an already-pinned command', () => {
    pinnedCommandIds = ['new-terminal']

    renderPalette()

    expect(screen.getAllByLabelText('Unpin New Terminal').length).toBeGreaterThan(0)
  })

  it('keeps optimistic pin state when persistence fails', async () => {
    const consoleWarn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    togglePinnedCommand.mockRejectedValueOnce(new Error('storage unavailable'))
    const { props } = renderPalette()

    fireEvent.click(screen.getByLabelText('Pin Save Workspace Snapshot'))

    await waitFor(() => {
      expect(consoleWarn).toHaveBeenCalledWith('Failed to toggle pinned command', expect.any(Error))
    })
    expect(props.onClose).not.toHaveBeenCalled()
    expect(props.onSaveSnapshot).not.toHaveBeenCalled()

    consoleWarn.mockRestore()
  })

  it('runs a command when recent-command persistence fails', async () => {
    const consoleWarn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    saveRecentCommand.mockRejectedValueOnce(new Error('storage unavailable'))
    const { props } = renderPalette()

    fireEvent.click(screen.getByText('Save Workspace Snapshot'))

    await waitFor(() => {
      expect(props.onClose).toHaveBeenCalled()
      expect(props.onSaveSnapshot).toHaveBeenCalled()
      expect(consoleWarn).toHaveBeenCalledWith('Failed to save recent command', expect.any(Error))
    })

    consoleWarn.mockRestore()
  })

  it('closes on Escape and backdrop click', () => {
    const { container, props } = renderPalette()

    fireEvent.keyDown(window, { key: 'Escape' })
    expect(props.onClose).toHaveBeenCalledTimes(1)

    const backdrop = container.querySelector('.fixed.inset-0')
    expect(backdrop).not.toBeNull()
    fireEvent.click(backdrop as Element)
    expect(props.onClose).toHaveBeenCalledTimes(2)
  })
})
