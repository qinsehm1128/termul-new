import { describe, expect, it } from 'vitest'
import { CODEMIRROR_MONO_FONT_FAMILY, createTermulTheme } from './codemirror-theme'

describe('codemirror-theme', () => {
  it('uses the JetBrains Mono Variable stack for editor chrome', () => {
    expect(CODEMIRROR_MONO_FONT_FAMILY.startsWith('"JetBrains Mono Variable"')).toBe(true)
    expect(CODEMIRROR_MONO_FONT_FAMILY).toContain('"JetBrains Mono"')
    expect(CODEMIRROR_MONO_FONT_FAMILY).not.toMatch(/Nerd Font/)
  })

  it('returns theme extensions for dark and light editors', () => {
    expect(createTermulTheme(true).length).toBeGreaterThan(0)
    expect(createTermulTheme(false).length).toBeGreaterThan(0)
  })
})
