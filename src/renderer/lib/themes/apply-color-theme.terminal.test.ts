/**
 * T-G1B — the terminal color theme is independent of the UI color theme.
 *
 * `terminalColorTheme: null` means "follow the UI theme"; a concrete id pins
 * the terminals while the UI theme moves freely.
 *
 * The assertions compare *palettes*, not ids. `getColorThemeDefinition` falls
 * back to the default theme silently, so an id-shaped assertion would pass on
 * a fallback; a background color that belongs to exactly one bundled theme
 * cannot.
 */
import type { Terminal } from '@xterm/xterm'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { getTerminalOptions } from '@/components/terminal/terminal-config'
import { useAppSettingsStore } from '@/stores/app-settings-store'
import { DEFAULT_APP_SETTINGS } from '@/types/settings'
import { clearRegistry, registerTerminal } from '@/utils/terminal-registry'
import {
  applyColorTheme,
  getActiveTerminalTheme,
  getActiveTerminalThemeId,
  getLastAppliedColorThemeId
} from './apply-color-theme'
import { BUNDLED_COLOR_THEMES } from './bundled-themes'
import { COLOR_THEME_CHANGED_EVENT, type ColorThemeChangedDetail } from './types'

const DRACULA_BG = BUNDLED_COLOR_THEMES.dracula.dark.palette.neutral
const NORD_BG = BUNDLED_COLOR_THEMES.nord.dark.palette.neutral

/** A terminal that is already open when the theme changes. */
function openTerminal(id: string): Terminal {
  const terminal = { options: {}, rows: 24, refresh: vi.fn() } as unknown as Terminal
  registerTerminal(id, terminal)
  return terminal
}

function setTerminalColorTheme(value: string | null): void {
  useAppSettingsStore.setState({
    settings: { ...DEFAULT_APP_SETTINGS, terminalColorTheme: value }
  })
}

beforeEach(() => {
  clearRegistry()
  setTerminalColorTheme(DEFAULT_APP_SETTINGS.terminalColorTheme)
})

afterEach(() => {
  clearRegistry()
  useAppSettingsStore.setState({ settings: DEFAULT_APP_SETTINGS })
})

describe('terminalColorTheme = null (the default)', () => {
  it('is the shipped default, so terminals follow the UI theme out of the box', () => {
    expect(DEFAULT_APP_SETTINGS.terminalColorTheme).toBeNull()
  })

  it('moves the terminal when the UI theme changes', () => {
    const terminal = openTerminal('a')

    applyColorTheme('dracula')
    expect(terminal.options.theme?.background).toBe(DRACULA_BG)

    applyColorTheme('nord')
    expect(terminal.options.theme?.background).toBe(NORD_BG)
  })
})

describe('terminalColorTheme = a concrete id', () => {
  it('leaves the terminal alone when the UI theme changes', () => {
    const terminal = openTerminal('a')
    setTerminalColorTheme('dracula')

    applyColorTheme('nord')

    expect(terminal.options.theme?.background).toBe(DRACULA_BG)
    expect(getLastAppliedColorThemeId()).toBe('nord')
  })

  it('returns the terminal to the UI theme when set back to null', () => {
    const terminal = openTerminal('a')
    setTerminalColorTheme('dracula')
    applyColorTheme('nord')
    expect(terminal.options.theme?.background).toBe(DRACULA_BG)

    setTerminalColorTheme(null)
    applyColorTheme('nord')

    expect(terminal.options.theme?.background).toBe(NORD_BG)
  })

  it('follows the UI theme again when the override id no longer resolves', () => {
    setTerminalColorTheme('a-theme-the-user-deleted')
    applyColorTheme('nord')

    // Not the default theme's palette: the silent fallback would strand the
    // terminals on a theme nothing else in the window is using.
    expect(getActiveTerminalThemeId()).toBe('nord')
    expect(getActiveTerminalTheme().background).toBe(NORD_BG)
  })
})

/**
 * The split-state gate.
 *
 * `getActiveTerminalTheme` has three call sites — this module's own terminal
 * pass, `terminal-config.ts`'s `getTerminalOptions` (a brand-new terminal), and
 * `ConnectedTerminal`'s cached-restore branch. Missing any one of them shows up
 * as "the terminal I already had follows the UI theme, the one I just opened
 * follows the override".
 */
describe('open and newly-opened terminals agree', () => {
  it('resolves to the same theme under one setting', () => {
    const alreadyOpen = openTerminal('a')
    setTerminalColorTheme('dracula')

    applyColorTheme('nord')
    const newlyOpened = getTerminalOptions('MacIntel').theme

    expect(alreadyOpen.options.theme?.background).toBe(DRACULA_BG)
    expect(newlyOpened?.background).toBe(DRACULA_BG)
    expect(newlyOpened).toEqual(alreadyOpen.options.theme)
  })

  it('agrees while following the UI theme too', () => {
    const alreadyOpen = openTerminal('a')

    applyColorTheme('nord')

    expect(getTerminalOptions('MacIntel').theme).toEqual(alreadyOpen.options.theme)
  })
})

/**
 * `use-codemirror.ts:384` and `MermaidBlock.tsx:86` are the other two real
 * `COLOR_THEME_CHANGED_EVENT` listeners, and both resolve their colors from the
 * *UI* theme (`getLastAppliedColorThemeId` / `detail.syntax`). An override that
 * leaked into either would leave the editor and the diagrams on the terminal's
 * theme.
 */
describe('the editor listeners still see the UI theme', () => {
  it('dispatches the UI theme id and its syntax while the terminal is overridden', () => {
    const details: ColorThemeChangedDetail[] = []
    const listener = (event: Event): void => {
      details.push((event as CustomEvent<ColorThemeChangedDetail>).detail)
    }
    window.addEventListener(COLOR_THEME_CHANGED_EVENT, listener)
    setTerminalColorTheme('dracula')

    applyColorTheme('nord')
    window.removeEventListener(COLOR_THEME_CHANGED_EVENT, listener)

    expect(details).toHaveLength(1)
    expect(details[0]?.themeId).toBe('nord')
    expect(getLastAppliedColorThemeId()).toBe('nord')
    // Nord's `syntax-keyword` override, i.e. resolved from the UI theme.
    expect(details[0]?.syntax.keyword).toBe(
      BUNDLED_COLOR_THEMES.nord.dark.overrides?.['syntax-keyword']
    )
  })
})
