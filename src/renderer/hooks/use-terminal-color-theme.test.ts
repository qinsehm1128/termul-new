/**
 * T-G1B — a terminal that is already mounted has to react when
 * `terminalColorTheme` changes.
 *
 * The setting change dispatches no `COLOR_THEME_CHANGED_EVENT` (the UI theme
 * did not move, and telling the CodeMirror / Mermaid listeners otherwise would
 * be a lie), so this hook is the only thing that repaints a live terminal.
 */

import { act, renderHook } from '@testing-library/react'
import type { Terminal } from '@xterm/xterm'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { applyColorTheme } from '@/lib/themes'
import { BUNDLED_COLOR_THEMES } from '@/lib/themes/bundled-themes'
import { useAppSettingsStore } from '@/stores/app-settings-store'
import { DEFAULT_APP_SETTINGS } from '@/types/settings'
import { useTerminalColorTheme } from './use-terminal-color-theme'

const DRACULA_BG = BUNDLED_COLOR_THEMES.dracula.dark.palette.neutral
const NORD_BG = BUNDLED_COLOR_THEMES.nord.dark.palette.neutral

function fakeTerminal(): Terminal {
  return { options: {}, rows: 24, refresh: vi.fn() } as unknown as Terminal
}

function setTerminalColorTheme(value: string | null): void {
  act(() => {
    useAppSettingsStore.setState({
      settings: { ...DEFAULT_APP_SETTINGS, terminalColorTheme: value }
    })
  })
}

beforeEach(() => {
  useAppSettingsStore.setState({ settings: DEFAULT_APP_SETTINGS })
  applyColorTheme('nord')
})

afterEach(() => {
  useAppSettingsStore.setState({ settings: DEFAULT_APP_SETTINGS })
})

describe('useTerminalColorTheme', () => {
  it('attaches the UI theme when the setting is null', () => {
    const terminal = fakeTerminal()

    renderHook(() => useTerminalColorTheme(terminal))

    expect(terminal.options.theme?.background).toBe(NORD_BG)
  })

  it('repaints a mounted terminal when the override is set and cleared', () => {
    const terminal = fakeTerminal()
    renderHook(() => useTerminalColorTheme(terminal))

    setTerminalColorTheme('dracula')
    expect(terminal.options.theme?.background).toBe(DRACULA_BG)

    setTerminalColorTheme(null)
    expect(terminal.options.theme?.background).toBe(NORD_BG)
  })
})
