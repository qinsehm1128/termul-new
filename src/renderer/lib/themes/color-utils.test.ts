import { describe, expect, it } from 'vitest'
import {
  hexChroma,
  hexHue,
  hexToHslComponents,
  hueDistance,
  mixHex,
  normalizeHex,
  parseHexColor,
  shouldOverrideToken
} from './color-utils'

describe('color-utils', () => {
  it('parses 6-digit hex', () => {
    expect(parseHexColor('#3b82f6')).toEqual({ r: 59, g: 130, b: 246 })
  })

  it('parses 3-digit hex', () => {
    expect(parseHexColor('#fff')).toEqual({ r: 255, g: 255, b: 255 })
  })

  it('converts blue hex to hsl components', () => {
    expect(hexToHslComponents('#3b82f6')).toBe('217 91% 60%')
  })

  it('mixes two colors', () => {
    expect(mixHex('#000000', '#ffffff', 0.5)).toBe('#808080')
  })

  it('normalizes 3- and 6-digit hex', () => {
    expect(normalizeHex('#ABC')).toBe('#aabbcc')
    expect(normalizeHex('#E5E5E5')).toBe('#e5e5e5')
  })

  it('rejects malformed hex values', () => {
    expect(() => parseHexColor('#12zzzz')).toThrow('Invalid hex color')
    expect(() => normalizeHex('#e4e4e45e')).toThrow('Invalid hex color')
  })

  it('detects when override differs from base', () => {
    expect(shouldOverrideToken('#9cdcfe', '#e5e5e5')).toBe(true)
    expect(shouldOverrideToken('#e5e5e5', '#E5E5E5')).toBe(false)
  })

  it('measures chroma and hue for ANSI mapping', () => {
    expect(hexChroma('#808080')).toBe(0)
    expect(hexChroma('#ff0000')).toBe(255)
    expect(hexHue('#ff0000')).toBeCloseTo(0, 0)
    expect(hexHue('#00ff00')).toBeCloseTo(120, 0)
    expect(hueDistance(10, 350)).toBeCloseTo(20, 0)
  })
})
