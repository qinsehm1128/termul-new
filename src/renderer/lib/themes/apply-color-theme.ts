import type { ITheme } from '@xterm/xterm'
import { forEachTerminal } from '@/utils/terminal-registry'
import { ansi16FromPalette } from './ansi-palette'
import { applyThemeToTerminal } from './apply-theme-to-terminal'
import {
  DEFAULT_COLOR_THEME_ID,
  getColorThemeDefinition,
  hasBundledColorTheme
} from './bundled-themes'
import { darkenHex, hexToHslComponents, lightenHex, mixHex } from './color-utils'
import { deriveSurfaces } from './derive-surfaces'
import { resolveSyntaxColors } from './resolve-syntax'
import {
  COLOR_THEME_CHANGED_EVENT,
  type ColorThemeChangedDetail,
  type ColorThemeDefinition,
  type ThemeAppearance,
  type ThemePalette
} from './types'

let lastAppliedThemeId = DEFAULT_COLOR_THEME_ID

export function getLastAppliedColorThemeId(): string {
  return lastAppliedThemeId
}

function applyDocumentAppearance(appearance: ThemeAppearance): void {
  const root = document.documentElement
  if (appearance === 'light') {
    root.style.colorScheme = 'light'
    root.classList.remove('dark')
  } else {
    root.style.colorScheme = 'dark'
    root.classList.add('dark')
  }
}

function applyCssVariables(palette: ThemePalette, appearance: ThemeAppearance): void {
  const root = document.documentElement
  const surfaces = deriveSurfaces(palette, appearance)
  const { card, popover, secondary, muted, border, sidebar } = surfaces
  const primaryForeground =
    appearance === 'light'
      ? hexToHslComponents(lightenHex(palette.primary, 0.98))
      : hexToHslComponents(lightenHex(palette.primary, 0.95))
  const accentForeground =
    appearance === 'light'
      ? hexToHslComponents(lightenHex(palette.accent, 0.98))
      : hexToHslComponents(lightenHex(palette.accent, 0.95))

  const vars: Record<string, string> = {
    '--background': hexToHslComponents(palette.neutral),
    '--foreground': hexToHslComponents(palette.ink),
    '--card': hexToHslComponents(card),
    '--card-foreground': hexToHslComponents(palette.ink),
    '--popover': hexToHslComponents(popover),
    '--popover-foreground': hexToHslComponents(palette.ink),
    '--primary': hexToHslComponents(palette.primary),
    '--primary-foreground': primaryForeground,
    '--secondary': hexToHslComponents(secondary),
    '--secondary-foreground': hexToHslComponents(mixHex(palette.ink, palette.neutral, 0.35)),
    '--muted': hexToHslComponents(muted),
    '--muted-foreground': hexToHslComponents(mixHex(palette.ink, palette.neutral, 0.5)),
    '--accent': hexToHslComponents(palette.accent),
    '--accent-foreground': accentForeground,
    '--destructive': hexToHslComponents(palette.error),
    '--destructive-foreground': hexToHslComponents('#ffffff'),
    '--success': hexToHslComponents(palette.success),
    '--success-foreground': hexToHslComponents('#ffffff'),
    '--connection': hexToHslComponents(palette.info),
    '--warning': hexToHslComponents(palette.warning),
    '--warning-foreground': hexToHslComponents(
      appearance === 'light' ? darkenHex(palette.warning, 0.45) : darkenHex(palette.warning, 0.55)
    ),
    '--border': hexToHslComponents(border),
    '--input': hexToHslComponents(border),
    '--ring': hexToHslComponents(palette.primary),
    '--terminal-bg': hexToHslComponents(palette.neutral),
    '--terminal-fg': hexToHslComponents(palette.ink),
    '--surface-dark': hexToHslComponents(card),
    '--surface-darker': hexToHslComponents(palette.neutral),
    '--status-bar': hexToHslComponents(secondary),
    '--status-bar-foreground': hexToHslComponents(mixHex(palette.ink, palette.neutral, 0.45)),
    '--sidebar-background': hexToHslComponents(sidebar),
    '--sidebar-foreground': hexToHslComponents(mixHex(palette.ink, palette.neutral, 0.35)),
    '--sidebar-primary': hexToHslComponents(palette.primary),
    '--sidebar-primary-foreground': hexToHslComponents('#ffffff'),
    '--sidebar-accent': hexToHslComponents(secondary),
    '--sidebar-accent-foreground': hexToHslComponents(palette.ink),
    '--sidebar-border': hexToHslComponents(border),
    '--sidebar-ring': hexToHslComponents(palette.primary)
  }

  for (const [key, value] of Object.entries(vars)) {
    root.style.setProperty(key, value)
  }

  applyDocumentAppearance(appearance)
}

export function paletteToXtermTheme(palette: ThemePalette, appearance: ThemeAppearance): ITheme {
  const isLight = appearance === 'light'
  return {
    background: palette.neutral,
    foreground: palette.ink,
    cursor: palette.ink,
    cursorAccent: palette.neutral,
    selectionBackground: mixHex(palette.primary, palette.neutral, isLight ? 0.25 : 0.35),
    selectionForeground: palette.ink,
    selectionInactiveBackground: isLight
      ? darkenHex(palette.neutral, 0.06)
      : lightenHex(palette.neutral, 0.12),
    ...ansi16FromPalette(palette, appearance)
  }
}

function applyTerminalThemes(xtermTheme: ITheme): void {
  forEachTerminal((terminal) => {
    applyThemeToTerminal(terminal, xtermTheme)
  })
}

function dispatchThemeChanged(detail: ColorThemeChangedDetail): void {
  window.dispatchEvent(new CustomEvent(COLOR_THEME_CHANGED_EVENT, { detail }))
}

/** Apply theme to document, terminals, and notify editors (instant, no persistence). */
export function applyColorTheme(themeId: string): void {
  const theme = getColorThemeDefinition(themeId)
  const variant = theme.dark
  const syntax = resolveSyntaxColors(theme)
  const xtermTheme = paletteToXtermTheme(variant.palette, theme.appearance)

  applyCssVariables(variant.palette, theme.appearance)
  applyTerminalThemes(xtermTheme)
  lastAppliedThemeId = theme.id
  dispatchThemeChanged({ themeId: theme.id, syntax })
}

export function getActiveTerminalTheme(): ITheme {
  const theme = getColorThemeDefinition(lastAppliedThemeId)
  return paletteToXtermTheme(theme.dark.palette, theme.appearance)
}

export function isKnownColorThemeId(themeId: string): boolean {
  return hasBundledColorTheme(themeId)
}

/** @internal for tests */
export function resolveThemeForTest(theme: ColorThemeDefinition): {
  syntax: ReturnType<typeof resolveSyntaxColors>
  xterm: ITheme
} {
  return {
    syntax: resolveSyntaxColors(theme),
    xterm: paletteToXtermTheme(theme.dark.palette, theme.appearance)
  }
}
