import { brandCanonical } from '@shared/brand'
import { afterEach, describe, expect, it } from 'vitest'
import { applyColorTheme, paletteToXtermTheme, resolveThemeForTest } from './apply-color-theme'
import { BUNDLED_COLOR_THEMES } from './bundled-themes'
import { hexToHslComponents } from './color-utils'
import { deriveSurfaces } from './derive-surfaces'
import { resolveSyntaxColors } from './resolve-syntax'

describe('apply-color-theme', () => {
  afterEach(() => {
    document.documentElement.removeAttribute('style')
    document.documentElement.classList.remove('dark')
  })

  it('includes dark and light bundled themes', () => {
    const ids = Object.keys(BUNDLED_COLOR_THEMES)
    expect(ids).toContain(brandCanonical().themeId)
    expect(ids).toContain(brandCanonical().themeFamilyLight)
    expect(ids).toContain('catppuccin')
    expect(ids).toContain('catppuccin-light')
    expect(ids.length).toBe(20)
  })

  it('derives syntax colors from catppuccin palette', () => {
    const theme = BUNDLED_COLOR_THEMES.catppuccin
    const syntax = resolveSyntaxColors(theme)
    expect(syntax.keyword).toBe('#cba6f7')
    expect(syntax.string).toBe('#a6e3a1')
    expect(syntax.function).toBe('#89b4fa')
  })

  it('separates the brand theme function color from keyword', () => {
    const syntax = resolveSyntaxColors(BUNDLED_COLOR_THEMES[brandCanonical().themeId])
    expect(syntax.keyword).toBe('#c586c0')
    expect(syntax.function).toBe('#dcdcaa')
  })

  it('maps palette to xterm theme', () => {
    const { xterm } = resolveThemeForTest(BUNDLED_COLOR_THEMES.dracula)
    expect(xterm.background).toBe('#1d1e28')
    expect(xterm.foreground).toBe('#f8f8f2')
    expect(xterm.green).toBe('#50fa7b')
    expect(xterm.red).toBe('#ff5555')
  })

  it('does not reuse the Se olive accent as ANSI blue or magenta', () => {
    const xterm = paletteToXtermTheme(
      BUNDLED_COLOR_THEMES[brandCanonical().themeId].dark.palette,
      'dark'
    )
    expect(xterm.blue).not.toBe('#8a9d72')
    expect(xterm.magenta).not.toBe('#8a9d72')
    expect(xterm.blue).toBe('#2472c8')
    expect(xterm.green).toBe('#0dbc79')
    expect(xterm.red).toBe('#c26b6b')
  })

  it('exports paletteToXtermTheme with 16 ansi colors', () => {
    const xterm = paletteToXtermTheme(BUNDLED_COLOR_THEMES.nord.dark.palette, 'dark')
    expect(xterm.brightBlue).toBeTruthy()
    expect(xterm.brightWhite).toBeTruthy()
  })

  it('maps light palette to xterm theme', () => {
    const theme = BUNDLED_COLOR_THEMES['github-light']
    const { xterm } = resolveThemeForTest(theme)
    expect(theme.appearance).toBe('light')
    expect(xterm.background).toBe('#ffffff')
    expect(xterm.foreground).toBe('#24292f')
  })

  it.each([
    brandCanonical().themeId,
    brandCanonical().themeFamilyLight
  ])('applies a distinct --popover elevation from --card for %s', (themeId) => {
    applyColorTheme(themeId)
    const theme = BUNDLED_COLOR_THEMES[themeId]
    const surfaces = deriveSurfaces(theme.dark.palette, theme.appearance)
    const root = document.documentElement
    const card = root.style.getPropertyValue('--card')
    const popover = root.style.getPropertyValue('--popover')

    expect(card).toBe(hexToHslComponents(surfaces.card))
    expect(popover).toBe(hexToHslComponents(surfaces.popover))
    expect(popover).not.toBe(card)
  })
})
