import type { HostStatusRecord, HostUiSnapshot } from '@shared/types/plugin-host.types'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { PluginStatusItems } from './PluginStatusItems'

const state = vi.hoisted(() => ({
  host: {
    snapshot: null as HostUiSnapshot | null,
    contextRevision: 12,
    connected: true,
    contextPending: false,
    context: null as import('@shared/types/plugin-host.types').HostContextState | null,
    execute: vi.fn().mockResolvedValue({ success: true, data: {} })
  },
  plugins: {
    connected: true,
    bootId: 'boot',
    plugins: [
      { id: 'org.alpha', state: 'active', desiredEnabled: true, generation: 'g1' },
      { id: 'org.beta', state: 'active', desiredEnabled: true, generation: 'g1' }
    ]
  }
}))
vi.mock('./host-ui-store', () => ({
  usePluginHostUiStore: Object.assign(
    (select: (value: typeof state.host) => unknown) => select(state.host),
    { getState: () => state.host }
  )
}))
vi.mock('@/stores/plugins-store', () => ({
  usePluginsStore: Object.assign(
    (select: (value: typeof state.plugins) => unknown) => select(state.plugins),
    { getState: () => state.plugins }
  )
}))
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }))

function status(overrides: Partial<HostStatusRecord> = {}): HostStatusRecord {
  return {
    pluginId: 'org.alpha',
    pluginName: 'Alpha',
    generation: 'g1',
    revision: 1,
    createdAt: 1,
    updatedAt: 1,
    expiresAt: null,
    id: 'build',
    text: 'Ready',
    label: 'Build',
    location: 'statusBar',
    scope: 'window',
    order: 0,
    icon: 'cloud',
    tone: 'success',
    commandId: 'build',
    ...overrides
  }
}
function setItems(statusItems: HostStatusRecord[]): void {
  state.host.snapshot = {
    schemaVersion: 1,
    bootId: 'boot',
    revision: 1,
    notifications: [],
    progress: [],
    statusItems
  }
}
beforeEach(() => {
  vi.clearAllMocks()
  state.plugins.connected = true
  state.plugins.plugins = [
    { id: 'org.alpha', state: 'active', desiredEnabled: true, generation: 'g1' },
    { id: 'org.beta', state: 'active', desiredEnabled: true, generation: 'g1' }
  ]
  state.host.connected = true
  state.host.contextPending = false
  state.host.contextRevision = 12
  state.host.context = {
    activeProject: { id: 'p1', name: 'Project', path: '/project' },
    projectIds: ['p1'],
    activeTerminalId: null,
    activeSessionId: null,
    terminals: [],
    agentSessions: [],
    focus: { textInput: false, terminal: false },
    truncated: false
  }
  setItems([status()])
})
afterEach(cleanup)

describe('PluginStatusItems', () => {
  it('sorts by order, pluginId and id independent of arrival order', () => {
    setItems([
      status({ id: 'b', text: 'third' }),
      status({ pluginId: 'org.beta', text: 'fourth' }),
      status({ id: 'a', text: 'second' }),
      status({ id: 'priority', text: 'first', order: -1 })
    ])
    render(<PluginStatusItems />)
    expect(screen.getAllByRole('button').map((item) => item.textContent)).toEqual([
      'first',
      'second',
      'third',
      'fourth'
    ])
  })

  it('filters hidden, expired and other-project activeProject items while retaining window items', () => {
    setItems([
      status({ id: 'window', text: 'Window' }),
      status({ id: 'project', text: 'Project', scope: 'activeProject', projectId: 'p1' }),
      status({ id: 'other', text: 'Other', scope: 'activeProject', projectId: 'p2' }),
      status({ id: 'hidden', text: 'Hidden', visible: false }),
      status({ id: 'expired', text: 'Expired', expiresAt: Date.now() - 1 })
    ])
    const mounted = render(<PluginStatusItems projectId="p1" />)
    expect(screen.getByText('Window')).toBeInTheDocument()
    expect(screen.getByText('Project')).toBeInTheDocument()
    expect(screen.queryByText('Other')).not.toBeInTheDocument()
    expect(screen.queryByText('Hidden')).not.toBeInTheDocument()
    expect(screen.queryByText('Expired')).not.toBeInTheDocument()
    mounted.rerender(<PluginStatusItems />)
    expect(screen.queryByText('Project')).not.toBeInTheDocument()
    expect(screen.getByText('Window')).toBeInTheDocument()
  })

  it('uses host-owned icons, safe tone classes, and accessible attribution', () => {
    setItems([
      status({
        icon: '<img src=x>',
        tone: 'bg-red-500' as HostStatusRecord['tone'],
        text: '<b>unsafe</b>'
      })
    ])
    const { container } = render(<PluginStatusItems />)
    const button = screen.getByRole('button', { name: 'Alpha (org.alpha): Build — <b>unsafe</b>' })
    expect(button).toHaveClass('text-status-bar-foreground')
    expect(button).not.toHaveClass('bg-red-500')
    expect(button.querySelector('svg')).toHaveClass('lucide-puzzle')
    expect(container.querySelector('img')).toBeNull()
    expect(container.querySelector('b')).toBeNull()
  })

  it('executes declared status contribution with the latest context revision and project target', () => {
    render(<PluginStatusItems projectId="p1" />)
    state.host.contextRevision = 23
    const button = screen.getByRole('button')
    button.focus()
    expect(button).toHaveFocus()
    fireEvent.click(button)
    expect(state.host.execute).toHaveBeenCalledWith({
      pluginId: 'org.alpha',
      generation: 'g1',
      kind: 'status',
      contributionId: 'build',
      contextRevision: 23,
      target: { projectId: 'p1' }
    })
  })

  it('renders items without commands as noninteractive text', () => {
    setItems([status({ commandId: undefined })])
    render(<PluginStatusItems />)
    expect(screen.queryByRole('button')).not.toBeInTheDocument()
    expect(screen.getByText('Ready')).toBeInTheDocument()
  })

  it.each([
    'disconnected',
    'disabled',
    'inactive',
    'generation',
    'boot'
  ])('hides %s plugin records', (condition) => {
    if (condition === 'disconnected') state.plugins.connected = false
    if (condition === 'disabled') state.plugins.plugins[0].desiredEnabled = false
    if (condition === 'inactive') state.plugins.plugins[0].state = 'failed'
    if (condition === 'generation') state.plugins.plugins[0].generation = 'g2'
    if (condition === 'boot') state.host.snapshot!.bootId = 'old'
    render(<PluginStatusItems />)
    expect(screen.queryByText('Ready')).not.toBeInTheDocument()
  })

  it.each([
    'pending',
    'missing',
    'disconnected',
    'zero'
  ])('does not consume cached status actions with %s context', (condition) => {
    render(<PluginStatusItems projectId="p1" />)
    if (condition === 'pending') state.host.contextPending = true
    if (condition === 'missing') state.host.context = null
    if (condition === 'disconnected') state.host.connected = false
    if (condition === 'zero') state.host.contextRevision = 0
    fireEvent.click(screen.getByRole('button'))
    expect(state.host.execute).not.toHaveBeenCalled()
  })

  it('disables status controls while publication is pending', () => {
    state.host.contextPending = true
    render(<PluginStatusItems projectId="p1" />)
    expect(screen.getByRole('button')).toBeDisabled()
  })

  it('passes the accepted active session identity explicitly', () => {
    state.host.context = {
      ...state.host.context!,
      activeSessionId: 'session-b',
      agentSessions: [
        {
          id: 'session-b',
          projectId: 'p1',
          status: 'active',
          activeTurn: true,
          openTurnId: 'turn',
          replaying: false
        }
      ]
    }
    render(<PluginStatusItems projectId="p1" />)
    fireEvent.click(screen.getByRole('button'))
    expect(state.host.execute).toHaveBeenCalledWith(
      expect.objectContaining({ target: { projectId: 'p1', sessionId: 'session-b' } })
    )
  })

  it('does not execute a stale record after a lifecycle or snapshot change', () => {
    render(<PluginStatusItems projectId="p1" />)
    state.plugins.connected = false
    fireEvent.click(screen.getByRole('button'))
    expect(state.host.execute).not.toHaveBeenCalled()
    state.plugins.connected = true
    setItems([status({ revision: 2 })])
    fireEvent.click(screen.getByRole('button'))
    expect(state.host.execute).not.toHaveBeenCalled()
  })
})
