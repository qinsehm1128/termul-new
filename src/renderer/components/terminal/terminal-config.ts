import type { ITerminalOptions } from '@xterm/xterm'
import { getActiveTerminalTheme } from '@/lib/themes'

// Resize debounce delay in milliseconds - prevents flooding PTY with resize events during drag
export const RESIZE_DEBOUNCE_MS = 50

// Windows build number for ConPTY. Build 21376 (Windows 10 20H2) introduced stable ConPTY
// support. Using this value enables xterm.js to apply appropriate workarounds for ConPTY
// behavior (e.g., correct line wrapping calculations).
export const CONPTY_MIN_BUILD_NUMBER = 21376

export const DEFAULT_TERMINAL_OPTIONS: ITerminalOptions = {
  // Cross-platform monospace stack:
  // - JetBrains Mono Variable: bundled via @fontsource-variable, available everywhere.
  // - Cascadia / Menlo / Consolas / SF Mono: native fallbacks per OS.
  // - Ubuntu Mono / DejaVu Sans Mono: ships with most Linux distros.
  fontFamily:
    '"JetBrains Mono Variable", "JetBrains Mono", "Cascadia Code", "SF Mono", Menlo, Monaco, Consolas, "Ubuntu Mono", "DejaVu Sans Mono", "Liberation Mono", "Courier New", monospace',
  fontSize: 14,
  cursorBlink: true,
  cursorStyle: 'block',
  allowTransparency: false,
  // R-04: enabled solely so `terminal.unicode` is reachable for the Unicode v11
  // width tables. All proposed-API usage is confined to terminal-unicode.ts; no
  // other proposed member may be used anywhere in the renderer.
  allowProposedApi: true,
  // lineHeight > 1 yields a fractional CSS cell height (e.g. 16.5px at
  // 14px / DPR 2). xterm v6 then maps wheel scrollTop through
  // Math.round(scrollTop / cellHeight), so the last visible row is a blend
  // of two buffer lines — leftover glyphs after scroll. Keep cells on
  // integer pixels.
  lineHeight: 1,
  letterSpacing: 0,
  smoothScrollDuration: 0,
  // Wide/ambiguous CJK and box-drawing leftovers after a scroll.
  rescaleOverlappingGlyphs: true,
  scrollback: 10000,
  tabStopWidth: 4,
  convertEol: false,
  ignoreBracketedPasteMode: false,
  drawBoldTextInBrightColors: true,
  rightClickSelectsWord: true,
  // Wheel / trackpad still scroll; the custom xterm v6 slider is a thick
  // overlay on the last column and reads as a second system scrollbar.
  scrollbar: { showScrollbar: false },
  // The user-facing accessibility setting overrides this per ConnectedTerminal.
  // Keep the baseline off to avoid accessibility-tree overhead for most users.
  screenReaderMode: false
}

/**
 * Get platform-aware terminal options.
 * On Windows, adds windowsPty configuration for ConPTY compatibility.
 * Note: Uses navigator.platform (not process.platform) because this runs in
 * renderer process where Node.js APIs are not available.
 */
export function getTerminalOptions(platform: string): ITerminalOptions {
  const baseOptions: ITerminalOptions = {
    ...DEFAULT_TERMINAL_OPTIONS,
    theme: getActiveTerminalTheme()
  }

  if (platform.startsWith('Win')) {
    return {
      ...baseOptions,
      windowsPty: {
        backend: 'conpty',
        buildNumber: CONPTY_MIN_BUILD_NUMBER
      }
    }
  }

  return baseOptions
}

/** Apply cell-metric options to a live/cached terminal (project-switch restore). */
export function applyTerminalRenderOptions(terminal: {
  options: Pick<
    ITerminalOptions,
    | 'lineHeight'
    | 'letterSpacing'
    | 'smoothScrollDuration'
    | 'rescaleOverlappingGlyphs'
    | 'scrollbar'
  >
}): void {
  terminal.options.lineHeight = DEFAULT_TERMINAL_OPTIONS.lineHeight
  terminal.options.letterSpacing = DEFAULT_TERMINAL_OPTIONS.letterSpacing
  terminal.options.smoothScrollDuration = DEFAULT_TERMINAL_OPTIONS.smoothScrollDuration
  terminal.options.rescaleOverlappingGlyphs = DEFAULT_TERMINAL_OPTIONS.rescaleOverlappingGlyphs
  terminal.options.scrollbar = DEFAULT_TERMINAL_OPTIONS.scrollbar
}
