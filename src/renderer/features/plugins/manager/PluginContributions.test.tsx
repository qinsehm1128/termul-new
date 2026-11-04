import type { PluginInfo } from '@shared/types/plugin.types'
import { cleanup, render, screen, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { pluginShortcutId, useKeyboardShortcutsStore } from '@/stores/keyboard-shortcuts-store'
import { PluginContributions } from './PluginContributions'

vi.mock('@/lib/platform', () => ({ isMac: true }))
vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string, values?: { id?: string }) => `${key}${values?.id ? `:${values.id}` : ''}`
  })
}))

function fixture(): PluginInfo {
  return {
    id: 'org.plugin/with space',
    desiredEnabled: true,
    state: 'active',
    generation: 'g1',
    configRevision: 1,
    sha256: '',
    lastError: null,
    manifest: {
      schemaVersion: 1,
      id: 'org.plugin/with space',
      name: 'Plugin',
      version: '1.0.0',
      abi: { min: 1, max: 1 },
      contributes: {
        configuration: { title: 'Settings', properties: {} },
        views: [{ id: 'view/one two', title: 'Open view', location: 'sidebar' }],
        commands: [{ id: 'run-command', title: 'Run', description: 'Runs the command safely' }],
        menus: [{ id: 'menu-entry', commandId: 'run-command', location: 'terminal.context' }],
        keybindings: [
          { id: 'run-key', commandId: 'run-command', key: 'Ctrl+Alt+9', mac: 'Meta+Alt+9' }
        ],
        statusItems: [
          {
            id: 'status-entry',
            label: 'Status label',
            location: 'statusBar',
            scope: 'activeProject'
          }
        ],
        eventSubscriptions: [{ id: 'event-entry', topic: 'terminal.created', scope: 'window' }]
      },
      services: { provides: [], requires: [] }
    }
  }
}

const baseline = useKeyboardShortcutsStore.getState()
beforeEach(() => useKeyboardShortcutsStore.setState(baseline, true))
afterEach(() => {
  cleanup()
  useKeyboardShortcutsStore.setState(baseline, true)
})
function show(plugin = fixture(), connected = true): ReturnType<typeof render> {
  return render(
    <MemoryRouter>
      <PluginContributions plugin={plugin} connected={connected} />
    </MemoryRouter>
  )
}

function keySection(): HTMLElement {
  return screen.getByRole('region', { name: 'inspector.keybindings' })
}

describe('PluginContributions', () => {
  it('renders each contribution and encoded active view links, without command execution buttons', () => {
    show()
    expect(screen.getByRole('link', { name: 'Open view' }).getAttribute('href')).toBe(
      '/plugins/org.plugin%2Fwith%20space/views/view%2Fone%20two'
    )
    expect(screen.queryAllByRole('button')).toHaveLength(0)
    expect(screen.getByText('Runs the command safely')).toBeTruthy()
    for (const text of [
      'menu-entry',
      'terminal.context',
      'Meta+Alt+9',
      'Status label',
      'statusBar',
      'activeProject',
      'terminal.created',
      'window'
    ]) {
      expect(screen.getAllByText(text).length).toBeGreaterThan(0)
    }
    expect(
      within(screen.getByRole('region', { name: 'inspector.menus' })).getByText('run-command')
    ).toBeTruthy()
  })

  it.each([
    { state: 'disabled' as const, connected: true, enabled: true, generation: 'g1' },
    { state: 'active' as const, connected: false, enabled: true, generation: 'g1' },
    { state: 'active' as const, connected: true, enabled: false, generation: 'g1' },
    { state: 'active' as const, connected: true, enabled: true, generation: null }
  ])('disables views when not executable: %o', ({ state, connected, enabled, generation }) => {
    show({ ...fixture(), state, desiredEnabled: enabled, generation }, connected)
    expect(screen.queryByRole('link')).toBeNull()
    expect((screen.getByRole('button', { name: 'Open view' }) as HTMLButtonElement).disabled).toBe(
      true
    )
  })

  it('reads custom keys and conflicts without mutating the keyboard shortcut store', () => {
    const plugin = fixture()
    const store = useKeyboardShortcutsStore.getState()
    store.registerPluginShortcuts(plugin.id, 'g1', [
      {
        pluginId: plugin.id,
        pluginName: 'Plugin',
        bindingId: 'run-key',
        generation: 'g1',
        label: 'Run',
        defaultKey: 'Meta+Alt+9'
      }
    ])
    const id = pluginShortcutId(plugin.id, 'run-key')
    store.updateShortcut(id, 'Meta+Alt+8')
    const snapshot = useKeyboardShortcutsStore.getState()
    const { unmount } = show(plugin)
    expect(within(keySection()).getAllByText('Meta+Alt+8')).toHaveLength(2)
    expect(within(keySection()).getByText('inspector.customKey:')).toBeTruthy()
    expect(useKeyboardShortcutsStore.getState()).toBe(snapshot)
    unmount()

    // Resolve duplicate and reserved conflicts using the real store.
    store.registerPluginShortcuts('org.first', 'g1', [
      {
        pluginId: 'org.first',
        pluginName: 'First',
        bindingId: 'first-key',
        generation: 'g1',
        label: 'First',
        defaultKey: 'Meta+Alt+7'
      }
    ])
    store.updateShortcut(pluginShortcutId('org.first', 'first-key'), 'Meta+Alt+8')
    const conflicted = show(plugin)
    expect(within(keySection()).getByText('inspector.unbound')).toBeTruthy()
    expect(
      within(keySection()).getByText('inspector.conflictDuplicate:plugin:org.first:first-key')
    ).toBeTruthy()
    conflicted.unmount()
    store.updateShortcut(id, 'cmd+q')
    show(plugin)
    expect(within(keySection()).getByText('inspector.unbound')).toBeTruthy()
    expect(within(keySection()).getByText('inspector.conflictReserved:macQuit')).toBeTruthy()
  })

  it('retains explicit unbinding overrides for unregistered or inactive plugins', () => {
    const plugin = fixture()
    useKeyboardShortcutsStore.setState({
      pluginOverrides: { [pluginShortcutId(plugin.id, 'run-key')]: '' }
    })
    show({ ...plugin, state: 'disabled' })
    expect(within(keySection()).getAllByText('inspector.unbound')).toHaveLength(2)
  })

  it('has an empty state for every section, including absent optional contributions', () => {
    const plugin = fixture()
    plugin.manifest.contributes = {
      commands: [],
      views: [],
      configuration: { title: '', properties: {} }
    }
    show(plugin)
    for (const suffix of [
      'Views',
      'Commands',
      'Menus',
      'Keybindings',
      'StatusItems',
      'EventSubscriptions'
    ]) {
      expect(screen.getByText(`inspector.empty${suffix}`)).toBeTruthy()
    }
    expect(screen.getAllByRole('region')).toHaveLength(6)
  })
})
