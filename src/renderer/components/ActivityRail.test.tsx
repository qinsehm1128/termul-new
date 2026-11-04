import type { PluginInfo, PluginView } from '@shared/types/plugin.types'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import * as appSettingsHooks from '@/hooks/use-app-settings'
import { resetNavigationStore, useNavigationStore } from '@/stores/navigation-store'
import { usePluginsStore } from '@/stores/plugins-store'
import { useSSHPanelStore } from '@/stores/ssh-panel-store'
import { useSSHStore } from '@/stores/ssh-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { ActivityRail } from './ActivityRail'

const { mockUpdatePanelVisibility, mockToastError, mockNavigate, platformState } = vi.hoisted(
  () => ({
    mockUpdatePanelVisibility: vi.fn(() => Promise.resolve()),
    mockToastError: vi.fn(),
    mockNavigate: vi.fn(),
    platformState: { isMac: false }
  })
)

vi.mock('sonner', () => ({
  toast: {
    error: mockToastError
  }
}))

vi.mock('@/lib/platform', () => ({
  get isMac() {
    return platformState.isMac
  }
}))

// Mutable: defaults to desktop so existing tests pass. Web-mode tests set
// this to false to verify the SSH rail button's disabled-with-reason gate.
const { tauriRef } = vi.hoisted(() => ({ tauriRef: { current: true as boolean } }))

vi.mock('@/lib/tauri-runtime', () => ({
  isTauriContext: () => tauriRef.current
}))

vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom')
  return {
    ...actual,
    useNavigate: () => mockNavigate
  }
})

function sidebarPlugin(overrides: Partial<PluginInfo> = {}): PluginInfo {
  return {
    id: 'example.plugin',
    manifest: {
      schemaVersion: 1,
      id: 'example.plugin',
      name: 'Example Plugin',
      version: '1.0.0',
      abi: { min: 1, max: 1 },
      contributes: {
        commands: [],
        configuration: { title: 'Example Plugin', properties: {} },
        views: [{ id: 'dashboard', title: 'Plugin dashboard', location: 'sidebar' }]
      },
      services: { provides: [], requires: [] }
    },
    state: 'active',
    desiredEnabled: true,
    generation: '1',
    configRevision: 0,
    sha256: 'test',
    lastError: null,
    ...overrides
  }
}

describe('ActivityRail', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    platformState.isMac = false
    vi.spyOn(appSettingsHooks, 'useUpdatePanelVisibility').mockReturnValue(
      mockUpdatePanelVisibility
    )
    resetNavigationStore()
    usePluginsStore.setState({ plugins: [], connected: false })
    useSSHPanelStore.setState({ isVisible: true })
    useSSHStore.setState({ activeProfileId: null })
    useTerminalStore.setState({ terminals: [], activeTerminalId: '', ptyIdIndex: new Map() })
  })

  function renderRail(path = '/') {
    return render(
      <TooltipProvider delayDuration={0}>
        <MemoryRouter initialEntries={[path]}>
          <ActivityRail />
        </MemoryRouter>
      </TooltipProvider>
    )
  }

  describe('plugin sidebar views', () => {
    it('renders a separate runtime group with accessible titles, plugin tooltips and fallback icons', async () => {
      const plugin = sidebarPlugin()
      plugin.manifest.contributes.views.push({
        id: 'panel',
        title: 'Panel only',
        location: 'panel'
      })
      usePluginsStore.setState({ plugins: [plugin], connected: true })
      const entriesBefore = useNavigationStore.getState().layout.entries
      renderRail()

      const group = screen.getByRole('group', { name: 'Plugin views' })
      const button = within(group).getByRole('button', { name: 'Plugin dashboard' })
      expect(screen.queryByRole('button', { name: 'Panel only' })).not.toBeInTheDocument()
      expect(button.querySelector('svg')).toHaveClass('lucide-puzzle')
      expect(group.querySelector('[data-rail-row]')).toBeNull()
      expect(button).not.toHaveAttribute('data-rail-item-id')
      fireEvent.pointerMove(button, { pointerType: 'mouse' })
      fireEvent.pointerEnter(button, { pointerType: 'mouse' })
      fireEvent.mouseOver(button)
      expect(await screen.findByRole('tooltip')).toHaveTextContent(
        'Plugin dashboard (Example Plugin)'
      )
      expect(screen.getByRole('tooltip')).not.toHaveTextContent('reorder')
      expect(useNavigationStore.getState().layout.entries).toEqual(entriesBefore)
    })

    it.each<Partial<PluginInfo>>([
      { desiredEnabled: false },
      { state: 'disabled' },
      { state: 'installed' },
      { state: 'quarantined' },
      { state: 'failed' }
    ])('hides ineligible plugins: %j', (overrides) => {
      usePluginsStore.setState({ plugins: [sidebarPlugin(overrides)], connected: true })
      renderRail()

      expect(screen.queryByRole('button', { name: 'Plugin dashboard' })).not.toBeInTheDocument()
      expect(screen.queryByRole('group', { name: 'Plugin views' })).not.toBeInTheDocument()
    })

    it('removes the runtime group when the plugin is uninstalled', () => {
      usePluginsStore.setState({ plugins: [sidebarPlugin()], connected: true })
      renderRail()
      expect(screen.getByRole('button', { name: 'Plugin dashboard' })).toBeVisible()

      act(() => usePluginsStore.setState({ plugins: [] }))
      expect(screen.queryByRole('group', { name: 'Plugin views' })).not.toBeInTheDocument()
    })

    it('hides stale active plugins when disconnected and reacts to disconnects', () => {
      usePluginsStore.setState({ plugins: [sidebarPlugin()], connected: false })
      renderRail()
      expect(screen.queryByRole('button', { name: 'Plugin dashboard' })).not.toBeInTheDocument()

      act(() => usePluginsStore.setState({ connected: true }))
      expect(screen.getByRole('button', { name: 'Plugin dashboard' })).toBeVisible()
      act(() => usePluginsStore.setState({ connected: false }))
      expect(screen.queryByRole('group', { name: 'Plugin views' })).not.toBeInTheDocument()
    })

    it('navigates with encoded plugin and view IDs without changing navigation settings', () => {
      const plugin = sidebarPlugin({ id: 'example/plugin ?' })
      plugin.manifest.contributes.views[0].id = 'dashboard/with space?'
      usePluginsStore.setState({ plugins: [plugin], connected: true })
      useSSHStore.setState({ activeProfileId: 'ssh-profile' })
      const layoutBefore = useNavigationStore.getState().layout
      renderRail()

      fireEvent.click(screen.getByRole('button', { name: 'Plugin dashboard' }))

      expect(mockNavigate).toHaveBeenCalledWith(
        '/plugins/example%2Fplugin%20%3F/views/dashboard%2Fwith%20space%3F'
      )
      expect(useSSHStore.getState().activeProfileId).toBeNull()
      expect(useNavigationStore.getState().layout).toEqual(layoutBefore)
    })

    it('styles the current plugin route like other active rail icons', () => {
      const plugin = sidebarPlugin()
      plugin.manifest.contributes.views.push({
        id: 'other',
        title: 'Other plugin view',
        location: 'sidebar',
        icon: 'terminal'
      })
      usePluginsStore.setState({ plugins: [plugin], connected: true })
      renderRail('/plugins/example.plugin/views/dashboard')

      const active = screen.getByRole('button', { name: 'Plugin dashboard' })
      const inactive = screen.getByRole('button', { name: 'Other plugin view' })
      expect(active).toHaveAttribute('aria-current', 'page')
      expect(active.querySelector('svg')).toHaveClass('text-foreground')
      expect(inactive).not.toHaveAttribute('aria-current')
      expect(inactive.querySelector('svg')).toHaveClass('text-muted-foreground')
      expect(inactive.querySelector('svg')).toHaveClass('lucide-terminal')
    })

    it('sorts plugins by name while preserving contributed view order and source order', () => {
      const makePlugin = (id: string, name: string, views: PluginView[]): PluginInfo => {
        const plugin = sidebarPlugin({ id })
        plugin.manifest.name = name
        plugin.manifest.contributes.views = views
        return plugin
      }
      const plugins = [
        makePlugin('z', 'Zebra', [{ id: 'z1', title: 'Zebra view', location: 'sidebar' }]),
        makePlugin('a', 'Alpha', [
          { id: 'a2', title: 'Alpha second ID first', location: 'sidebar' },
          { id: 'a1', title: 'Alpha first ID second', location: 'sidebar' }
        ])
      ]
      usePluginsStore.setState({ plugins, connected: true })
      renderRail()

      const group = screen.getByRole('group', { name: 'Plugin views' })
      expect(
        within(group)
          .getAllByRole('button')
          .map((button) => button.getAttribute('aria-label'))
      ).toEqual(['Alpha second ID first', 'Alpha first ID second', 'Zebra view'])
      expect(usePluginsStore.getState().plugins.map((plugin) => plugin.id)).toEqual(['z', 'a'])
    })
  })

  describe('collapsible divider groups', () => {
    it('opens the permanent Plugins manager from the tools group and marks it active', () => {
      renderRail('/plugins')
      const tools = screen.getByRole('group', { name: 'Tools' })
      const plugins = within(tools).getByRole('button', { name: 'Open Plugins' })
      expect(plugins).toHaveAttribute('aria-current', 'page')
      expect(plugins.querySelector('svg')).toHaveClass('lucide-puzzle')
      fireEvent.click(plugins)
      expect(mockNavigate).toHaveBeenCalledWith('/plugins')
      const items = useNavigationStore.getState().layout.entries
      expect(items.findIndex((entry) => entry.id === 'plugins')).toBe(
        items.findIndex((entry) => entry.id === 'mcp') + 1
      )
    })

    it('collapses a built-in group with a full-width accessible button and restores via keyboard', () => {
      renderRail()
      const divider = screen.getByRole('button', { name: 'tools group' })
      expect(divider).toHaveAttribute('aria-expanded', 'true')
      fireEvent.click(divider)
      expect(divider).toHaveAttribute('aria-expanded', 'false')
      expect(screen.queryByRole('button', { name: 'Open MCP' })).not.toBeInTheDocument()
      expect(useNavigationStore.getState().layout.entries).toContainEqual({
        kind: 'divider',
        id: 'tools',
        collapsed: true
      })
      act(() => divider.focus())
      fireEvent.keyDown(divider, { key: 'Enter' })
      expect(screen.getByRole('button', { name: 'Open MCP' })).toBeVisible()
      fireEvent.keyDown(divider, { key: ' ' })
      expect(screen.queryByRole('button', { name: 'Open MCP' })).not.toBeInTheDocument()
    })

    it('keeps the current route visible while hiding inactive items in its collapsed group', () => {
      renderRail('/plugins')
      fireEvent.click(screen.getByRole('button', { name: 'tools group' }))
      expect(screen.getByRole('button', { name: 'Open Plugins' })).toBeVisible()
      expect(screen.queryByRole('button', { name: 'Open MCP' })).not.toBeInTheDocument()
    })

    it('toggles a custom group and exposes expand/collapse in its context menu', async () => {
      act(() => useNavigationStore.getState().insertDivider(1))
      renderRail()
      const divider = screen.getByRole('button', { name: 'separator' })
      fireEvent.contextMenu(divider)
      fireEvent.click(await screen.findByRole('menuitem', { name: 'Collapse Group' }))
      expect(
        screen.queryByRole('button', { name: 'Open the terminal board' })
      ).not.toBeInTheDocument()
      expect(screen.getByRole('button', { name: 'Open the conversations area' })).toBeVisible()
      fireEvent.contextMenu(divider)
      fireEvent.click(await screen.findByRole('menuitem', { name: 'Expand Group' }))
      expect(screen.getByRole('button', { name: 'Open the terminal board' })).toBeVisible()
    })

    it('persists plugin header collapse and retains an active contributed view', () => {
      const plugin = sidebarPlugin()
      plugin.manifest.contributes.views.push({
        id: 'other',
        title: 'Other view',
        location: 'sidebar'
      })
      usePluginsStore.setState({ plugins: [plugin], connected: true })
      const { unmount } = renderRail('/plugins/example.plugin/views/dashboard')
      fireEvent.click(screen.getByRole('button', { name: 'Plugins' }))
      expect(screen.getByRole('button', { name: 'Plugin dashboard' })).toBeVisible()
      expect(screen.queryByRole('button', { name: 'Other view' })).not.toBeInTheDocument()
      unmount()
      renderRail()
      expect(screen.getByRole('button', { name: 'Plugins' })).toHaveAttribute(
        'aria-expanded',
        'false'
      )
      expect(screen.queryByRole('button', { name: 'Plugin dashboard' })).not.toBeInTheDocument()
      fireEvent.click(screen.getByRole('button', { name: 'Plugins' }))
      expect(screen.getByRole('button', { name: 'Other view' })).toBeVisible()
    })

    it('shows the group name and hidden count in the collapsed header tooltip', async () => {
      renderRail()
      const divider = screen.getByRole('button', { name: 'tools group' })
      fireEvent.click(divider)
      const entries = useNavigationStore.getState().layout.entries
      const toolsIndex = entries.findIndex((entry) => entry.id === 'tools')
      const pluginIndex = entries.findIndex((entry) => entry.id === 'plugin-views')
      const hiddenCount = entries
        .slice(toolsIndex + 1, pluginIndex)
        .filter((entry) => entry.kind === 'item').length
      fireEvent.pointerMove(divider, { pointerType: 'mouse' })
      fireEvent.pointerEnter(divider, { pointerType: 'mouse' })
      expect(await screen.findByRole('tooltip')).toHaveTextContent(
        `tools group — ${hiddenCount} hidden items`
      )
    })

    it('drops a dragged item into a collapsed group and expands it without navigating', () => {
      vi.stubGlobal('PointerEvent', MouseEvent)
      try {
        renderRail()
        fireEvent.click(screen.getByRole('button', { name: 'workspace group' }))
        const rows = Array.from(document.querySelectorAll<HTMLElement>('[data-rail-row="true"]'))
        rows.forEach((row, index) => {
          vi.spyOn(row, 'getBoundingClientRect').mockReturnValue({
            x: 0,
            y: index * 36,
            top: index * 36,
            bottom: (index + 1) * 36,
            left: 0,
            right: 44,
            width: 44,
            height: 36,
            toJSON: () => ({})
          })
        })
        const workspaceIndex = rows.findIndex(
          (row) => row.dataset.railEntry === 'workspace-contexts'
        )
        const skillsIndex = rows.findIndex((row) => row.dataset.railEntry === 'skills')
        const skills = screen.getByRole('button', { name: 'Open skills' })
        fireEvent.pointerDown(skills, { button: 0, clientX: 20, clientY: skillsIndex * 36 + 18 })
        fireEvent.pointerMove(document, { clientX: 20, clientY: workspaceIndex * 36 + 30 })
        expect(document.querySelector('[data-rail-drop-indicator]')).not.toBeNull()
        fireEvent.pointerUp(document, { clientX: 20, clientY: workspaceIndex * 36 + 30 })
        fireEvent.click(screen.getByRole('button', { name: 'Open skills' }))
        expect(mockNavigate).not.toHaveBeenCalled()
        expect(screen.getByRole('button', { name: 'workspace group' })).toHaveAttribute(
          'aria-expanded',
          'true'
        )
        const entries = useNavigationStore.getState().layout.entries
        expect(entries.findIndex((entry) => entry.id === 'skills')).toBeLessThan(
          entries.findIndex((entry) => entry.id === 'tools')
        )
      } finally {
        vi.unstubAllGlobals()
      }
    })

    it('shows insertion feedback below an active item retained in a collapsed group', () => {
      vi.stubGlobal('PointerEvent', MouseEvent)
      try {
        renderRail('/mcp')
        fireEvent.click(screen.getByRole('button', { name: 'tools group' }))
        const rows = Array.from(document.querySelectorAll<HTMLElement>('[data-rail-row="true"]'))
        rows.forEach((row, index) => {
          vi.spyOn(row, 'getBoundingClientRect').mockReturnValue({
            x: 0,
            y: index * 36,
            top: index * 36,
            bottom: (index + 1) * 36,
            left: 0,
            right: 44,
            width: 44,
            height: 36,
            toJSON: () => ({})
          })
        })
        const mcpIndex = rows.findIndex((row) => row.dataset.railEntry === 'mcp')
        fireEvent.pointerDown(screen.getByRole('button', { name: 'Open projects' }), {
          button: 0,
          clientX: 20,
          clientY: 18
        })
        fireEvent.pointerMove(document, { clientX: 20, clientY: mcpIndex * 36 + 30 })
        expect(rows[mcpIndex].querySelector('[data-rail-drop-indicator="after"]')).not.toBeNull()
        fireEvent.pointerUp(document, { clientX: 20, clientY: mcpIndex * 36 + 30 })
        expect(screen.getByRole('button', { name: 'tools group' })).toHaveAttribute(
          'aria-expanded',
          'true'
        )
        const entries = useNavigationStore.getState().layout.entries
        expect(entries.findIndex((entry) => entry.id === 'projects')).toBe(
          entries.findIndex((entry) => entry.id === 'mcp') + 1
        )
      } finally {
        vi.unstubAllGlobals()
      }
    })

    it('keeps Alt+Arrow movement across a collapsed group available', () => {
      renderRail()
      fireEvent.click(screen.getByRole('button', { name: 'workspace group' }))
      const tools = screen.getByRole('button', { name: 'Open skills' })
      fireEvent.keyDown(tools, { key: 'ArrowUp', altKey: true })
      expect(screen.getByRole('button', { name: 'Open skills' })).toBeVisible()
      expect(screen.getByRole('button', { name: 'workspace group' })).toHaveAttribute(
        'aria-expanded',
        'true'
      )
    })
  })

  it('does not render sidebar or file-explorer toggles (moved to titlebar)', () => {
    renderRail()

    expect(screen.queryByRole('button', { name: /sidebar/i })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /file explorer/i })).not.toBeInTheDocument()
  })

  it('navigates to preferences on click', () => {
    renderRail()

    fireEvent.click(screen.getByRole('button', { name: 'Open preferences' }))

    expect(mockNavigate).toHaveBeenCalledWith('/preferences')
  })

  it('navigates to the MCP page on click', () => {
    renderRail()

    fireEvent.click(screen.getByRole('button', { name: 'Open MCP' }))

    expect(mockNavigate).toHaveBeenCalledWith('/mcp')
  })

  it('navigates to quick terminals on click', () => {
    renderRail()

    fireEvent.click(screen.getByRole('button', { name: 'Open quick terminals' }))

    expect(mockNavigate).toHaveBeenCalledWith('/quick-terminals')
  })

  it('exposes the keyboard shortcuts trigger', () => {
    renderRail()

    expect(screen.getByRole('button', { name: 'Open keyboard shortcuts menu' })).toBeInTheDocument()
  })

  it('disables color themes when no toggle handler is provided', () => {
    renderRail()

    const themeButton = screen.getByRole('button', { name: 'Color themes' })
    expect(themeButton).toBeDisabled()
    expect(themeButton).toHaveAttribute('aria-disabled', 'true')
    expect(themeButton).not.toHaveAttribute('aria-pressed')
  })

  it('toggles color themes when a toggle handler is provided', () => {
    const onToggleThemePicker = vi.fn()
    render(
      <MemoryRouter>
        <ActivityRail isThemePickerOpen onToggleThemePicker={onToggleThemePicker} />
      </MemoryRouter>
    )

    const themeButton = screen.getByRole('button', { name: 'Color themes' })
    expect(themeButton).not.toBeDisabled()
    expect(themeButton).toHaveAttribute('aria-pressed', 'true')

    fireEvent.click(themeButton)

    expect(onToggleThemePicker).toHaveBeenCalledTimes(1)
  })

  it('renders the brand mark', () => {
    renderRail()

    const mark = screen.getByRole('img', { name: 'Se' })
    expect(mark).toBeInTheDocument()
    expect(mark).toHaveAttribute('src', '/se-mark.png')
  })

  it('separates project actions from the conversations workspace', () => {
    const { container } = renderRail()

    const projectActions = screen.getByRole('group', { name: 'Project workspace and tools' })
    const conversationActions = screen.getByRole('group', { name: 'Conversation workspace' })
    const tools = screen.getByRole('group', { name: 'Tools' })

    expect(within(projectActions).getByRole('button', { name: 'Open projects' })).toBeVisible()
    expect(
      within(projectActions).getByRole('button', { name: 'Open the terminal board' })
    ).toBeVisible()
    expect(
      within(conversationActions).getByRole('button', {
        name: 'Open the conversations area'
      })
    ).toBeVisible()
    expect(within(tools).getByRole('button', { name: 'Open scheduled tasks' })).toBeVisible()
    expect(
      within(projectActions).queryByRole('button', { name: 'Open scheduled tasks' })
    ).not.toBeInTheDocument()
    expect(
      within(conversationActions).queryByRole('button', { name: 'Open scheduled tasks' })
    ).not.toBeInTheDocument()
    expect(
      projectActions.compareDocumentPosition(conversationActions) & Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy()
    expect(
      container.querySelector('[data-activity-rail-divider="workspace-contexts"]')
    ).toBeInTheDocument()
  })

  it('inserts a separator from an icon menu and removes it from the separator menu', async () => {
    const { container } = renderRail()

    fireEvent.contextMenu(screen.getByRole('button', { name: 'Open projects' }))
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Insert Separator Below' }))

    const entries = useNavigationStore.getState().layout.entries
    expect(entries.slice(0, 2)).toEqual([
      { kind: 'item', id: 'projects' },
      { kind: 'divider', id: 'sep-1' }
    ])
    const separator = container.querySelector('[data-activity-rail-divider="sep-1"]')
    expect(separator).toBeInTheDocument()

    fireEvent.contextMenu(separator as Element)
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Remove Separator' }))

    expect(container.querySelector('[data-activity-rail-divider="sep-1"]')).toBeNull()
    expect(useNavigationStore.getState().layout.entries.slice(0, 2)).toEqual([
      { kind: 'item', id: 'projects' },
      { kind: 'item', id: 'terminals' }
    ])
  })

  it('gives skills, AI channels, and MCP distinct icons', () => {
    renderRail()

    const iconOf = (name: string) =>
      screen.getByRole('button', { name }).querySelector('svg')?.getAttribute('class')
    const icons = ['Open skills', 'Open AI Channels', 'Open MCP'].map(iconOf)
    expect(icons.every(Boolean)).toBe(true)
    expect(new Set(icons).size).toBe(3)
  })

  it('offers no remove action on a built-in separator', () => {
    const { container } = renderRail()

    fireEvent.contextMenu(
      container.querySelector('[data-activity-rail-divider="workspace-contexts"]') as Element
    )

    expect(screen.queryByRole('menuitem', { name: 'Remove Separator' })).toBeNull()
  })

  it('provides hover labels for primary and utility actions', () => {
    const { container } = renderRail()

    for (const label of ['Projects', 'Terminals', 'Conversations', 'Preferences', 'Color themes']) {
      expect(container.querySelector(`[data-rail-tooltip="${label}"]`)).toBeInTheDocument()
    }
  })

  it('shows the localized label when an action is hovered', async () => {
    renderRail()

    const projectsButton = screen.getByRole('button', { name: 'Open projects' })
    fireEvent.pointerMove(projectsButton, { pointerType: 'mouse' })
    fireEvent.pointerEnter(projectsButton, { pointerType: 'mouse' })
    fireEvent.mouseOver(projectsButton)

    expect(await screen.findByRole('tooltip')).toHaveTextContent('Projects')
  })

  it('keeps the brand row draggable on macOS for top-left window moves', () => {
    platformState.isMac = true

    renderRail()

    const rail = screen.getByRole('navigation', { name: 'Global actions' })
    expect(rail.className).not.toContain('pt-[32px]')
    expect(rail.querySelector('[data-tauri-drag-region="true"]')).not.toBeNull()
  })

  it('opens the terminal board from the project section', () => {
    renderRail()

    fireEvent.click(screen.getByRole('button', { name: 'Open the terminal board' }))

    expect(mockNavigate).toHaveBeenCalledWith('/terminals')
  })

  // A selected SSH profile covers the main area on every route; a rail pick
  // that leaves it selected only changes the route behind the SSH view.
  it.each([
    ['Open the terminal board', '/terminals'],
    ['Open projects', '/'],
    ['Open preferences', '/preferences']
  ])('leaves the SSH view when "%s" is picked', (name, path) => {
    useSSHStore.setState({ activeProfileId: 'profile-1' })
    renderRail()

    fireEvent.click(screen.getByRole('button', { name }))

    expect(useSSHStore.getState().activeProfileId).toBeNull()
    expect(mockNavigate).toHaveBeenCalledWith(path)
  })

  it('keeps the SSH view when only the SSH panel is toggled', async () => {
    useSSHStore.setState({ activeProfileId: 'profile-1' })
    renderRail()

    fireEvent.click(screen.getByRole('button', { name: 'Hide SSH panel' }))

    await waitFor(() => {
      expect(mockUpdatePanelVisibility).toHaveBeenCalledWith('sshPanelVisible', false)
    })
    expect(useSSHStore.getState().activeProfileId).toBe('profile-1')
  })

  it('shows a live terminal count on the board action', () => {
    useTerminalStore.setState({
      terminals: [
        { id: 't-live', name: 'zsh', ptyId: 'pty-1', shell: 'zsh' },
        { id: 't-dead', name: 'old', shell: 'zsh' },
        // Exited shells keep their pty id until cleanup; they are not running.
        { id: 't-exited', name: 'done', ptyId: 'pty-2', shell: 'zsh', healthStatus: 'exited' }
      ]
    })

    renderRail()

    expect(screen.getByRole('button', { name: 'Open the terminal board' })).toHaveTextContent('1')
  })

  it('enters the regular project workspace via the projects action', () => {
    render(
      <MemoryRouter initialEntries={['/conversations']}>
        <ActivityRail />
      </MemoryRouter>
    )

    fireEvent.click(screen.getByRole('button', { name: 'Open projects' }))

    expect(mockNavigate).toHaveBeenCalledWith('/')
  })

  it('opens git changes when a project is available', () => {
    const onOpenGitChanges = vi.fn()
    render(
      <MemoryRouter>
        <ActivityRail onOpenGitChanges={onOpenGitChanges} canOpenGitChanges />
      </MemoryRouter>
    )

    fireEvent.click(screen.getByRole('button', { name: 'Open git changes' }))

    expect(onOpenGitChanges).toHaveBeenCalledTimes(1)
  })

  it('disables git changes when no project is available', () => {
    const onOpenGitChanges = vi.fn()
    render(
      <MemoryRouter>
        <ActivityRail onOpenGitChanges={onOpenGitChanges} canOpenGitChanges={false} />
      </MemoryRouter>
    )

    const gitButton = screen.getByRole('button', { name: 'Open git changes' })
    expect(gitButton).toBeDisabled()
    fireEvent.click(gitButton)
    expect(onOpenGitChanges).not.toHaveBeenCalled()
  })

  it('opens the conversations area from the rail chat toggle', () => {
    render(
      <MemoryRouter>
        <ActivityRail />
      </MemoryRouter>
    )

    const chatButton = screen.getByRole('button', { name: 'Open the conversations area' })
    expect(chatButton).toHaveAttribute('aria-pressed', 'false')
    fireEvent.click(chatButton)
    expect(mockNavigate).toHaveBeenCalledWith('/conversations')
  })

  it('returns to the project workspace when the conversations area is active', () => {
    render(
      <MemoryRouter initialEntries={['/conversations']}>
        <ActivityRail />
      </MemoryRouter>
    )

    const chatButton = screen.getByRole('button', { name: 'Open the conversations area' })
    expect(chatButton).toHaveAttribute('aria-pressed', 'true')
    fireEvent.click(chatButton)
    expect(mockNavigate).toHaveBeenCalledWith('/')
  })

  it('toggles the SSH panel via persistence-aware updater on click', async () => {
    renderRail()

    fireEvent.click(screen.getByRole('button', { name: 'Hide SSH panel' }))

    await waitFor(() => {
      expect(mockUpdatePanelVisibility).toHaveBeenCalledWith('sshPanelVisible', false)
    })
  })

  it('shows error toast when SSH panel persistence update fails', async () => {
    mockUpdatePanelVisibility.mockRejectedValueOnce(new Error('persist failed'))

    renderRail()

    fireEvent.click(screen.getByRole('button', { name: 'Hide SSH panel' }))

    await waitFor(() => {
      expect(mockToastError).toHaveBeenCalledWith('persist failed')
    })
  })

  it('disables the SSH rail button with a desktop-only reason on web', () => {
    const prev = tauriRef.current
    tauriRef.current = false
    try {
      renderRail()
      const sshButton = screen.getByRole('button', { name: /SSH/i })
      expect(sshButton).toBeDisabled()
      expect(sshButton.parentElement).toHaveAttribute('data-rail-tooltip', 'SSH is desktop-only')
    } finally {
      tauriRef.current = prev
    }
  })
})
