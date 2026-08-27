import type { Terminal } from '@xterm/xterm'
import { useEffect } from 'react'
import {
  applyThemeToTerminal,
  COLOR_THEME_CHANGED_EVENT,
  getActiveTerminalTheme
} from '@/lib/themes'

/** Keep a terminal instance in sync with the active color theme (attach + live updates). */
export function useTerminalColorTheme(terminal: Terminal | null): void {
  useEffect(() => {
    if (!terminal) return
    const apply = (): void => {
      applyThemeToTerminal(terminal, getActiveTerminalTheme())
    }
    apply()
    window.addEventListener(COLOR_THEME_CHANGED_EVENT, apply)
    return () => {
      window.removeEventListener(COLOR_THEME_CHANGED_EVENT, apply)
    }
  }, [terminal])
}
