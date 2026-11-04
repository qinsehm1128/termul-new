import type { ExecuteContributionRequest, HostWhen } from '@shared/types/plugin-host.types'
import { useCallback, useEffect } from 'react'
import { useAcpStore } from '@/features/agent-session/stores/acp-store'
import { usePluginHostUiStore } from '@/features/plugins/host-ui-store'
import { pluginApi } from '@/features/plugins/plugin-api'
import { persistenceApi } from '@/lib/api'
import {
  type KeybindingScheme,
  loadKeybindingSchemeFiles,
  parseUserScheme,
  type SchemeIssue
} from '@/lib/keybinding-schemes'
import { isMac } from '@/lib/platform'
import { isTauriContext } from '@/lib/tauri-runtime'
import { matchesShortcut, useKeyboardShortcutsStore } from '@/stores/keyboard-shortcuts-store'
import { usePluginsStore } from '@/stores/plugins-store'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { useWorkspaceStore } from '@/stores/workspace-store'
import type { KeyboardShortcutsConfig } from '@/types/settings'
import {
  DEFAULT_KEYBOARD_SHORTCUTS,
  KEYBINDING_SCHEME_KEY,
  KEYBOARD_SHORTCUTS_KEY
} from '@/types/settings'

// Deep clone defaults preserving customKey from loaded data
function mergeWithDefaults(loaded: Partial<KeyboardShortcutsConfig>): KeyboardShortcutsConfig {
  const result: KeyboardShortcutsConfig = {}

  for (const [key, defaultShortcut] of Object.entries(DEFAULT_KEYBOARD_SHORTCUTS)) {
    const loadedShortcut = loaded[key]
    result[key] = {
      ...defaultShortcut,
      customKey: loadedShortcut?.customKey
    }
  }

  for (const [id, shortcut] of Object.entries(loaded)) {
    if (id.startsWith('plugin:') && shortcut && typeof shortcut.customKey === 'string') {
      result[id] = { id, label: '', description: '', defaultKey: '', customKey: shortcut.customKey }
    }
  }
  return result
}

/** Persist intended overrides, not runtime conflict suppression, including inactive plugins. */
function shortcutsForPersistence(): KeyboardShortcutsConfig {
  const { shortcuts, pluginOverrides } = useKeyboardShortcutsStore.getState()
  const result = Object.fromEntries(
    Object.entries(shortcuts).filter(([id]) => !id.startsWith('plugin:'))
  )
  for (const [id, customKey] of Object.entries(pluginOverrides ?? {})) {
    result[id] = { id, label: '', description: '', defaultKey: '', customKey }
  }
  return result
}

/**
 * Read the user scheme files into the store. Only the desktop app has them;
 * the web client keeps the bundled schemes.
 */
export async function reloadUserKeybindingSchemes(): Promise<void> {
  if (!isTauriContext()) return
  const result = await loadKeybindingSchemeFiles(false)
  const store = useKeyboardShortcutsStore.getState()
  if (!result.success) {
    store.setUserSchemes([], [{ file: '', message: result.error }])
    return
  }
  const schemes: KeybindingScheme[] = []
  const issues: SchemeIssue[] = []
  for (const file of result.data.files) {
    const parsed = parseUserScheme(file)
    if (parsed.scheme) schemes.push(parsed.scheme)
    issues.push(...parsed.issues)
  }
  store.setUserSchemes(schemes, issues)
}

export function useKeyboardShortcutsLoader(): void {
  const setShortcuts = useKeyboardShortcutsStore((state) => state.setShortcuts)

  useEffect(() => {
    async function load(): Promise<void> {
      const [result, scheme] = await Promise.all([
        persistenceApi.read<KeyboardShortcutsConfig>(KEYBOARD_SHORTCUTS_KEY),
        persistenceApi.read<string>(KEYBINDING_SCHEME_KEY),
        reloadUserKeybindingSchemes()
      ])
      if (result.success && result.data) {
        // Merge with defaults to handle new shortcuts added in updates
        setShortcuts(mergeWithDefaults(result.data))
      } else {
        // First load or read error - use defaults
        const defaults: KeyboardShortcutsConfig = {}
        for (const [key, shortcut] of Object.entries(DEFAULT_KEYBOARD_SHORTCUTS)) {
          defaults[key] = { ...shortcut }
        }
        setShortcuts(defaults)
      }
      const schemeId = scheme.success && typeof scheme.data === 'string' ? scheme.data : null
      if (schemeId) useKeyboardShortcutsStore.getState().applyScheme(schemeId)
    }
    load()
  }, [setShortcuts])
}

export function useApplyKeybindingScheme(): (schemeId: string) => Promise<void> {
  const applyScheme = useKeyboardShortcutsStore((state) => state.applyScheme)

  return useCallback(
    async (schemeId: string) => {
      applyScheme(schemeId)
      await persistenceApi.write(KEYBINDING_SCHEME_KEY, schemeId)
    },
    [applyScheme]
  )
}

/** Register runnable contributions; dormant overrides remain in the shortcut store. */
export function usePluginKeyboardShortcuts(): void {
  const plugins = usePluginsStore((state) => state.plugins)
  const connected = usePluginsStore((state) => state.connected)
  useEffect(() => {
    if (typeof useKeyboardShortcutsStore.getState !== 'function') return
    const store = useKeyboardShortcutsStore.getState()
    const active = new Map<string, string>()
    if (isTauriContext() && connected) {
      for (const plugin of plugins) {
        if (!plugin.desiredEnabled || plugin.state !== 'active' || !plugin.generation) continue
        active.set(plugin.id, plugin.generation)
        store.registerPluginShortcuts(
          plugin.id,
          plugin.generation,
          (plugin.manifest.contributes.keybindings ?? []).map((binding) => ({
            pluginId: plugin.id,
            pluginName: plugin.manifest.name,
            generation: plugin.generation as string,
            bindingId: binding.id,
            label:
              plugin.manifest.contributes.commands?.find(
                (command) => command.id === binding.commandId
              )?.title ?? binding.commandId,
            defaultKey: isMac ? (binding.mac ?? binding.key) : binding.key,
            when: binding.when
          }))
        )
      }
    }
    for (const binding of Object.values(
      useKeyboardShortcutsStore.getState().pluginShortcuts ?? {}
    )) {
      if (!active.has(binding.pluginId))
        store.unregisterPluginShortcuts(binding.pluginId, binding.generation)
    }
    return () => {
      for (const [pluginId, generation] of active) {
        useKeyboardShortcutsStore.getState().unregisterPluginShortcuts(pluginId, generation)
      }
    }
  }, [plugins, connected])
}

function pluginWhenMatches(
  when: HostWhen | undefined,
  target: ExecuteContributionRequest['target']
): boolean {
  if (!when) return true
  const context: Required<HostWhen> = {
    'project.exists': Boolean(target.projectId),
    'terminal.exists': Boolean(target.terminalId),
    'agentSession.exists': Boolean(target.sessionId),
    'terminal.focused': false,
    'textInput.focused': false
  }
  return Object.entries(when).every(([key, value]) => context[key as keyof HostWhen] === value)
}

/** Called only after all built-in handlers have declined the event. */
export function handlePluginShortcut(event: KeyboardEvent): boolean {
  if (
    typeof useKeyboardShortcutsStore.getState !== 'function' ||
    !isTauriContext() ||
    event.defaultPrevented ||
    event.isComposing ||
    event.repeat
  )
    return false
  const host = usePluginHostUiStore.getState()
  if (!host.connected || host.contextPending || !host.contextRevision || !host.context) return false
  const blocked = (node: EventTarget | null) =>
    node instanceof Element &&
    Boolean(
      node.closest(
        'input, textarea, select, [contenteditable]:not([contenteditable="false"]), [role="textbox"], .xterm, [data-terminal-id], .monaco-editor, .cm-editor, .CodeMirror'
      )
    )
  if (blocked(event.target) || blocked(document.activeElement)) return false
  const lifecycle = usePluginsStore.getState()
  if (!lifecycle.connected) return false
  const tab = useWorkspaceStore.getState().getActiveTab()
  const projectId = useProjectStore.getState().activeProjectId
  const target: ExecuteContributionRequest['target'] = {}
  if (projectId) target.projectId = projectId
  if (tab?.type === 'terminal') {
    const terminal = useTerminalStore
      .getState()
      .terminals.find((item) => item.id === tab.terminalId)
    if (terminal) {
      target.terminalId = terminal.id
      if (terminal.projectId) target.projectId = terminal.projectId
    }
  } else if (tab?.type === 'agent-chat') {
    const session =
      tab.sessionId ??
      Object.values(useAcpStore.getState().sessions).find(
        (item) => item.conversationId === tab.conversationId
      )?.id
    if (session) target.sessionId = session
  }
  const displayedPath = new URL(window.location.hash.slice(1) || '/', window.location.origin)
    .pathname
  if (
    displayedPath !== '/' &&
    displayedPath !== '/terminal' &&
    !/^\/c\/[^/]+\/?$/.test(displayedPath)
  ) {
    delete target.projectId
    delete target.terminalId
    delete target.sessionId
    if (host.context.activeProject) target.projectId = host.context.activeProject.id
    if (host.context.activeTerminalId) target.terminalId = host.context.activeTerminalId
    if (host.context.activeSessionId) target.sessionId = host.context.activeSessionId
  }
  // Raw stores can contain entities omitted from the bounded acknowledged snapshot.
  // Such targets are unavailable, not safe to dispatch under this context revision.
  if (target.projectId && !host.context.projectIds.includes(target.projectId)) return false
  if (
    target.terminalId &&
    !host.context.terminals.some(
      (terminal) =>
        terminal.id === target.terminalId &&
        (!target.projectId || terminal.projectId === target.projectId)
    )
  )
    return false
  if (
    target.sessionId &&
    !host.context.agentSessions.some(
      (session) =>
        session.id === target.sessionId &&
        (!target.projectId || session.projectId === target.projectId)
    )
  )
    return false
  const store = useKeyboardShortcutsStore.getState()
  const contextRevision = host.contextRevision
  if (!contextRevision) return false
  const pluginShortcuts = store.pluginShortcuts ?? {}
  for (const id of Object.keys(pluginShortcuts).sort()) {
    const binding = pluginShortcuts[id]
    const plugin = lifecycle.plugins.find((item) => item.id === binding.pluginId)
    if (
      !plugin?.desiredEnabled ||
      plugin.state !== 'active' ||
      plugin.generation !== binding.generation
    )
      continue
    const shortcut = store.shortcuts[id]
    if (!shortcut || !matchesShortcut(event, shortcut.customKey ?? shortcut.defaultKey)) continue
    if (!pluginWhenMatches(binding.when, target)) continue
    event.preventDefault()
    event.stopPropagation()
    void pluginApi
      .executeContribution({
        pluginId: binding.pluginId,
        generation: binding.generation,
        kind: 'keybinding',
        contributionId: binding.bindingId,
        contextRevision,
        target
      })
      .catch((error: unknown) => console.error('Plugin shortcut failed:', error))
    return true
  }
  return false
}

export function useUpdateShortcut(): (id: string, customKey: string) => Promise<void> {
  const updateShortcut = useKeyboardShortcutsStore((state) => state.updateShortcut)

  return useCallback(
    async (id: string, customKey: string) => {
      updateShortcut(id, customKey)
      // Zustand updates are synchronous, so getState() returns updated state
      const updatedShortcuts = shortcutsForPersistence()
      await persistenceApi.writeDebounced(KEYBOARD_SHORTCUTS_KEY, updatedShortcuts)
    },
    [updateShortcut]
  )
}

export function useResetShortcut(): (id: string) => Promise<void> {
  const resetShortcut = useKeyboardShortcutsStore((state) => state.resetShortcut)

  return useCallback(
    async (id: string) => {
      resetShortcut(id)
      const updatedShortcuts = shortcutsForPersistence()
      await persistenceApi.writeDebounced(KEYBOARD_SHORTCUTS_KEY, updatedShortcuts)
    },
    [resetShortcut]
  )
}

export function useResetAllShortcuts(): () => Promise<void> {
  const resetAllShortcuts = useKeyboardShortcutsStore((state) => state.resetAllShortcuts)

  return useCallback(async () => {
    resetAllShortcuts()
    const updatedShortcuts = shortcutsForPersistence()
    await persistenceApi.write(KEYBOARD_SHORTCUTS_KEY, updatedShortcuts)
  }, [resetAllShortcuts])
}
