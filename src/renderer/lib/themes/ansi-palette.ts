import type { ITheme } from '@xterm/xterm'
import { hexChroma, hexHue, hueDistance } from './color-utils'
import type { ThemeAppearance, ThemePalette } from './types'

/** VS Code Dark+ ANSI — distinct red/green/blue, not workbench accents. */
export const STANDARD_ANSI_DARK = {
  black: '#000000',
  red: '#cd3131',
  green: '#0dbc79',
  yellow: '#e5e510',
  blue: '#2472c8',
  magenta: '#bc3fbc',
  cyan: '#11a8cd',
  white: '#e5e5e5',
  brightBlack: '#666666',
  brightRed: '#f14c4c',
  brightGreen: '#23d18b',
  brightYellow: '#f5f543',
  brightBlue: '#3b8eea',
  brightMagenta: '#d670d6',
  brightCyan: '#29b8db',
  brightWhite: '#e5e5e5'
} as const

/** VS Code Light+ ANSI. */
export const STANDARD_ANSI_LIGHT = {
  black: '#000000',
  red: '#cd3131',
  green: '#00bc00',
  yellow: '#949800',
  blue: '#0451a5',
  magenta: '#bc05bc',
  cyan: '#0598bc',
  white: '#555555',
  brightBlack: '#666666',
  brightRed: '#cd3131',
  brightGreen: '#14ce14',
  brightYellow: '#b5ba00',
  brightBlue: '#0451a5',
  brightMagenta: '#bc05bc',
  brightCyan: '#0598bc',
  brightWhite: '#a5a5a5'
} as const

const MIN_CHROMA = 56
const MAX_HUE_DISTANCE = 55

function pickAnsi(candidate: string | undefined, expectedHue: number, fallback: string): string {
  if (!candidate) return fallback
  if (hexChroma(candidate) < MIN_CHROMA) return fallback
  if (hueDistance(hexHue(candidate), expectedHue) > MAX_HUE_DISTANCE) return fallback
  return candidate
}

/**
 * 16 ANSI colors for xterm. Workbench tokens (olive primary, shared accent)
 * are only used when they are actually that hue — otherwise the standard
 * table keeps `ls` / git / TUI diffs readable.
 */
export function ansi16FromPalette(
  palette: ThemePalette,
  appearance: ThemeAppearance
): Pick<
  ITheme,
  | 'black'
  | 'red'
  | 'green'
  | 'yellow'
  | 'blue'
  | 'magenta'
  | 'cyan'
  | 'white'
  | 'brightBlack'
  | 'brightRed'
  | 'brightGreen'
  | 'brightYellow'
  | 'brightBlue'
  | 'brightMagenta'
  | 'brightCyan'
  | 'brightWhite'
> {
  const base = appearance === 'light' ? STANDARD_ANSI_LIGHT : STANDARD_ANSI_DARK
  return {
    ...base,
    red: pickAnsi(palette.error, 8, base.red),
    green: pickAnsi(palette.success, 135, base.green),
    yellow: pickAnsi(palette.warning, 48, base.yellow),
    blue: pickAnsi(palette.primary, 220, base.blue),
    magenta: pickAnsi(palette.accent, 300, base.magenta),
    cyan: pickAnsi(palette.info, 190, base.cyan)
  }
}
