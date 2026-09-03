import type { Terminal } from '@xterm/xterm'
import { useEffect } from 'react'
import {
  applyThemeToTerminal,
  COLOR_THEME_CHANGED_EVENT,
  getActiveTerminalTheme
} from '@/lib/themes'
import { useTerminalColorThemeSetting } from '@/stores/app-settings-store'

/** Keep a terminal instance in sync with the active color theme (attach + live updates). */
export function useTerminalColorTheme(terminal: Terminal | null): void {
  // `null` follows the UI theme, a concrete id overrides it. Read as a
  // dependency rather than through a second listener: `getActiveTerminalTheme`
  // stays the one resolver, and this effect is what re-runs when the user
  // switches between following and overriding.
  const terminalColorTheme = useTerminalColorThemeSetting()

  useEffect(() => {
    if (!terminal) return
    // Read inside the effect so the dependency is real: `getActiveTerminalTheme`
    // resolves this setting itself, so the value never appears in the body.
    void terminalColorTheme
    const apply = (): void => {
      applyThemeToTerminal(terminal, getActiveTerminalTheme())
    }
    apply()
    window.addEventListener(COLOR_THEME_CHANGED_EVENT, apply)
    return () => {
      window.removeEventListener(COLOR_THEME_CHANGED_EVENT, apply)
    }
  }, [terminal, terminalColorTheme])
}
