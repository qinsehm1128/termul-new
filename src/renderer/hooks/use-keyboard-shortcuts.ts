import { useCallback, useEffect } from 'react'
import { persistenceApi } from '@/lib/api'
import {
  type KeybindingScheme,
  loadKeybindingSchemeFiles,
  parseUserScheme,
  type SchemeIssue
} from '@/lib/keybinding-schemes'
import { isTauriContext } from '@/lib/tauri-runtime'
import { useKeyboardShortcutsStore } from '@/stores/keyboard-shortcuts-store'
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
      await persistenceApi.write(
        KEYBINDING_SCHEME_KEY,
        useKeyboardShortcutsStore.getState().schemeId
      )
    },
    [applyScheme]
  )
}

export function useUpdateShortcut(): (id: string, customKey: string) => Promise<void> {
  const updateShortcut = useKeyboardShortcutsStore((state) => state.updateShortcut)

  return useCallback(
    async (id: string, customKey: string) => {
      updateShortcut(id, customKey)
      // Zustand updates are synchronous, so getState() returns updated state
      const updatedShortcuts = useKeyboardShortcutsStore.getState().shortcuts
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
      const updatedShortcuts = useKeyboardShortcutsStore.getState().shortcuts
      await persistenceApi.writeDebounced(KEYBOARD_SHORTCUTS_KEY, updatedShortcuts)
    },
    [resetShortcut]
  )
}

export function useResetAllShortcuts(): () => Promise<void> {
  const resetAllShortcuts = useKeyboardShortcutsStore((state) => state.resetAllShortcuts)

  return useCallback(async () => {
    resetAllShortcuts()
    const updatedShortcuts = useKeyboardShortcutsStore.getState().shortcuts
    await persistenceApi.write(KEYBOARD_SHORTCUTS_KEY, updatedShortcuts)
  }, [resetAllShortcuts])
}
