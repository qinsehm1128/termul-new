import { brandCanonical } from '@shared/brand'
import { afterEach, describe, expect, it } from 'vitest'
import { useAppSettingsStore } from '@/stores/app-settings-store'
import { DEFAULT_APP_SETTINGS } from '@/types/settings'
import { applyColorTheme, deriveUiTextTones } from './apply-color-theme'
import { BUNDLED_COLOR_THEMES } from './bundled-themes'
import { contrastRatio, hexToHslComponents, mixHex } from './color-utils'
import { deriveSurfaces } from './derive-surfaces'

const themes = Object.values(BUNDLED_COLOR_THEMES)

function surfacesOf(id: string): string[] {
  const theme = BUNDLED_COLOR_THEMES[id]
  const { card, secondary, sidebar } = deriveSurfaces(theme.dark.palette, theme.appearance)
  return [theme.dark.palette.neutral, sidebar, card, secondary]
}

function minContrast(color: string, surfaces: string[]): number {
  return Math.min(...surfaces.map((surface) => contrastRatio(color, surface)))
}

describe('UI text contrast floors', () => {
  afterEach(() => {
    document.documentElement.removeAttribute('style')
    useAppSettingsStore.setState({ settings: DEFAULT_APP_SETTINGS })
  })

  it('lifts One Dark sidebar text from 3.5:1 to a readable 4.5:1', () => {
    const { palette } = BUNDLED_COLOR_THEMES['one-dark'].dark
    const surfaces = surfacesOf('one-dark')
    // The fixed 35% fade this replaces.
    expect(minContrast(mixHex(palette.ink, palette.neutral, 0.35), surfaces)).toBeLessThan(4)

    const tones = deriveUiTextTones(palette, 'dark', 0)

    expect(minContrast(tones.secondary, surfaces)).toBeGreaterThanOrEqual(4.5)
    expect(tones.ink).toBe(palette.ink)
  })

  it.each(
    themes.map((theme) => [theme.id])
  )('keeps %s secondary text readable at 0% contrast', (id) => {
    const theme = BUNDLED_COLOR_THEMES[id]
    const tones = deriveUiTextTones(theme.dark.palette, theme.appearance, 0)
    const surfaces = surfacesOf(id)
    // A floor the theme's own ink cannot reach falls back to the ink itself.
    const reachable = (floor: number) =>
      Math.min(floor, minContrast(theme.dark.palette.ink, surfaces))

    expect(minContrast(tones.secondary, surfaces)).toBeGreaterThanOrEqual(reachable(4.5))
    expect(minContrast(tones.muted, surfaces)).toBeGreaterThanOrEqual(reachable(3))
  })

  it('leaves a theme that already clears the floors exactly as it was', () => {
    const { palette } = BUNDLED_COLOR_THEMES[brandCanonical().themeId].dark

    const tones = deriveUiTextTones(palette, 'dark', 0)

    expect(tones).toEqual({
      ink: palette.ink,
      secondary: mixHex(palette.ink, palette.neutral, 0.35),
      muted: mixHex(palette.ink, palette.neutral, 0.5),
      statusBar: mixHex(palette.ink, palette.neutral, 0.45)
    })
  })

  it.each(
    themes.map((theme) => [theme.id])
  )('raises every tone of %s at the 50% midpoint (0.14.7 "high")', (id) => {
    const theme = BUNDLED_COLOR_THEMES[id]
    const surfaces = surfacesOf(id)
    const standard = deriveUiTextTones(theme.dark.palette, theme.appearance, 0)

    const high = deriveUiTextTones(theme.dark.palette, theme.appearance, 50)

    expect(minContrast(high.ink, surfaces)).toBeGreaterThanOrEqual(7)
    expect(minContrast(high.secondary, surfaces)).toBeGreaterThanOrEqual(
      minContrast(standard.secondary, surfaces)
    )
    expect(minContrast(high.muted, surfaces)).toBeGreaterThanOrEqual(4.5)
  })

  it('applies the contrast setting to UI text but never to the terminal', () => {
    useAppSettingsStore.setState({
      settings: { ...DEFAULT_APP_SETTINGS, uiContrast: 50 }
    })
    const { palette } = BUNDLED_COLOR_THEMES['one-dark'].dark
    const high = deriveUiTextTones(palette, 'dark', 50)

    applyColorTheme('one-dark')

    const style = document.documentElement.style
    expect(style.getPropertyValue('--sidebar-foreground')).toBe(hexToHslComponents(high.secondary))
    expect(style.getPropertyValue('--foreground')).toBe(hexToHslComponents(high.ink))
    expect(style.getPropertyValue('--terminal-fg')).toBe(hexToHslComponents(palette.ink))
  })

  it.each(
    themes.map((theme) => [theme.id])
  )('never lowers %s contrast as the slider moves right', (id) => {
    const theme = BUNDLED_COLOR_THEMES[id]
    const surfaces = surfacesOf(id)
    let previous = deriveUiTextTones(theme.dark.palette, theme.appearance, 0)
    for (let level = 10; level <= 100; level += 10) {
      const tones = deriveUiTextTones(theme.dark.palette, theme.appearance, level)
      for (const key of ['ink', 'secondary', 'muted'] as const) {
        expect(minContrast(tones[key], surfaces)).toBeGreaterThanOrEqual(
          minContrast(previous[key], surfaces) - 0.01
        )
      }
      previous = tones
    }
  })

  it('brightens One Dark sidebar text further at 100% than at 50%', () => {
    const { palette } = BUNDLED_COLOR_THEMES['one-dark'].dark
    const surfaces = surfacesOf('one-dark')

    const mid = deriveUiTextTones(palette, 'dark', 50)
    const max = deriveUiTextTones(palette, 'dark', 100)

    expect(minContrast(max.secondary, surfaces)).toBeGreaterThan(
      minContrast(mid.secondary, surfaces) + 1
    )
  })
})
