import { darkenHex, lightenHex } from './color-utils'
import type { ThemeAppearance, ThemePalette } from './types'

export interface DerivedSurfaces {
  card: string
  popover: string
  secondary: string
  muted: string
  border: string
  sidebar: string
}

/** Derive elevated surface colors from base palette for dark or light chrome. */
export function deriveSurfaces(
  palette: ThemePalette,
  appearance: ThemeAppearance
): DerivedSurfaces {
  if (appearance === 'light') {
    return {
      card: darkenHex(palette.neutral, 0.015),
      popover: darkenHex(palette.neutral, 0.028),
      secondary: darkenHex(palette.neutral, 0.035),
      muted: darkenHex(palette.neutral, 0.055),
      border: darkenHex(palette.neutral, 0.09),
      sidebar: darkenHex(palette.neutral, 0.01)
    }
  }

  return {
    card: lightenHex(palette.neutral, 0.025),
    popover: lightenHex(palette.neutral, 0.04),
    secondary: lightenHex(palette.neutral, 0.045),
    muted: lightenHex(palette.neutral, 0.065),
    border: lightenHex(palette.neutral, 0.09),
    sidebar: lightenHex(palette.neutral, 0.012)
  }
}
