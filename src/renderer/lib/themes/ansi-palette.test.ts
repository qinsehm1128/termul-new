import { brandCanonical } from '@shared/brand'
import { describe, expect, it } from 'vitest'
import { ansi16FromPalette, STANDARD_ANSI_DARK } from './ansi-palette'
import { BUNDLED_COLOR_THEMES } from './bundled-themes'

describe('ansi16FromPalette', () => {
  it('keeps colorful theme tokens that match the ANSI hue', () => {
    const ansi = ansi16FromPalette(BUNDLED_COLOR_THEMES.dracula.dark.palette, 'dark')
    expect(ansi.red).toBe('#ff5555')
    expect(ansi.green).toBe('#50fa7b')
    expect(ansi.magenta).toBe('#ff79c6')
    expect(ansi.cyan).toBe('#8be9fd')
  })

  it('falls back to a vivid table when the workbench accent is muted olive', () => {
    const ansi = ansi16FromPalette(
      {
        neutral: '#131410',
        ink: '#e6e5e0',
        primary: '#8a9d72',
        accent: '#8a9d72',
        success: '#739d6c',
        warning: '#c39f69',
        error: '#c26b6b',
        info: '#7898ab'
      },
      'dark'
    )
    expect(ansi.blue).toBe(STANDARD_ANSI_DARK.blue)
    expect(ansi.magenta).toBe(STANDARD_ANSI_DARK.magenta)
    expect(ansi.green).toBe(STANDARD_ANSI_DARK.green)
    expect(ansi.red).toBe('#c26b6b')
  })

  it('keeps standard ANSI blue distinct from the Se Velvet Plum brand primary', () => {
    const ansi = ansi16FromPalette(
      BUNDLED_COLOR_THEMES[brandCanonical().themeId].dark.palette,
      'dark'
    )
    expect(ansi.blue).toBe(STANDARD_ANSI_DARK.blue)
    expect(ansi.green).toBe('#8bc9b8')
    expect(ansi.red).toBe('#eb6f92')
  })
})
