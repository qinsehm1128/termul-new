import type { PluginInfo } from '@shared/types/plugin.types'
import { act, renderHook, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { usePluginHostUiStore } from '@/features/plugins/host-ui-store'
import { pluginApi } from '@/features/plugins/plugin-api'
import { persistenceApi } from '@/lib/api'
import { isMac } from '@/lib/platform'
import { useKeyboardShortcutsStore } from '@/stores/keyboard-shortcuts-store'
import { usePluginsStore } from '@/stores/plugins-store'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { useWorkspaceStore } from '@/stores/workspace-store'
import { KEYBINDING_SCHEME_KEY, KEYBOARD_SHORTCUTS_KEY } from '@/types/settings'
import {
  handlePluginShortcut,
  useApplyKeybindingScheme,
  useKeyboardShortcutsLoader,
  usePluginKeyboardShortcuts,
  useUpdateShortcut
} from './use-keyboard-shortcuts'

vi.mock('@/features/plugins/plugin-api', () => ({
  pluginApi: { executeContribution: vi.fn(async () => ({ success: true, data: {} })) }
}))

const mocks = vi.hoisted(() => ({
  read: vi.fn(),
  loadFiles: vi.fn(),
  isTauri: vi.fn(() => true)
}))

vi.mock('@/lib/api', () => ({
  persistenceApi: {
    read: mocks.read,
    write: vi.fn(async () => {}),
    writeDebounced: vi.fn(async () => {})
  }
}))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: mocks.isTauri }))
vi.mock('@/lib/keybinding-schemes', async () => {
  const actual = await vi.importActual<typeof import('@/lib/keybinding-schemes')>(
    '@/lib/keybinding-schemes'
  )
  return { ...actual, loadKeybindingSchemeFiles: mocks.loadFiles }
})

function persisted(values: Record<string, unknown>) {
  mocks.read.mockImplementation(async (key: string) => ({
    success: true,
    data: values[key] ?? null
  }))
}

const plugin: PluginInfo = {
  id: 'org.test.keys',
  desiredEnabled: true,
  state: 'active',
  generation: 'g1',
  configRevision: 1,
  sha256: 'a'.repeat(64),
  lastError: null,
  manifest: {
    schemaVersion: 1,
    id: 'org.test.keys',
    name: 'Test keys',
    version: '1.0.0',
    abi: { min: 1, max: 1 },
    services: { provides: [], requires: [] },
    contributes: {
      commands: [{ id: 'run', title: 'Run command' }],
      views: [],
      configuration: { title: '', properties: {} },
      keybindings: [
        {
          id: 'run-key',
          commandId: 'run',
          key: 'ctrl+alt+shift+y',
          when: { 'project.exists': true }
        }
      ]
    }
  }
}

const id = 'plugin:org.test.keys:run-key'

describe('plugin keyboard contributions', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    window.history.replaceState(null, '', '#/')
    useKeyboardShortcutsStore.setState({
      pluginShortcuts: {},
      pluginOverrides: {},
      pluginConflicts: {}
    })
    useKeyboardShortcutsStore.getState().resetAllShortcuts()
    usePluginsStore.setState({ connected: true, plugins: [plugin] })
    usePluginHostUiStore.setState({
      contextRevision: 7,
      connected: true,
      contextPending: false,
      context: {
        activeProject: { id: 'project-1', name: 'Project', path: '/project' },
        projectIds: ['project-1', 'project-2'],
        activeTerminalId: null,
        activeSessionId: null,
        terminals: [{ id: 'terminal-1', ptyId: null, projectId: 'project-2' }],
        agentSessions: [
          {
            id: 'session-1',
            projectId: 'project-1',
            status: 'idle',
            activeTurn: false,
            openTurnId: null,
            replaying: false
          }
        ],
        focus: { textInput: false, terminal: false },
        truncated: false
      }
    })
    mocks.isTauri.mockReturnValue(true)
    useProjectStore.setState({ activeProjectId: 'project-1' })
    mocks.loadFiles.mockResolvedValue({ success: true, data: { dir: '/k', files: [] } })
    useWorkspaceStore.setState({
      root: { type: 'leaf', id: 'pane-1', tabs: [], activeTabId: '' },
      activePaneId: 'pane-1'
    })
    document.body.innerHTML = ''
    persisted({})
  })
  it('registers only connected active desired-enabled generations and removes stale bindings', () => {
    const hook = renderHook(() => usePluginKeyboardShortcuts())
    expect(useKeyboardShortcutsStore.getState().pluginShortcuts[id]).toMatchObject({
      generation: 'g1',
      label: 'Run command'
    })
    act(() => usePluginsStore.setState({ plugins: [{ ...plugin, generation: 'g2' }] }))
    expect(useKeyboardShortcutsStore.getState().pluginShortcuts[id].generation).toBe('g2')
    act(() => usePluginsStore.setState({ plugins: [{ ...plugin, desiredEnabled: false }] }))
    expect(useKeyboardShortcutsStore.getState().pluginShortcuts).toEqual({})
    act(() => usePluginsStore.setState({ plugins: [plugin], connected: false }))
    expect(useKeyboardShortcutsStore.getState().pluginShortcuts).toEqual({})
    act(() =>
      usePluginsStore.setState({ connected: true, plugins: [{ ...plugin, state: 'disabled' }] })
    )
    expect(useKeyboardShortcutsStore.getState().pluginShortcuts).toEqual({})
    hook.unmount()
  })
  it('treats missing legacy plugin maps as empty during mount, dispatch and persistence', async () => {
    usePluginsStore.setState({ connected: false, plugins: [] })
    useKeyboardShortcutsStore.setState({
      pluginShortcuts: undefined,
      pluginOverrides: undefined,
      pluginConflicts: undefined
    } as never)
    const { unmount } = renderHook(() => usePluginKeyboardShortcuts())
    expect(
      handlePluginShortcut(
        new KeyboardEvent('keydown', { key: 'j', ctrlKey: true, shiftKey: true })
      )
    ).toBe(false)
    const update = renderHook(() => useUpdateShortcut())
    await act(async () => {
      await update.result.current('sidebarToggle', 'ctrl+alt+y')
    })
    expect(persistenceApi.writeDebounced).toHaveBeenCalledWith(
      KEYBOARD_SHORTCUTS_KEY,
      expect.objectContaining({
        sidebarToggle: expect.objectContaining({ customKey: 'ctrl+alt+y' })
      })
    )
    update.unmount()
    unmount()
  })

  it('retains loaded dormant overrides through builtin edits, scheme switches and later registration', async () => {
    persisted({ [KEYBOARD_SHORTCUTS_KEY]: { [id]: { customKey: 'ctrl+alt+shift+u' } } })
    const hook = renderHook(() => {
      useKeyboardShortcutsLoader()
      return { update: useUpdateShortcut(), scheme: useApplyKeybindingScheme() }
    })
    await waitFor(() =>
      expect(useKeyboardShortcutsStore.getState().pluginOverrides[id]).toBe('ctrl+alt+shift+u')
    )
    await act(() => hook.result.current.update('sidebarToggle', 'ctrl+alt+shift+o'))
    expect(vi.mocked(persistenceApi.writeDebounced).mock.calls.at(-1)?.[1]).toMatchObject({
      [id]: { customKey: 'ctrl+alt+shift+u' }
    })
    await act(() => hook.result.current.scheme('iterm2'))
    const sync = renderHook(() => usePluginKeyboardShortcuts())
    expect(useKeyboardShortcutsStore.getState().shortcuts[id].customKey).toBe('ctrl+alt+shift+u')
    sync.unmount()
  })
  const keyEvent = (init: KeyboardEventInit = {}) =>
    new KeyboardEvent('keydown', {
      key: 'y',
      altKey: true,
      shiftKey: true,
      ...(isMac ? { metaKey: true } : { ctrlKey: true }),
      cancelable: true,
      ...init
    })
  it('uses the acknowledged visible quick workspace instead of hidden primary selection', () => {
    window.history.replaceState(null, '', '#/quick-terminals/quick-1')
    useWorkspaceStore.setState({
      root: {
        type: 'leaf',
        id: 'pane-hidden',
        activeTabId: 'hidden-tab',
        tabs: [
          { id: 'hidden-tab', type: 'terminal', terminalId: 'hidden-terminal', title: 'Hidden' }
        ]
      },
      activePaneId: 'pane-hidden'
    })
    useTerminalStore.setState({
      terminals: [{ id: 'hidden-terminal', ptyId: 'hidden-pty', projectId: 'project-1' }]
    } as never)
    const context = usePluginHostUiStore.getState().context!
    usePluginHostUiStore.setState({
      context: {
        ...context,
        activeTerminalId: 'quick-terminal',
        activeSessionId: null,
        terminals: [{ id: 'quick-terminal', ptyId: 'quick-pty', projectId: 'project-1' }]
      }
    })
    renderHook(() => usePluginKeyboardShortcuts())
    expect(handlePluginShortcut(keyEvent())).toBe(true)
    expect(pluginApi.executeContribution).toHaveBeenCalledWith(
      expect.objectContaining({ target: { projectId: 'project-1', terminalId: 'quick-terminal' } })
    )
    window.history.replaceState(null, '', '#/')
  })

  it('invokes only a contribution id with current generation, target and context revision', () => {
    renderHook(() => usePluginKeyboardShortcuts())
    const event = keyEvent()
    expect(handlePluginShortcut(event)).toBe(true)
    expect(event.defaultPrevented).toBe(true)
    expect(pluginApi.executeContribution).toHaveBeenCalledWith({
      pluginId: plugin.id,
      generation: 'g1',
      kind: 'keybinding',
      contributionId: 'run-key',
      target: { projectId: 'project-1' },
      contextRevision: 7
    })
    act(() =>
      usePluginsStore.setState({ plugins: [{ ...plugin, generation: 'g2', state: 'disabled' }] })
    )
    expect(handlePluginShortcut(keyEvent())).toBe(false)
  })
  it('activates an initially unbound plugin shortcut through the existing update and mac cmd alias path', async () => {
    usePluginsStore.setState({
      plugins: [
        {
          ...plugin,
          manifest: {
            ...plugin.manifest,
            contributes: {
              ...plugin.manifest.contributes,
              keybindings: [{ id: 'run-key', commandId: 'run', key: '' }]
            }
          }
        }
      ]
    })
    const hook = renderHook(() => {
      usePluginKeyboardShortcuts()
      return useUpdateShortcut()
    })
    expect(handlePluginShortcut(keyEvent())).toBe(false)
    await act(() => hook.result.current(id, 'ctrl+alt+shift+y'))
    expect(handlePluginShortcut(keyEvent())).toBe(true)
    expect(vi.mocked(persistenceApi.writeDebounced).mock.calls.at(-1)?.[1]).toMatchObject({
      [id]: { customKey: 'ctrl+alt+shift+y' }
    })
  })
  it('uses the current terminal record and agent-chat session as contribution targets', () => {
    renderHook(() => usePluginKeyboardShortcuts())
    useTerminalStore.setState({
      terminals: [{ id: 'terminal-1', name: 'Shell', shell: '/bin/zsh', projectId: 'project-2' }]
    })
    act(() =>
      useWorkspaceStore.setState({
        root: {
          type: 'leaf',
          id: 'pane-1',
          tabs: [{ id: 'tab-1', type: 'terminal', title: 'Shell', terminalId: 'terminal-1' }],
          activeTabId: 'tab-1'
        }
      })
    )
    expect(handlePluginShortcut(keyEvent())).toBe(true)
    expect(vi.mocked(pluginApi.executeContribution).mock.calls.at(-1)?.[0].target).toEqual({
      projectId: 'project-2',
      terminalId: 'terminal-1'
    })
    act(() =>
      useWorkspaceStore.setState({
        root: {
          type: 'leaf',
          id: 'pane-1',
          tabs: [
            {
              id: 'chat-1',
              type: 'agent-chat',
              title: 'Chat',
              conversationId: 'conversation-1',
              sessionId: 'session-1'
            }
          ],
          activeTabId: 'chat-1'
        }
      })
    )
    expect(handlePluginShortcut(keyEvent())).toBe(true)
    expect(vi.mocked(pluginApi.executeContribution).mock.calls.at(-1)?.[0].target).toEqual({
      projectId: 'project-1',
      sessionId: 'session-1'
    })
  })
  it('does not consume targets omitted from the acknowledged bounded context', () => {
    renderHook(() => usePluginKeyboardShortcuts())
    const context = usePluginHostUiStore.getState().context!
    const declined = () => {
      const event = keyEvent()
      expect(handlePluginShortcut(event)).toBe(false)
      expect(event.defaultPrevented).toBe(false)
    }
    usePluginHostUiStore.setState({ context: null })
    declined()
    usePluginHostUiStore.setState({
      context: { ...context, projectIds: [], truncated: true },
      contextPending: false
    })
    declined()
    usePluginHostUiStore.setState({ context: { ...context, terminals: [], truncated: true } })
    useTerminalStore.setState({
      terminals: [{ id: 'terminal-1', name: 'Shell', shell: '/bin/zsh', projectId: 'project-2' }]
    })
    useWorkspaceStore.setState({
      root: {
        type: 'leaf',
        id: 'pane-1',
        tabs: [{ id: 'tab-1', type: 'terminal', title: 'Shell', terminalId: 'terminal-1' }],
        activeTabId: 'tab-1'
      }
    })
    declined()
    usePluginHostUiStore.setState({
      context: {
        ...context,
        terminals: [{ id: 'terminal-1', ptyId: null, projectId: 'project-1' }]
      }
    })
    declined()
    useWorkspaceStore.setState({
      root: {
        type: 'leaf',
        id: 'pane-1',
        tabs: [
          {
            id: 'chat-1',
            type: 'agent-chat',
            title: 'Chat',
            conversationId: 'conversation-1',
            sessionId: 'session-1'
          }
        ],
        activeTabId: 'chat-1'
      }
    })
    usePluginHostUiStore.setState({ context: { ...context, agentSessions: [], truncated: true } })
    declined()
    usePluginHostUiStore.setState({
      context: {
        ...context,
        agentSessions: context.agentSessions.map((session) => ({
          ...session,
          projectId: 'project-2'
        }))
      }
    })
    declined()
    expect(pluginApi.executeContribution).not.toHaveBeenCalled()
  })
  it('does not consume prevented, composing, repeated, editable or terminal-focused events', () => {
    renderHook(() => usePluginKeyboardShortcuts())
    const prevented = keyEvent()
    prevented.preventDefault()
    expect(handlePluginShortcut(prevented)).toBe(false)
    expect(handlePluginShortcut(keyEvent({ isComposing: true }))).toBe(false)
    expect(handlePluginShortcut(keyEvent({ repeat: true }))).toBe(false)
    for (const html of [
      '<input/>',
      '<textarea/>',
      '<div contenteditable="true" tabindex="0"></div>',
      '<div class="xterm" tabindex="0"></div>',
      '<div class="monaco-editor" tabindex="0"></div>',
      '<div class="cm-editor" tabindex="0"></div>'
    ]) {
      document.body.innerHTML = html
      const element = document.body.firstElementChild as HTMLElement
      element.focus()
      const event = keyEvent()
      Object.defineProperty(event, 'target', { value: element })
      expect(handlePluginShortcut(event)).toBe(false)
      expect(event.defaultPrevented).toBe(false)
    }
    expect(pluginApi.executeContribution).not.toHaveBeenCalled()
  })
  it('registers and executes nothing in a browser', () => {
    mocks.isTauri.mockReturnValue(false)
    renderHook(() => usePluginKeyboardShortcuts())
    expect(useKeyboardShortcutsStore.getState().pluginShortcuts).toEqual({})
    expect(handlePluginShortcut(keyEvent())).toBe(false)
    expect(pluginApi.executeContribution).not.toHaveBeenCalled()
  })
  it('does not consume keys while context publication is pending', () => {
    renderHook(() => usePluginKeyboardShortcuts())
    usePluginHostUiStore.setState({ contextPending: true })
    const event = keyEvent()
    expect(handlePluginShortcut(event)).toBe(false)
    expect(event.defaultPrevented).toBe(false)
    expect(pluginApi.executeContribution).not.toHaveBeenCalled()
  })
  it('fails closed on missing context, false when, or disconnected host', () => {
    renderHook(() => usePluginKeyboardShortcuts())
    usePluginHostUiStore.setState({ contextRevision: 0 })
    expect(handlePluginShortcut(keyEvent())).toBe(false)
    usePluginHostUiStore.setState({ contextRevision: 8 })
    useProjectStore.setState({ activeProjectId: null })
    expect(handlePluginShortcut(keyEvent())).toBe(false)
    usePluginsStore.setState({ connected: false })
    expect(handlePluginShortcut(keyEvent())).toBe(false)
    expect(pluginApi.executeContribution).not.toHaveBeenCalled()
  })
})

describe('useKeyboardShortcutsLoader', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.isTauri.mockReturnValue(true)
    const store = useKeyboardShortcutsStore.getState()
    store.setUserSchemes([], [])
    store.applyScheme('se-default')
    store.resetAllShortcuts()
    mocks.loadFiles.mockResolvedValue({ success: true, data: { dir: '/k', files: [] } })
  })

  it('restores the saved scheme on top of the saved custom keys', async () => {
    persisted({
      [KEYBOARD_SHORTCUTS_KEY]: { sidebarToggle: { customKey: 'cmd+shift+s' } },
      [KEYBINDING_SCHEME_KEY]: 'iterm2'
    })
    renderHook(() => useKeyboardShortcutsLoader())

    await waitFor(() => expect(useKeyboardShortcutsStore.getState().schemeId).toBe('iterm2'))
    const { shortcuts } = useKeyboardShortcutsStore.getState()
    expect(shortcuts.sidebarToggle.customKey).toBe('cmd+shift+s')
  })

  it('can restore a user scheme because the files load first', async () => {
    mocks.loadFiles.mockResolvedValue({
      success: true,
      data: {
        dir: '/k',
        files: [{ name: 'mine.json', content: '{"bindings":{"splitRight":"cmd+e"}}' }]
      }
    })
    persisted({ [KEYBINDING_SCHEME_KEY]: 'user:mine' })
    renderHook(() => useKeyboardShortcutsLoader())

    await waitFor(() => expect(useKeyboardShortcutsStore.getState().schemeId).toBe('user:mine'))
    expect(useKeyboardShortcutsStore.getState().shortcuts.splitRight.defaultKey).toBe('cmd+e')
  })

  it('reports an unreadable scheme folder', async () => {
    mocks.loadFiles.mockResolvedValue({ success: false, error: 'EACCES', code: 'X' })
    persisted({})
    renderHook(() => useKeyboardShortcutsLoader())

    await waitFor(() =>
      expect(useKeyboardShortcutsStore.getState().schemeIssues).toEqual([
        { file: '', message: 'EACCES' }
      ])
    )
  })

  it('does not touch scheme files in the web client', async () => {
    mocks.isTauri.mockReturnValue(false)
    persisted({})
    renderHook(() => useKeyboardShortcutsLoader())

    // The scheme files would be requested in the same tick as the reads.
    await waitFor(() => expect(mocks.read).toHaveBeenCalledTimes(2))
    expect(mocks.loadFiles).not.toHaveBeenCalled()
  })
})
