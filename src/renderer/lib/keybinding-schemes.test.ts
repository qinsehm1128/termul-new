import { describe, expect, it } from 'vitest'
import { DEFAULT_KEYBOARD_SHORTCUTS } from '@/types/settings'
import {
  BUILTIN_KEYBINDING_SCHEMES,
  buildSchemeShortcuts,
  isValidAccelerator,
  type KeybindingScheme,
  parseUserScheme,
  resolveSchemeBindings
} from './keybinding-schemes'

describe('resolveSchemeBindings', () => {
  it('gives iTerm2 on macOS the ⌘K clear and frees ⌘K from the palette', () => {
    const bindings = resolveSchemeBindings('iterm2', BUILTIN_KEYBINDING_SCHEMES, true)
    expect(bindings.clearTerminal).toBe('cmd+k')
    expect(bindings.commandPalette).toBe('')
    expect(bindings.nextTerminal).toBe('cmd+shift+]')
  })

  it('leaves mac-only bindings out on other platforms', () => {
    const bindings = resolveSchemeBindings('iterm2', BUILTIN_KEYBINDING_SCHEMES, false)
    expect(bindings).not.toHaveProperty('clearTerminal')
    expect(bindings).not.toHaveProperty('commandPalette')
  })

  it("uses Ghostty's Linux split keys off macOS", () => {
    const bindings = resolveSchemeBindings('ghostty', BUILTIN_KEYBINDING_SCHEMES, false)
    expect(bindings.splitRight).toBe('ctrl+shift+o')
    expect(bindings.splitDown).toBe('ctrl+shift+e')
  })

  it('layers a scheme over the one it extends', () => {
    const user: KeybindingScheme = {
      id: 'user:mine',
      name: 'Mine',
      builtin: false,
      extends: 'iterm2',
      bindings: { clearTerminal: 'cmd+shift+k', splitRight: null }
    }
    const bindings = resolveSchemeBindings('user:mine', [...BUILTIN_KEYBINDING_SCHEMES, user], true)
    expect(bindings.clearTerminal).toBe('cmd+shift+k')
    expect(bindings.splitRight).toBe('')
    expect(bindings.nextTerminal).toBe('cmd+shift+]')
  })

  it('stops on an extends cycle instead of looping', () => {
    const a: KeybindingScheme = {
      id: 'a',
      name: 'A',
      builtin: false,
      extends: 'b',
      bindings: { splitRight: 'cmd+1' }
    }
    const b: KeybindingScheme = {
      id: 'b',
      name: 'B',
      builtin: false,
      extends: 'a',
      bindings: { splitDown: 'cmd+2' }
    }
    expect(resolveSchemeBindings('a', [a, b], true)).toEqual({
      splitRight: 'cmd+1',
      splitDown: 'cmd+2'
    })
  })
})

describe('buildSchemeShortcuts', () => {
  it('puts the scheme key in defaultKey and keeps every custom key', () => {
    const shortcuts = buildSchemeShortcuts(
      { clearTerminal: 'cmd+k', commandPalette: '' },
      { clearTerminal: 'cmd+k', sidebarToggle: 'cmd+shift+s' }
    )
    expect(shortcuts.clearTerminal.defaultKey).toBe('cmd+k')
    // Kept even though it equals the scheme key, so switching back keeps it.
    expect(shortcuts.clearTerminal.customKey).toBe('cmd+k')
    expect(shortcuts.commandPalette.defaultKey).toBe('')
    expect(shortcuts.sidebarToggle.customKey).toBe('cmd+shift+s')
    expect(shortcuts.newProject.defaultKey).toBe(DEFAULT_KEYBOARD_SHORTCUTS.newProject.defaultKey)
  })
})

describe('isValidAccelerator', () => {
  it.each([
    ['cmd+d', true],
    ['ctrl+shift+arrowup', true],
    ['f2', true],
    ['cmd+', false],
    ['cmd+shift', false],
    ['hyper+d', false],
    ['cmd+cmd+d', false]
  ])('%s → %s', (value, expected) => {
    expect(isValidAccelerator(value)).toBe(expected)
  })
})

describe('parseUserScheme', () => {
  it('reads name, extends and bindings and names the scheme after the file', () => {
    const { scheme, issues } = parseUserScheme({
      name: 'work.json',
      content: JSON.stringify({
        name: 'Work',
        extends: 'ghostty',
        bindings: { splitRight: 'cmd+d', clearTerminal: { mac: 'cmd+k', other: null } }
      })
    })
    expect(issues).toEqual([])
    expect(scheme).toEqual({
      id: 'user:work',
      name: 'Work',
      builtin: false,
      extends: 'ghostty',
      bindings: { splitRight: 'cmd+d', clearTerminal: { mac: 'cmd+k', other: null } }
    })
  })

  it('skips and reports unknown actions and bad keys, keeping the rest', () => {
    const { scheme, issues } = parseUserScheme({
      name: 'typo.json',
      content: JSON.stringify({
        bindings: {
          splitRight: 'cmd+d',
          splitRigth: 'cmd+e',
          splitDown: 'cmd+',
          zoomIn: { win: 'x' }
        }
      })
    })
    expect(scheme?.name).toBe('typo')
    expect(scheme?.bindings).toEqual({ splitRight: 'cmd+d' })
    expect(issues.map((issue) => issue.message)).toEqual([
      'unknown action "splitRigth"',
      'invalid key for "splitDown"',
      'invalid key for "zoomIn"'
    ])
  })

  it('reports a file that is not JSON or has no bindings object', () => {
    expect(parseUserScheme({ name: 'a.json', content: '{' }).scheme).toBeUndefined()
    const noBindings = parseUserScheme({ name: 'b.json', content: '{"name":"B"}' })
    expect(noBindings.scheme).toBeUndefined()
    expect(noBindings.issues).toEqual([{ file: 'b.json', message: '"bindings" must be an object' }])
    const unreadable = parseUserScheme({ name: 'c.json', error: 'permission denied' })
    expect(unreadable.issues).toEqual([{ file: 'c.json', message: 'permission denied' }])
  })
})
