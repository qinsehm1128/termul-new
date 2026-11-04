import type {
  HostNotificationRecord,
  HostProgressRecord,
  HostUiSnapshot
} from '@shared/types/plugin-host.types'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { PluginFeedback, pluginFeedbackToastId } from './PluginFeedback'

const state = vi.hoisted(() => ({
  host: {
    snapshot: null as HostUiSnapshot | null,
    contextRevision: 17,
    connected: true,
    contextPending: false,
    context: null as import('@shared/types/plugin-host.types').HostContextState | null,
    execute: vi.fn().mockResolvedValue({ success: true, data: {} }),
    dismiss: vi.fn().mockResolvedValue(undefined)
  },
  plugins: {
    connected: true,
    bootId: 'boot',
    plugins: [{ id: 'org.test', state: 'active', desiredEnabled: true, generation: 'g1' }]
  },
  custom: vi.fn(),
  dismissToast: vi.fn()
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
vi.mock('@/stores/project-store', () => ({
  useProjectStore: { getState: () => ({ activeProjectId: 'project-1' }) }
}))
vi.mock('sonner', () => ({ toast: { custom: state.custom, dismiss: state.dismissToast } }))
vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string, values?: Record<string, string>) =>
      key === 'feedback.attribution'
        ? `Plugin: ${values?.name}`
        : key === 'feedback.dismiss'
          ? `Dismiss ${values?.title}`
          : key
  })
}))

const metadata = {
  pluginId: 'org.test',
  pluginName: 'Test Plugin',
  generation: 'g1',
  revision: 1,
  createdAt: 1,
  updatedAt: 1,
  expiresAt: null
}
function notification(overrides: Partial<HostNotificationRecord> = {}): HostNotificationRecord {
  return {
    ...metadata,
    id: 'notice',
    title: 'Build ready',
    severity: 'success',
    message: '<img src=x onerror=alert(1)>',
    actions: [{ label: 'Open build', commandId: 'open', args: null }],
    ...overrides
  }
}
function progress(overrides: Partial<HostProgressRecord> = {}): HostProgressRecord {
  return {
    ...metadata,
    id: 'build',
    title: 'Building',
    state: 'running',
    fraction: 0.25,
    cancelCommandId: 'cancel',
    ...overrides
  }
}
function snapshot(
  notifications = [notification()],
  progressItems: HostProgressRecord[] = []
): HostUiSnapshot {
  return {
    schemaVersion: 1,
    bootId: 'boot',
    revision: 1,
    notifications,
    progress: progressItems,
    statusItems: []
  }
}
function renderCard(index = 0): void {
  const factory = state.custom.mock.calls[index][0] as () => ReactNode
  render(<>{factory()}</>)
}

beforeEach(() => {
  vi.clearAllMocks()
  state.plugins.connected = true
  state.plugins.bootId = 'boot'
  state.plugins.plugins = [
    { id: 'org.test', state: 'active', desiredEnabled: true, generation: 'g1' }
  ]
  state.host.snapshot = snapshot()
  state.host.connected = true
  state.host.contextPending = false
  state.host.contextRevision = 17
  state.host.context = {
    activeProject: { id: 'project-1', name: 'Project', path: '/project' },
    projectIds: ['project-1'],
    activeTerminalId: null,
    activeSessionId: null,
    terminals: [],
    agentSessions: [],
    focus: { textInput: false, terminal: false },
    truncated: false
  }
})
afterEach(cleanup)

describe('PluginFeedback', () => {
  it('uses stable collision-safe IDs and updates progress without adding a second identity', () => {
    const record = progress()
    state.host.snapshot = snapshot([], [record])
    const mounted = render(<PluginFeedback />)
    const id = state.custom.mock.calls[0][1].id
    expect(state.custom.mock.calls[0][1]).toEqual({ id, duration: Infinity, dismissible: false })
    state.host.snapshot = snapshot([], [progress({ fraction: 0.75, revision: 2 })])
    mounted.rerender(<PluginFeedback />)
    expect(state.custom.mock.calls[1][1].id).toBe(id)
    expect(pluginFeedbackToastId({ kind: 'progress', record })).not.toBe(
      pluginFeedbackToastId({ kind: 'notification', record: notification({ id: record.id }) })
    )
    mounted.unmount()
    expect(state.dismissToast).toHaveBeenCalledWith(id)
  })

  it('renders attributed plain text, keyboard-operable declared actions, and host dismiss', async () => {
    render(<PluginFeedback />)
    renderCard()
    expect(screen.getByText('Plugin: Test Plugin · org.test')).toBeInTheDocument()
    expect(screen.getByText('<img src=x onerror=alert(1)>')).toBeInTheDocument()
    expect(document.querySelector('img')).toBeNull()
    const action = screen.getByRole('button', { name: 'Open build' })
    action.focus()
    expect(action).toHaveFocus()
    fireEvent.click(action)
    await waitFor(() =>
      expect(state.host.execute).toHaveBeenCalledWith({
        pluginId: 'org.test',
        generation: 'g1',
        kind: 'notification',
        contributionId: 'notice',
        actionIndex: 0,
        contextRevision: 17,
        target: { projectId: 'project-1' }
      })
    )
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss Build ready' }))
    expect(state.host.dismiss).toHaveBeenCalledWith({
      pluginId: 'org.test',
      generation: 'g1',
      kind: 'notification',
      id: 'notice'
    })
  })

  it.each([
    'pending',
    'missing',
    'disconnected',
    'zero'
  ])('blocks notification actions with %s context, including cached handlers', (condition) => {
    render(<PluginFeedback />)
    renderCard()
    if (condition === 'pending') state.host.contextPending = true
    if (condition === 'missing') state.host.context = null
    if (condition === 'disconnected') state.host.connected = false
    if (condition === 'zero') state.host.contextRevision = 0
    fireEvent.click(screen.getByRole('button', { name: 'Open build' }))
    expect(state.host.execute).not.toHaveBeenCalled()
  })

  it('disables notification and cancellation controls during pending context publication', () => {
    state.host.contextPending = true
    state.host.snapshot = snapshot([notification()], [progress()])
    render(<PluginFeedback />)
    renderCard(0)
    renderCard(1)
    expect(screen.getByRole('button', { name: 'Open build' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'feedback.cancel' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'Dismiss Build ready' })).toBeEnabled()
  })

  it('passes the acknowledged active terminal explicitly after publication', async () => {
    state.host.context = {
      ...state.host.context!,
      activeTerminalId: 'terminal-b',
      terminals: [{ id: 'terminal-b', ptyId: 'pty-b', projectId: 'project-1' }]
    }
    render(<PluginFeedback />)
    renderCard()
    fireEvent.click(screen.getByRole('button', { name: 'Open build' }))
    expect(state.host.execute).toHaveBeenCalledWith(
      expect.objectContaining({ target: { projectId: 'project-1', terminalId: 'terminal-b' } })
    )
    await waitFor(() => expect(screen.getByRole('button', { name: 'Open build' })).toBeEnabled())
  })

  it('routes progress cancellation through the contribution, not a raw command', async () => {
    state.host.snapshot = snapshot([], [progress()])
    render(<PluginFeedback />)
    renderCard()
    expect(screen.getByRole('progressbar')).toHaveAttribute('value', '0.25')
    fireEvent.click(screen.getByRole('button', { name: 'feedback.cancel' }))
    expect(state.host.execute).toHaveBeenCalledWith({
      pluginId: 'org.test',
      generation: 'g1',
      kind: 'progress',
      contributionId: 'build',
      contextRevision: 17,
      target: { projectId: 'project-1' }
    })
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'feedback.cancel' })).toBeEnabled()
    )
  })

  it('does not offer cancellation after completion and supports indeterminate progress', () => {
    state.host.snapshot = snapshot([], [progress({ state: 'completed', fraction: undefined })])
    render(<PluginFeedback />)
    renderCard()
    expect(screen.queryByRole('button', { name: 'feedback.cancel' })).not.toBeInTheDocument()
    expect(screen.getByRole('progressbar')).not.toHaveAttribute('value')
  })

  it.each([
    'disconnected',
    'disabled',
    'inactive',
    'generation',
    'boot',
    'expired'
  ])('suppresses %s records and removes previously rendered feedback', (condition) => {
    const mounted = render(<PluginFeedback />)
    const id = state.custom.mock.calls[0][1].id
    if (condition === 'disconnected') state.plugins.connected = false
    if (condition === 'disabled')
      state.plugins.plugins = [{ ...state.plugins.plugins[0], desiredEnabled: false }]
    if (condition === 'inactive')
      state.plugins.plugins = [{ ...state.plugins.plugins[0], state: 'failed' }]
    if (condition === 'generation')
      state.plugins.plugins = [{ ...state.plugins.plugins[0], generation: 'g2' }]
    if (condition === 'boot') state.host.snapshot = { ...state.host.snapshot!, bootId: 'old' }
    if (condition === 'expired')
      state.host.snapshot = snapshot([notification({ expiresAt: Date.now() - 1 })])
    mounted.rerender(<PluginFeedback />)
    expect(state.custom).toHaveBeenCalledTimes(1)
    expect(state.dismissToast).toHaveBeenCalledWith(id)
  })

  it('rejects actions from stale card handlers and hides private transport failures', async () => {
    render(<PluginFeedback />)
    renderCard()
    state.host.snapshot = snapshot([notification({ revision: 2 })])
    fireEvent.click(screen.getByRole('button', { name: 'Open build' }))
    expect(state.host.execute).not.toHaveBeenCalled()
    state.host.snapshot = snapshot()
    state.host.execute.mockRejectedValueOnce(new Error('sensitive internals'))
    fireEvent.click(screen.getByRole('button', { name: 'Open build' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('errors.generic')
    expect(screen.queryByText('sensitive internals')).not.toBeInTheDocument()
  })
})
