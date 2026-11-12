import type { HostContextState, HostWhen, PluginInfo, PluginMenu } from '@shared/types/plugin.types'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { usePluginsStore } from '@/stores/plugins-store'
import { matchesHostWhen } from './contribution-context'
import { usePluginHostUiStore } from './host-ui-store'
import { PluginMenuItems } from './PluginMenuItems'
import { pluginApi } from './plugin-api'

vi.mock('./plugin-api', () => ({
  pluginTransportAvailable: () => true,
  pluginApi: { executeContribution: vi.fn() }
}))
vi.mock('@/components/ui/context-menu', () => ({
  ContextMenuSeparator: () => <hr />,
  ContextMenuGroup: ({ children }: { children: React.ReactNode }) => (
    <fieldset aria-label="Plugins">{children}</fieldset>
  ),
  ContextMenuItem: ({
    children,
    onSelect
  }: {
    children: React.ReactNode
    onSelect: () => void
  }) => (
    <button type="button" role="menuitem" onClick={onSelect}>
      {children}
    </button>
  )
}))

function plugin(overrides: Partial<PluginInfo> = {}, menu: Partial<PluginMenu> = {}): PluginInfo {
  return {
    id: 'org.test.menu',
    manifest: {
      schemaVersion: 1,
      id: 'org.test.menu',
      name: 'Test Plugin',
      version: '1.0.0',
      abi: { min: 1, max: 1 },
      contributes: {
        commands: [{ id: 'run', title: 'Plugin action' }],
        configuration: { title: 'Config', properties: {} },
        views: [],
        menus: [{ id: 'menu', commandId: 'run', location: 'terminal.tab', ...menu }]
      },
      services: { provides: [], requires: [] }
    },
    source: { kind: 'local', path: '/plugin.wasm' },
    state: 'active',
    desiredEnabled: true,
    runtimeEnabled: true,
    generation: 'generation',
    configRevision: 0,
    manifestRevision: 0,
    ...overrides
  }
}

function context(): HostContextState {
  return {
    activeProject: { id: 'active-project', name: 'Active', path: '/active' },
    projectIds: ['active-project', 'inactive-project', 'other', 'clicked'],
    activeTerminalId: 'active-terminal',
    activeSessionId: 'active-session',
    terminals: [
      { id: 'active-terminal', ptyId: null, projectId: 'active-project' },
      { id: 'inactive-terminal', ptyId: null, projectId: 'inactive-project' },
      { id: 'clicked', ptyId: null, projectId: 'other' }
    ],
    agentSessions: ['active-session', 'clicked-session', 'clicked'].map((id) => ({
      id,
      projectId: 'active-project',
      status: 'active',
      activeTurn: false,
      openTurnId: null,
      replaying: false
    })),
    focus: { terminal: false, textInput: false },
    truncated: false
  }
}

beforeEach(() => {
  vi.mocked(pluginApi.executeContribution)
    .mockReset()
    .mockResolvedValue({ success: true, data: { result: null } })
  usePluginsStore.setState({ connected: true, plugins: [plugin()] })
  usePluginHostUiStore.setState({ contextRevision: 7, context: context(), contextPending: true })
})
afterEach(() => {
  cleanup()
  usePluginsStore.setState({ connected: false, plugins: [] })
  usePluginHostUiStore.setState({ contextRevision: 0, context: null })
})

describe('PluginMenuItems', () => {
  it('adds one group after native items with icon/name attribution and sends clicked IDs', () => {
    const target = { terminalId: 'inactive-terminal', projectId: 'inactive-project' }
    render(
      <>
        <button type="button" role="menuitem">
          Native close
        </button>
        <PluginMenuItems location="terminal.tab" target={target} />
      </>
    )
    expect(screen.getAllByRole('menuitem').map((item) => item.textContent)).toEqual([
      'Native close',
      'Plugin actionTest Plugin'
    ])
    expect(screen.getAllByRole('separator')).toHaveLength(1)
    expect(
      screen.getByRole('menuitem', { name: /Plugin action/ }).querySelector('svg')
    ).not.toBeNull()
    fireEvent.click(screen.getByRole('menuitem', { name: /Plugin action/ }))
    expect(pluginApi.executeContribution).toHaveBeenCalledWith({
      pluginId: 'org.test.menu',
      generation: 'generation',
      kind: 'menu',
      contributionId: 'menu',
      contextRevision: 7,
      target
    })
  })

  it.each([
    'terminal.tab',
    'terminal.context',
    'project.context',
    'agentSession.context'
  ] as const)('renders the %s location only', (location) => {
    usePluginsStore.setState({ plugins: [plugin({}, { location })] })
    const target = location.startsWith('terminal.')
      ? { terminalId: 'clicked' }
      : location === 'project.context'
        ? { projectId: 'clicked' }
        : { sessionId: 'clicked-session' }
    render(<PluginMenuItems location={location} target={target} />)
    expect(screen.getByText('Plugin action')).toBeInTheDocument()
  })

  it.each([
    { connected: false },
    { plugins: [plugin({ state: 'inactive' })] },
    { plugins: [plugin({ desiredEnabled: false })] },
    { plugins: [plugin({ generation: undefined })] }
  ])('hides unavailable plugins and the separator: %j', (state) => {
    usePluginsStore.setState(state)
    const { container } = render(
      <PluginMenuItems location="terminal.tab" target={{ terminalId: 'clicked' }} />
    )
    expect(container).toBeEmptyDOMElement()
  })

  it('evaluates finite AND against clicked target and focus, not active objects', () => {
    usePluginsStore.setState({
      plugins: [
        plugin(
          {},
          { when: { 'terminal.exists': true, 'project.exists': false, 'textInput.focused': false } }
        )
      ]
    })
    const { rerender } = render(
      <PluginMenuItems location="terminal.tab" target={{ terminalId: 'clicked' }} />
    )
    expect(screen.getByText('Plugin action')).toBeInTheDocument()
    rerender(
      <PluginMenuItems
        location="terminal.tab"
        target={{ terminalId: 'clicked', projectId: 'other' }}
      />
    )
    expect(screen.queryByText('Plugin action')).not.toBeInTheDocument()
    expect(screen.queryByRole('separator')).not.toBeInTheDocument()
  })

  it('removes open-menu actions on disconnect or disable', () => {
    render(<PluginMenuItems location="terminal.tab" target={{ terminalId: 'clicked' }} />)
    act(() => usePluginsStore.setState({ connected: false }))
    expect(screen.queryByText('Plugin action')).not.toBeInTheDocument()
    act(() =>
      usePluginsStore.setState({ connected: true, plugins: [plugin({ desiredEnabled: false })] })
    )
    expect(screen.queryByText('Plugin action')).not.toBeInTheDocument()
  })

  it('shares one separator across plugins and orders contributions deterministically', () => {
    usePluginsStore.setState({
      plugins: [plugin(), plugin({ id: 'org.other' }, { id: 'first', order: -1 })]
    })
    render(<PluginMenuItems location="terminal.tab" target={{ terminalId: 'clicked' }} />)
    expect(screen.getAllByRole('separator')).toHaveLength(1)
    fireEvent.click(screen.getAllByRole('menuitem')[0])
    expect(pluginApi.executeContribution).toHaveBeenCalledWith(
      expect.objectContaining({ pluginId: 'org.other', contributionId: 'first' })
    )
  })

  it('reads the latest published context revision when the action is selected', () => {
    render(<PluginMenuItems location="terminal.tab" target={{ terminalId: 'clicked' }} />)
    act(() => usePluginHostUiStore.setState({ contextRevision: 11 }))
    fireEvent.click(screen.getByRole('menuitem'))
    expect(pluginApi.executeContribution).toHaveBeenCalledWith(
      expect.objectContaining({ contextRevision: 11 })
    )
  })

  it('does not append an empty group for another location or a missing command', () => {
    usePluginsStore.setState({ plugins: [plugin({}, { commandId: 'missing' })] })
    const { rerender, container } = render(
      <PluginMenuItems location="terminal.tab" target={{ terminalId: 'clicked' }} />
    )
    expect(container).toBeEmptyDOMElement()
    act(() => usePluginsStore.setState({ plugins: [plugin()] }))
    rerender(<PluginMenuItems location="project.context" target={{ projectId: 'clicked' }} />)
    expect(container).toBeEmptyDOMElement()
  })

  it('uses explicit body focus and clicked session identity for finite focus predicates', () => {
    usePluginsStore.setState({
      plugins: [
        plugin(
          {},
          {
            location: 'agentSession.context',
            when: {
              'agentSession.exists': true,
              'terminal.focused': true,
              'textInput.focused': false
            }
          }
        )
      ]
    })
    const { rerender } = render(
      <PluginMenuItems
        location="agentSession.context"
        target={{ sessionId: 'clicked' }}
        terminalFocused
      />
    )
    expect(screen.getByRole('menuitem')).toBeInTheDocument()
    rerender(
      <PluginMenuItems
        location="terminal.tab"
        target={{ sessionId: 'clicked' }}
        terminalFocused
        textInputFocused
      />
    )
    expect(screen.queryByRole('menuitem')).not.toBeInTheDocument()
  })

  it.each([
    { projectId: 'omitted-project' },
    { terminalId: 'omitted-terminal' },
    { sessionId: 'omitted-session' }
  ])('hides bounded-context omitted targets: %j', (target) => {
    const location = target.projectId
      ? 'project.context'
      : target.terminalId
        ? 'terminal.context'
        : 'agentSession.context'
    usePluginsStore.setState({ plugins: [plugin({}, { location })] })
    usePluginHostUiStore.setState({ context: { ...context(), truncated: true } })
    const { container } = render(<PluginMenuItems location={location} target={target} />)
    expect(container).toBeEmptyDOMElement()
    expect(pluginApi.executeContribution).not.toHaveBeenCalled()
  })

  it.each([
    { context: null },
    { contextRevision: 0 }
  ])('requires an acknowledged context: %j', (state) => {
    usePluginHostUiStore.setState(state)
    const { container } = render(
      <PluginMenuItems location="terminal.tab" target={{ terminalId: 'clicked' }} />
    )
    expect(container).toBeEmptyDOMElement()
  })

  it('defaults focus predicates to the acknowledged context without gating pending publication', () => {
    usePluginsStore.setState({
      plugins: [plugin({}, { when: { 'terminal.focused': true, 'textInput.focused': false } })]
    })
    usePluginHostUiStore.setState({
      context: { ...context(), focus: { terminal: true, textInput: false } },
      contextPending: true
    })
    render(<PluginMenuItems location="terminal.tab" target={{ terminalId: 'clicked' }} />)
    expect(screen.getByRole('menuitem')).toBeInTheDocument()
    act(() =>
      usePluginHostUiStore.setState({
        context: { ...context(), focus: { terminal: false, textInput: true } }
      })
    )
    expect(screen.queryByRole('menuitem')).not.toBeInTheDocument()
  })

  it('rejects a target from a different acknowledged project', () => {
    const { container } = render(
      <PluginMenuItems
        location="terminal.tab"
        target={{ terminalId: 'inactive-terminal', projectId: 'active-project' }}
      />
    )
    expect(container).toBeEmptyDOMElement()
  })

  it('ignores unknown/malformed when keys instead of interpreting expressions', () => {
    const context = {
      'project.exists': false,
      'terminal.exists': true,
      'agentSession.exists': false,
      'terminal.focused': false,
      'textInput.focused': false
    }
    expect(matchesHostWhen(undefined, context)).toBe(true)
    expect(matchesHostWhen({}, context)).toBe(true)
    expect(matchesHostWhen({ 'terminal.exists': true, 'terminal.focused': false }, context)).toBe(
      true
    )
    expect(matchesHostWhen({ expression: 'true' } as unknown as HostWhen, context)).toBe(false)
    expect(matchesHostWhen({ 'terminal.exists': 'true' } as unknown as HostWhen, context)).toBe(
      false
    )
  })
})
