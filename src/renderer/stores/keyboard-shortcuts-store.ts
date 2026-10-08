import { create } from 'zustand'
import {
  BUILTIN_KEYBINDING_SCHEMES,
  buildSchemeShortcuts,
  DEFAULT_SCHEME_ID,
  type KeybindingScheme,
  resolveSchemeBindings,
  type SchemeIssue
} from '@/lib/keybinding-schemes'
import { isMac } from '@/lib/platform'
import type { KeyboardShortcut, KeyboardShortcutsConfig, ShortcutScope } from '@/types/settings'
import { DEFAULT_KEYBOARD_SHORTCUTS } from '@/types/settings'

interface KeyboardShortcutsState {
  shortcuts: KeyboardShortcutsConfig
  isLoaded: boolean
  schemeId: string
  schemes: KeybindingScheme[]
  schemeIssues: SchemeIssue[]
  setShortcuts: (shortcuts: KeyboardShortcutsConfig) => void
  updateShortcut: (id: string, customKey: string) => void
  resetShortcut: (id: string) => void
  resetAllShortcuts: () => void
  /** Replace the user schemes, keeping the bundled ones first. */
  setUserSchemes: (schemes: KeybindingScheme[], issues: SchemeIssue[]) => void
  /** Switch scheme; recorded custom keys are kept. Unknown ids fall back to Se. */
  applyScheme: (schemeId: string) => void
}

// Deep clone defaults to avoid mutation
function cloneDefaults(): KeyboardShortcutsConfig {
  const result: KeyboardShortcutsConfig = {}
  for (const [key, shortcut] of Object.entries(DEFAULT_KEYBOARD_SHORTCUTS)) {
    result[key] = { ...shortcut }
  }
  return result
}

function customKeysOf(shortcuts: KeyboardShortcutsConfig): Record<string, string | undefined> {
  return Object.fromEntries(Object.entries(shortcuts).map(([id, entry]) => [id, entry.customKey]))
}

export const useKeyboardShortcutsStore = create<KeyboardShortcutsState>((set) => ({
  shortcuts: cloneDefaults(),
  isLoaded: false,
  schemeId: DEFAULT_SCHEME_ID,
  schemes: [...BUILTIN_KEYBINDING_SCHEMES],
  schemeIssues: [],

  setUserSchemes: (userSchemes, issues) =>
    set({ schemes: [...BUILTIN_KEYBINDING_SCHEMES, ...userSchemes], schemeIssues: issues }),

  applyScheme: (requestedId) =>
    set((state) => {
      const schemeId = state.schemes.some((scheme) => scheme.id === requestedId)
        ? requestedId
        : DEFAULT_SCHEME_ID
      return {
        schemeId,
        shortcuts: buildSchemeShortcuts(
          resolveSchemeBindings(schemeId, state.schemes),
          customKeysOf(state.shortcuts)
        )
      }
    }),

  setShortcuts: (shortcuts) => set({ shortcuts, isLoaded: true }),

  updateShortcut: (id, customKey) =>
    set((state) => {
      const shortcut = state.shortcuts[id]
      if (!shortcut) return state

      return {
        shortcuts: {
          ...state.shortcuts,
          [id]: {
            ...shortcut,
            customKey: shortcutsEqual(customKey, shortcut.defaultKey) ? undefined : customKey
          }
        }
      }
    }),

  resetShortcut: (id) =>
    set((state) => {
      const shortcut = state.shortcuts[id]
      if (!shortcut) return state

      return {
        shortcuts: {
          ...state.shortcuts,
          [id]: {
            ...shortcut,
            customKey: undefined
          }
        }
      }
    }),

  resetAllShortcuts: () =>
    set((state) => ({
      shortcuts: buildSchemeShortcuts(resolveSchemeBindings(state.schemeId, state.schemes), {})
    }))
}))

// Helper: Check if a key combination conflicts with any other shortcut that
// can fire in the same place (see `scopesOverlap`).
export function findConflictingShortcut(
  shortcuts: KeyboardShortcutsConfig,
  key: string,
  excludeId: string
): KeyboardShortcut | undefined {
  if (!key) return undefined
  const scope = shortcuts[excludeId]?.scope
  const target = conflictKey(key)
  for (const shortcut of Object.values(shortcuts)) {
    if (shortcut.id === excludeId || !scopesOverlap(scope, shortcut.scope)) continue
    const activeKey = shortcut.customKey ?? shortcut.defaultKey
    if (activeKey && conflictKey(activeKey) === target) {
      return shortcut
    }
  }
  return undefined
}

/** Keys Se handles outside the configurable table. */
export type ReservedShortcutId =
  | 'projectSwitch'
  | 'systemEdit'
  | 'macQuit'
  | 'macHide'
  | 'macMinimize'

const RESERVED_SHORTCUTS: readonly { id: ReservedShortcutId; keys: string[]; macOnly?: boolean }[] =
  [
    // ⌘1–9 / Ctrl+1–9 switch projects (WorkspaceLayout) and stay fixed.
    {
      id: 'projectSwitch',
      keys: ['1', '2', '3', '4', '5', '6', '7', '8', '9'].map((digit) => `ctrl+${digit}`)
    },
    // The native Edit menu owns these; macOS never delivers ⌘V to the webview.
    {
      id: 'systemEdit',
      keys: ['ctrl+c', 'ctrl+v', 'ctrl+x', 'ctrl+a', 'ctrl+z', 'ctrl+shift+z']
    },
    { id: 'macQuit', keys: ['cmd+q'], macOnly: true },
    { id: 'macHide', keys: ['cmd+h', 'cmd+alt+h'], macOnly: true },
    { id: 'macMinimize', keys: ['cmd+m'], macOnly: true }
  ]

export function findReservedShortcut(key: string): ReservedShortcutId | undefined {
  if (!key) return undefined
  const target = conflictKey(key)
  return RESERVED_SHORTCUTS.find(
    (reserved) =>
      (!reserved.macOnly || isMac) && reserved.keys.some((item) => conflictKey(item) === target)
  )?.id
}

export type ShortcutConflict =
  | { kind: 'duplicate'; otherId: string }
  | { kind: 'reserved'; reservedId: ReservedShortcutId }

/** Every conflict of every bound shortcut, keyed by shortcut id. */
export function detectShortcutConflicts(
  shortcuts: KeyboardShortcutsConfig
): Record<string, ShortcutConflict[]> {
  const result: Record<string, ShortcutConflict[]> = {}
  const entries = Object.values(shortcuts)
  for (const shortcut of entries) {
    const key = shortcut.customKey ?? shortcut.defaultKey
    const conflicts: ShortcutConflict[] = []
    const reservedId = findReservedShortcut(key)
    if (reservedId) conflicts.push({ kind: 'reserved', reservedId })
    const target = conflictKey(key)
    for (const other of entries) {
      if (other.id === shortcut.id || !scopesOverlap(shortcut.scope, other.scope)) continue
      const otherKey = other.customKey ?? other.defaultKey
      if (otherKey && conflictKey(otherKey) === target) {
        conflicts.push({ kind: 'duplicate', otherId: other.id })
      }
    }
    if (conflicts.length > 0) result[shortcut.id] = conflicts
  }
  return result
}

// Two shortcuts can collide when they listen in the same place: a global one
// fires everywhere, a scoped one only while its surface has focus.
function scopesOverlap(left: ShortcutScope | undefined, right: ShortcutScope | undefined): boolean {
  const a = left ?? 'global'
  const b = right ?? 'global'
  return a === b || a === 'global' || b === 'global'
}

// The physical press a key stands for. On macOS a lone ctrl modifier fires
// on ⌘ (see matchesShortcut), so 'ctrl+k' and 'cmd+k' are the same press.
function conflictKey(key: string): string {
  const canonical = canonicalizeShortcutKey(key)
  if (!isMac) return canonical
  const parts = canonical.split('+')
  if (!parts.includes('ctrl') || parts.includes('cmd')) return canonical
  return canonicalizeShortcutKey(parts.map((part) => (part === 'ctrl' ? 'cmd' : part)).join('+'))
}

const MODIFIER_ORDER = ['ctrl', 'cmd', 'shift', 'alt'] as const

function canonicalizeShortcutKey(key: string): string {
  const parts = key.split('+').filter(Boolean)
  const shortcutKey = parts[parts.length - 1]
  if (!shortcutKey) return key

  const modifiers = new Set(parts.slice(0, -1))
  const orderedModifiers = MODIFIER_ORDER.filter((modifier) => modifiers.has(modifier))
  return [...orderedModifiers, shortcutKey].join('+')
}

function shortcutsEqual(left: string, right: string): boolean {
  return canonicalizeShortcutKey(left) === canonicalizeShortcutKey(right)
}

// Physical keys whose character changes under ⇧ ('[' → '{', '1' → '!').
const PUNCTUATION_CODES: Record<string, string> = {
  BracketLeft: '[',
  BracketRight: ']',
  Minus: '-',
  Equal: '=',
  Comma: ',',
  Period: '.',
  Slash: '/',
  Semicolon: ';',
  Quote: "'",
  Backquote: '`',
  Backslash: '\\'
}

function keyFromCode(code: string): string | undefined {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3).toLowerCase()
  if (/^Digit[0-9]$/.test(code)) return code.slice(5)
  return PUNCTUATION_CODES[code]
}

// The key a shortcut names. ⌥ composes another character on macOS (⌥N → '˜')
// and ⇧ turns symbols into their shifted form, so `e.key` alone would never
// match 'alt+n' or 'shift+]'. A plain letter or digit keeps `e.key`, which
// follows the user's layout; only a composed or shifted character falls back
// to the physical key.
function shortcutKeyName(e: KeyboardEvent): string {
  const key = e.key.toLowerCase()
  if (/^[a-z0-9]$/.test(key)) return key
  if (e.altKey || e.shiftKey) {
    const physical = keyFromCode(e.code)
    if (physical) return physical
  }
  return key
}

// Helper: Normalize a keyboard event to our key format.
//
// Modifier tokens (preserved in output):
//   ctrl → Ctrl key on Windows/Linux, or Ctrl key on macOS
//   cmd  → Meta/⌘ key on macOS (only emitted on macOS)
//
// On macOS both modifiers can be held simultaneously (e.g. ⌘⌃T); each is
// emitted as its own token so multi-modifier combos record correctly.
export function normalizeKeyEvent(e: KeyboardEvent): string {
  const parts: string[] = []

  // Add modifiers in canonical order matching persisted defaults.
  if (isMac) {
    // macOS: emit ctrl and cmd independently so multi-modifier combos like
    // ⌘⌃T (cmd+ctrl held together) can be recorded. A single modifier still
    // emits just that token, preserving the ⌘+K vs Ctrl+K distinction.
    // Canonical order is ctrl before cmd (see MODIFIER_ORDER).
    if (e.ctrlKey) parts.push('ctrl')
    if (e.metaKey) parts.push('cmd')
  } else {
    if (e.ctrlKey) parts.push('ctrl')
    else if (e.metaKey) parts.push('cmd')
  }

  if (e.shiftKey) parts.push('shift')
  if (e.altKey) parts.push('alt')

  // Add the key itself (lowercase)
  let key = shortcutKeyName(e)

  // Handle special keys
  if (key === ' ') key = 'space'
  if (key === 'escape') key = 'esc'

  // Normalize key values for common keys that might have variations
  if (key === '-' || key === '–' || key === '—' || key === '_') key = '-'
  if (key === '=' || key === '+') key = '='

  // Skip if only modifier was pressed
  if (['control', 'alt', 'shift', 'meta'].includes(key)) {
    return parts.join('+')
  }

  parts.push(key)
  return parts.join('+')
}

// Helper: Format a key combination for display
export function formatKeyForDisplay(key: string): string {
  if (!key) return ''

  const parts = key.split('+')
  // On macOS a lone ctrl modifier is matched as ⌘ (⌃ stays with the shell),
  // so show the key that actually fires the shortcut.
  const primaryIsCmd = isMac && parts.includes('ctrl') && !parts.includes('cmd')

  return parts
    .map((part) => {
      switch (part) {
        case 'ctrl':
          return isMac ? (primaryIsCmd ? '⌘' : '⌃') : 'Ctrl'
        case 'cmd':
          return isMac ? '⌘' : 'Meta'
        case 'alt':
          return isMac ? '⌥' : 'Alt'
        case 'shift':
          return isMac ? '⇧' : 'Shift'
        case 'tab':
          return 'Tab'
        case 'esc':
          return 'Esc'
        case 'space':
          return 'Space'
        case 'pageup':
          return 'PageUp'
        case 'pagedown':
          return 'PageDown'
        case 'arrowup':
          return '↑'
        case 'arrowdown':
          return '↓'
        case 'arrowleft':
          return '←'
        case 'arrowright':
          return '→'
        case 'enter':
          return isMac ? '↩' : 'Enter'
        case 'home':
          return 'Home'
        case 'end':
          return 'End'
        case 'delete':
          return isMac ? '⌦' : 'Delete'
        case 'backspace':
          return isMac ? '⌫' : 'Backspace'
        default:
          return part.toUpperCase()
      }
    })
    .join(isMac ? '' : '+')
}

// Helper: Check if a keyboard event matches a shortcut key.
//
// Platform-aware matching:
//   - Config stores keys in 'ctrl+...' format (backward compatible).
//   - On macOS, 'cmd+...' from normalizeKeyEvent also matches 'ctrl+...' config entries.
//   - On Windows/Linux, matching is exact.
export function matchesShortcut(e: KeyboardEvent, shortcutKey: string): boolean {
  // On macOS, Ctrl+... (without ⌘) is the shell passthrough modifier.
  // It must never trigger app shortcuts — only ⌘+... does.
  // This mirrors the passthrough guard in ConnectedTerminal.tsx.
  if (isMac && e.ctrlKey && !e.metaKey) return false

  const normalized = normalizeKeyEvent(e)
  if (shortcutsEqual(normalized, shortcutKey)) return true

  // macOS cross-modifier alias: 'cmd+x' matches 'ctrl+x' config and vice-versa.
  // This lets the same 'ctrl+k' default work with both ⌘+K and Ctrl+K on Mac.
  // Only single-modifier combos are aliased — when both ⌘ and ⌃ are held
  // (e.g. ⌘⌃T) the combo is matched exactly, never swapped, otherwise the
  // alias would corrupt it into 'cmd+cmd+t'.
  if (isMac) {
    const parts = normalized.split('+')
    const hasCmd = parts.includes('cmd')
    const hasCtrl = parts.includes('ctrl')
    if (hasCmd !== hasCtrl) {
      const aliased = parts
        .map((part) => (part === 'cmd' ? 'ctrl' : part === 'ctrl' ? 'cmd' : part))
        .join('+')
      return shortcutsEqual(aliased, shortcutKey)
    }
  }

  return false
}
