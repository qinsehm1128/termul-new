import type {
  ExecuteContributionRequest,
  HostContextState,
  HostUiSnapshot,
  PluginConnectionChanged,
  PluginEvent,
  PluginInfo
} from '@shared/types/plugin.types'
import { parseHostUiSnapshot, parsePluginManifest } from '@shared/types/plugin.types'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { Toaster, toast } from 'sonner'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ContextMenu, ContextMenuContent, ContextMenuTrigger } from '@/components/ui/context-menu'
import {
  matchesShortcut,
  pluginShortcutId,
  useKeyboardShortcutsStore
} from '@/stores/keyboard-shortcuts-store'
import { usePluginsStore } from '@/stores/plugins-store'
import { useProjectStore } from '@/stores/project-store'
import pilotManifest from '../../../../src-tauri/crates/se-plugin-example/src/manifest.json'
import { createHostContextPublisher, projectHostContext } from './host-context'
import { usePluginHostUiStore } from './host-ui-store'
import { PluginFeedback, pluginFeedbackToastId } from './PluginFeedback'
import { PluginMenuItems } from './PluginMenuItems'
import { PluginStatusItems } from './PluginStatusItems'
import { pluginApi } from './plugin-api'

// Only the native transport is replaced. The stores, projection/publisher, Radix
// menus, feedback/status components, and Sonner renderer are the public P4 code.
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => true }))

const manifest = parsePluginManifest(pilotManifest)
const bootId = '223e4567-e89b-42d3-a456-426614174000'
const generation = '323e4567-e89b-42d3-a456-426614174000'
const projectId = 'project-active'
const otherProjectId = 'project-right-clicked'
const sourceId = '123e4567-e89b-42d3-a456-426614174000'
const plugin: PluginInfo = {
  id: manifest.id,
  manifest,
  state: 'active',
  desiredEnabled: true,
  generation,
  installPath: '/plugins/example',
  sourcePath: '/plugins/example',
  lastError: null
}
const initialPlugins = usePluginsStore.getState()
const initialHostUi = usePluginHostUiStore.getState()
const initialProjects = useProjectStore.getState()
const initialShortcuts = useKeyboardShortcutsStore.getState()
let hostSnapshot: HostUiSnapshot
let changed: (event: PluginEvent) => void
let connectionChanged: (event: PluginConnectionChanged) => void
let release: (() => void) | undefined
let publisher: ReturnType<typeof createHostContextPublisher> | undefined

function context(activeTurn: boolean, replaying = false): HostContextState {
  return projectHostContext({
    projects: [
      { id: projectId, name: 'Active project', path: '/projects/active' },
      { id: otherProjectId, name: 'Target project', path: '/projects/target' }
    ],
    activeProjectId: projectId,
    terminals: [
      { id: 'terminal-active', projectId, ptyId: 'pty-active' },
      { id: 'terminal-target', projectId: otherProjectId, ptyId: 'pty-target' }
    ],
    sessions: [
      {
        id: 'session-active',
        projectId,
        status: 'active',
        activeTurn,
        openTurnId: activeTurn ? 'turn-observed' : null,
        replaying
      },
      {
        id: 'session-target',
        projectId: otherProjectId,
        status: 'idle',
        activeTurn: false,
        openTurnId: null,
        replaying: false
      }
    ],
    activeTerminalId: 'terminal-active',
    activeSessionId: 'session-active',
    focus: { terminal: true, textInput: false }
  })
}

/** Native presentation fixtures, matching activity.rs rather than reimplementing
 * its turn detection, subscriptions, or storage. Rust owns those assertions. */
function presentation(active: boolean, revision: number, turnEnded = !active): HostUiSnapshot {
  const declaration = manifest.contributes.statusItems!.find((item) => item.id === 'activity')!
  const metadata = {
    pluginId: manifest.id,
    pluginName: manifest.name,
    generation,
    revision,
    createdAt: 1,
    updatedAt: revision,
    expiresAt: null
  }
  return parseHostUiSnapshot({
    schemaVersion: 1,
    bootId,
    revision,
    statusItems: [
      {
        ...declaration,
        ...metadata,
        text: active ? 'Agent active' : 'Agent idle',
        icon: 'calculator',
        projectId
      }
    ],
    progress: [
      {
        ...metadata,
        id: 'activity-turn',
        title: 'Agent activity',
        state: active ? 'running' : 'completed'
      }
    ],
    notifications: turnEnded
      ? [
          {
            ...metadata,
            id: 'activity-turn-ended',
            title: 'Agent turn ended',
            severity: 'info',
            message: 'An observed agent turn ended; this is not a success assertion.',
            actions: [
              {
                label: 'Summary',
                commandId: 'example.activity-summary',
                args: null
              }
            ]
          }
        ]
      : []
  })
}

async function connect(): Promise<void> {
  await act(async () => {
    release = usePluginHostUiStore.getState().subscribe()
    connectionChanged({ state: 'connected', bootId })
  })
  await waitFor(() => expect(usePluginHostUiStore.getState().snapshot).toEqual(hostSnapshot))
}

async function deliver(snapshot: HostUiSnapshot): Promise<void> {
  hostSnapshot = snapshot
  await act(async () => {
    changed({ type: 'host_ui_changed', bootId, revision: snapshot.revision })
  })
  await waitFor(() => expect(usePluginHostUiStore.getState().snapshot).toEqual(snapshot))
}

function assertAction(request: Omit<ExecuteContributionRequest, 'pluginId' | 'generation'>): void {
  expect(pluginApi.executeContribution).toHaveBeenLastCalledWith({
    pluginId: manifest.id,
    generation,
    ...request
  })
}

beforeEach(() => {
  hostSnapshot = presentation(true, 1)
  vi.spyOn(pluginApi, 'onChanged').mockImplementation((handler) => {
    changed = handler
    return Object.assign(vi.fn(), { ready: Promise.resolve() })
  })
  vi.spyOn(pluginApi, 'onConnectionChanged').mockImplementation((handler) => {
    connectionChanged = handler
    return Object.assign(vi.fn(), { ready: Promise.resolve() })
  })
  vi.spyOn(pluginApi, 'getHostUi').mockImplementation(async () => ({
    success: true,
    data: hostSnapshot
  }))
  vi.spyOn(pluginApi, 'executeContribution').mockResolvedValue({
    success: true,
    data: { result: null }
  })
  vi.spyOn(pluginApi, 'publishHostContext').mockImplementation(async (request) => ({
    success: true,
    data: {
      sourceSequence: request.sourceSequence,
      contextRevision: request.sourceSequence
    }
  }))
  usePluginsStore.setState({ plugins: [plugin], connected: true, bootId })
  useProjectStore.setState({ activeProjectId: projectId })
})

afterEach(() => {
  cleanup()
  publisher?.dispose()
  publisher = undefined
  release?.()
  release = undefined
  toast.dismiss()
  usePluginHostUiStore.setState(initialHostUi, true)
  usePluginsStore.setState(initialPlugins, true)
  useProjectStore.setState(initialProjects, true)
  useKeyboardShortcutsStore.setState(initialShortcuts, true)
  vi.restoreAllMocks()
})

describe('Activity Pilot public renderer integration', () => {
  it('shows active → idle, one turn-ended notification, in-place progress, and no replay/resync duplicate', async () => {
    await connect()
    publisher = createHostContextPublisher({
      api: pluginApi,
      sourceId,
      initial: context(true),
      onAck: (ack, snapshot) =>
        usePluginHostUiStore.getState().acknowledgeContext(ack.contextRevision, snapshot)
    })
    publisher.reconnect()
    await waitFor(() => expect(usePluginHostUiStore.getState().contextRevision).toBe(1))
    const custom = vi.spyOn(toast, 'custom')
    render(
      <>
        <Toaster />
        <PluginFeedback />
        <PluginStatusItems projectId={projectId} />
      </>
    )
    const status = screen.getByRole('button', { name: /Agent active/ })
    const progress = await screen.findByRole('progressbar', {
      name: 'Agent activity'
    })
    expect(screen.getByText('Running')).toBeInTheDocument()
    expect(screen.queryByText('Agent turn ended')).not.toBeInTheDocument()

    act(() => publisher!.capture(context(false)))
    await waitFor(() => expect(usePluginHostUiStore.getState().contextRevision).toBe(2))
    expect(pluginApi.publishHostContext).toHaveBeenLastCalledWith(
      expect.objectContaining({
        reset: false,
        events: [expect.objectContaining({ topic: 'agent.session.stateChanged' })]
      })
    )
    await deliver(presentation(false, 2))
    expect(screen.getByRole('button', { name: /Agent idle/ })).toBe(status)
    expect(screen.queryByText('Agent active')).not.toBeInTheDocument()
    await waitFor(() => expect(screen.getByText('Completed')).toBeInTheDocument())
    expect(screen.getByRole('progressbar', { name: 'Agent activity' })).toBe(progress)
    const notification = screen.getByRole('region', {
      name: `${manifest.name}: Agent turn ended`
    })
    expect(screen.getAllByText('Agent turn ended')).toHaveLength(1)
    expect(screen.getByText(/not a success assertion/)).toBeInTheDocument()
    expect(screen.queryByText('Running')).not.toBeInTheDocument()

    fireEvent.click(status)
    const activeTarget = {
      projectId,
      terminalId: 'terminal-active',
      sessionId: 'session-active'
    }
    assertAction({
      kind: 'status',
      contributionId: 'activity',
      contextRevision: 2,
      target: activeTarget
    })
    fireEvent.click(screen.getByRole('button', { name: 'Summary' }))
    await waitFor(() => expect(pluginApi.executeContribution).toHaveBeenCalledTimes(2))
    assertAction({
      kind: 'notification',
      contributionId: 'activity-turn-ended',
      contextRevision: 2,
      target: activeTarget,
      actionIndex: 0
    })

    // Replay the same host revision: public cache drops it before rendering.
    const callsBeforeReplay = custom.mock.calls.length
    await deliver(structuredClone(hostSnapshot))
    expect(custom).toHaveBeenCalledTimes(callsBeforeReplay)

    // A replay baseline and an explicit reconnect publish snapshots without
    // historical transitions. Their native fixture retains the existing record.
    act(() => publisher!.capture(context(false, true), true))
    await waitFor(() => expect(usePluginHostUiStore.getState().contextRevision).toBe(3))
    expect(pluginApi.publishHostContext).toHaveBeenLastCalledWith(
      expect.objectContaining({ reset: true, events: [] })
    )
    act(() => publisher!.reconnect())
    await waitFor(() => expect(usePluginHostUiStore.getState().contextRevision).toBe(4))
    expect(pluginApi.publishHostContext).toHaveBeenLastCalledWith(
      expect.objectContaining({ reset: true, events: [] })
    )
    await deliver({ ...hostSnapshot, revision: 3 })
    const notificationId = pluginFeedbackToastId({
      kind: 'notification',
      record: hostSnapshot.notifications[0]
    })
    const notificationIds = custom.mock.calls
      .map(([, options]) => options?.id)
      .filter((id) => id === notificationId)
    expect(new Set(notificationIds)).toEqual(new Set([notificationId]))
    expect(screen.getByRole('region', { name: `${manifest.name}: Agent turn ended` })).toBe(
      notification
    )
    expect(screen.getAllByText('Agent turn ended')).toHaveLength(1)
    expect(screen.getAllByRole('progressbar', { name: 'Agent activity' })).toHaveLength(1)
    expect(screen.getByRole('progressbar', { name: 'Agent activity' })).toBe(progress)
    expect(usePluginHostUiStore.getState().snapshot!.notifications).toHaveLength(1)
  })

  it.each([
    [
      'activity-terminal-tab',
      'terminal.tab',
      { projectId: otherProjectId, terminalId: 'terminal-target' }
    ],
    [
      'activity-terminal-context',
      'terminal.context',
      { projectId: otherProjectId, terminalId: 'terminal-target' }
    ],
    ['activity-project-context', 'project.context', { projectId: otherProjectId }],
    [
      'activity-agent-session',
      'agentSession.context',
      { projectId: otherProjectId, sessionId: 'session-target' }
    ]
  ] as const)('dispatches %s from %s with the right-clicked target, not active IDs', async (id, location, target) => {
    await connect()
    usePluginHostUiStore.getState().acknowledgeContext(11, context(true))
    expect(manifest.contributes.menus).toContainEqual(
      expect.objectContaining({
        id,
        location,
        commandId: 'example.activity-summary'
      })
    )
    render(
      <ContextMenu>
        <ContextMenuTrigger>Target object</ContextMenuTrigger>
        <ContextMenuContent>
          <PluginMenuItems location={location} target={target} />
        </ContextMenuContent>
      </ContextMenu>
    )
    fireEvent.contextMenu(screen.getByText('Target object'))
    const group = await screen.findByRole('group', { name: 'Plugins' })
    const item = within(group).getByRole('menuitem', {
      name: /Activity summary/
    })
    expect(within(item).getByText(manifest.name)).toBeInTheDocument()
    fireEvent.click(item)
    await waitFor(() => expect(pluginApi.executeContribution).toHaveBeenCalledTimes(1))
    assertAction({
      kind: 'menu',
      contributionId: id,
      contextRevision: 11,
      target
    })
  })

  it('registers the manifest pilot shortcut as available but unbound, without changing built-ins', () => {
    const binding = manifest.contributes.keybindings!.find(
      (entry) => entry.id === 'activity-summary'
    )!
    expect(binding).toEqual(
      expect.objectContaining({
        commandId: 'example.activity-summary',
        key: ''
      })
    )
    expect(manifest.contributes.commands).toContainEqual(
      expect.objectContaining({ id: 'example.activity-toggle' })
    )
    const builtIns = { ...useKeyboardShortcutsStore.getState().shortcuts }
    const id = pluginShortcutId(manifest.id, binding.id)
    useKeyboardShortcutsStore.getState().registerPluginShortcuts(manifest.id, generation, [
      {
        pluginId: manifest.id,
        pluginName: manifest.name,
        generation,
        bindingId: binding.id,
        commandId: binding.commandId,
        label: 'Activity summary',
        defaultKey: binding.key,
        when: binding.when
      }
    ])
    const registered = useKeyboardShortcutsStore.getState()
    expect(registered.shortcuts[id].defaultKey).toBe('')
    const effectiveKey = registered.shortcuts[id].customKey ?? registered.shortcuts[id].defaultKey
    expect(effectiveKey).toBe('')
    expect(registered.pluginConflicts[id]).toBeUndefined()
    expect(matchesShortcut(new KeyboardEvent('keydown', { key: 'a' }), effectiveKey)).toBe(false)
    for (const [key, value] of Object.entries(builtIns))
      expect(registered.shortcuts[key]).toEqual(value)
    registered.unregisterPluginShortcuts(manifest.id, generation)
    expect(useKeyboardShortcutsStore.getState().shortcuts[id]).toBeUndefined()
  })
})
