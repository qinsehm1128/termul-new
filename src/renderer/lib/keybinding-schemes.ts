/**
 * Keybinding schemes: named sets of shortcut overrides layered over the
 * built-in defaults. The effective key of an action is
 *
 *   user custom key  ??  scheme binding  ??  built-in default
 *
 * so switching schemes never discards a key the user recorded themselves.
 *
 * A scheme file (bundled or `~/.se-manager/keybindings/*.json`) looks like:
 *
 *   {
 *     "name": "My Keys",
 *     "extends": "iterm2",
 *     "bindings": {
 *       "splitRight": "cmd+d",
 *       "commandPalette": null,
 *       "clearTerminal": { "mac": "cmd+k", "other": "ctrl+shift+k" }
 *     }
 *   }
 *
 * `null` unbinds an action; a `{ mac, other }` object binds per platform.
 */
import type { IpcResult } from '@shared/types/ipc.types'
import { invoke } from '@tauri-apps/api/core'
import { isMac } from '@/lib/platform'
import type { KeyboardShortcutsConfig } from '@/types/settings'
import { DEFAULT_KEYBOARD_SHORTCUTS } from '@/types/settings'

export type SchemeBinding = string | null | { mac?: string | null; other?: string | null }

export interface KeybindingScheme {
  id: string
  name: string
  builtin: boolean
  extends?: string
  bindings: Record<string, SchemeBinding>
}

export const DEFAULT_SCHEME_ID = 'se-default'

// Both terminals share their split and pane keys on macOS (verified against
// iTerm2's MainMenu.xib and Ghostty's Config.zig). ⌘K clears the terminal
// there, so the command palette keeps only its ⌘⇧P binding.
const TERMINAL_STYLE_MAC_BINDINGS: Record<string, SchemeBinding> = {
  commandPalette: { mac: null },
  clearTerminal: { mac: 'cmd+k' },
  nextTerminal: { mac: 'cmd+shift+]' },
  prevTerminal: { mac: 'cmd+shift+[' }
}

export const BUILTIN_KEYBINDING_SCHEMES: readonly KeybindingScheme[] = [
  { id: DEFAULT_SCHEME_ID, name: 'Se', builtin: true, bindings: {} },
  {
    id: 'iterm2',
    name: 'iTerm2',
    builtin: true,
    bindings: { ...TERMINAL_STYLE_MAC_BINDINGS }
  },
  {
    id: 'ghostty',
    name: 'Ghostty',
    builtin: true,
    bindings: {
      ...TERMINAL_STYLE_MAC_BINDINGS,
      // Ghostty's Linux/Windows defaults.
      splitRight: { mac: 'cmd+d', other: 'ctrl+shift+o' },
      splitDown: { mac: 'cmd+shift+d', other: 'ctrl+shift+e' },
      focusPaneLeft: { mac: 'cmd+alt+arrowleft', other: 'ctrl+alt+arrowleft' },
      focusPaneRight: { mac: 'cmd+alt+arrowright', other: 'ctrl+alt+arrowright' },
      focusPaneUp: { mac: 'cmd+alt+arrowup', other: 'ctrl+alt+arrowup' },
      focusPaneDown: { mac: 'cmd+alt+arrowdown', other: 'ctrl+alt+arrowdown' },
      togglePaneZoom: { mac: 'cmd+shift+enter', other: 'ctrl+shift+enter' }
    }
  }
]

const MODIFIERS = new Set(['ctrl', 'cmd', 'shift', 'alt'])

/** A normalized accelerator: known modifiers, then exactly one key. */
export function isValidAccelerator(value: string): boolean {
  const parts = value.split('+')
  const key = parts.pop()
  if (!key || MODIFIERS.has(key)) return false
  return parts.every((part) => MODIFIERS.has(part)) && new Set(parts).size === parts.length
}

function resolveBinding(binding: SchemeBinding, mac: boolean): string | null | undefined {
  if (binding === null || typeof binding === 'string') return binding
  return mac ? binding.mac : binding.other
}

/**
 * The flat action → key map a scheme declares for this platform, following one
 * `extends` chain. `''` means unbound; an action the chain never names is
 * absent and keeps its built-in default.
 */
export function resolveSchemeBindings(
  schemeId: string,
  schemes: readonly KeybindingScheme[],
  mac: boolean = isMac
): Record<string, string> {
  const chain: KeybindingScheme[] = []
  let next: string | undefined = schemeId
  while (next && !chain.some((scheme) => scheme.id === next)) {
    const scheme = schemes.find((candidate) => candidate.id === next)
    if (!scheme) break
    chain.unshift(scheme)
    next = scheme.extends
  }

  const resolved: Record<string, string> = {}
  for (const scheme of chain) {
    for (const [actionId, binding] of Object.entries(scheme.bindings)) {
      const key = resolveBinding(binding, mac)
      if (key !== undefined) resolved[actionId] = key ?? ''
    }
  }
  return resolved
}

/**
 * Build the shortcut table for a scheme. `defaultKey` becomes the scheme's
 * key, so "reset" returns to the scheme and every consumer that already reads
 * `customKey ?? defaultKey` follows the scheme without change.
 */
export function buildSchemeShortcuts(
  bindings: Record<string, string>,
  customKeys: Record<string, string | undefined>
): KeyboardShortcutsConfig {
  const result: KeyboardShortcutsConfig = {}
  for (const [id, shortcut] of Object.entries(DEFAULT_KEYBOARD_SHORTCUTS)) {
    // A custom key is kept even when it equals this scheme's key: it is the
    // user's choice and must survive switching to a scheme that differs.
    result[id] = {
      ...shortcut,
      defaultKey: bindings[id] ?? shortcut.defaultKey,
      customKey: customKeys[id]
    }
  }
  return result
}

export interface SchemeFile {
  name: string
  content?: string
  error?: string
}

export interface SchemeIssue {
  file: string
  message: string
}

function parseBinding(value: unknown): SchemeBinding | undefined {
  if (value === null) return null
  if (typeof value === 'string') return isValidAccelerator(value) ? value : undefined
  if (typeof value !== 'object' || Array.isArray(value)) return undefined
  const platform: { mac?: string | null; other?: string | null } = {}
  for (const [key, entry] of Object.entries(value as Record<string, unknown>)) {
    if (key !== 'mac' && key !== 'other') return undefined
    if (entry !== null && (typeof entry !== 'string' || !isValidAccelerator(entry))) {
      return undefined
    }
    platform[key] = entry as string | null
  }
  return platform
}

/**
 * Parse one user scheme file. Bad entries are skipped and reported so one
 * typo does not throw away the rest of the file.
 */
export function parseUserScheme(file: SchemeFile): {
  scheme?: KeybindingScheme
  issues: SchemeIssue[]
} {
  const stem = file.name.replace(/\.json$/i, '')
  const issue = (message: string): SchemeIssue => ({ file: file.name, message })
  if (file.error !== undefined || file.content === undefined) {
    return { issues: [issue(file.error ?? 'unreadable')] }
  }

  let raw: unknown
  try {
    raw = JSON.parse(file.content)
  } catch (error) {
    return { issues: [issue(error instanceof Error ? error.message : 'invalid JSON')] }
  }
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    return { issues: [issue('scheme must be a JSON object')] }
  }

  const candidate = raw as { name?: unknown; extends?: unknown; bindings?: unknown }
  const issues: SchemeIssue[] = []
  const bindings: Record<string, SchemeBinding> = {}
  const rawBindings = candidate.bindings
  if (typeof rawBindings !== 'object' || rawBindings === null || Array.isArray(rawBindings)) {
    return { issues: [issue('"bindings" must be an object')] }
  }
  for (const [actionId, value] of Object.entries(rawBindings as Record<string, unknown>)) {
    if (!(actionId in DEFAULT_KEYBOARD_SHORTCUTS)) {
      issues.push(issue(`unknown action "${actionId}"`))
      continue
    }
    const binding = parseBinding(value)
    if (binding === undefined) {
      issues.push(issue(`invalid key for "${actionId}"`))
      continue
    }
    bindings[actionId] = binding
  }

  return {
    scheme: {
      id: `user:${stem}`,
      name:
        typeof candidate.name === 'string' && candidate.name.trim() ? candidate.name.trim() : stem,
      builtin: false,
      extends: typeof candidate.extends === 'string' ? candidate.extends : undefined,
      bindings
    },
    issues
  }
}

export interface SchemeDirListing {
  dir: string
  files: SchemeFile[]
}

/** List `~/.se-manager/keybindings/*.json` (desktop only). */
export function loadKeybindingSchemeFiles(
  ensureDir: boolean
): Promise<IpcResult<SchemeDirListing>> {
  return invoke<IpcResult<SchemeDirListing>>('keybinding_schemes_load', { ensureDir })
}
