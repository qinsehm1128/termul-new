const HEX_COLOR_PATTERN = /^(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/

function stripHexPrefix(hex: string): string {
  return hex.trim().replace(/^#/, '')
}

function assertValidHex(hex: string): string {
  const normalized = stripHexPrefix(hex)
  if (!HEX_COLOR_PATTERN.test(normalized)) {
    throw new Error(`Invalid hex color: ${hex}`)
  }
  return normalized
}

/** Parse #rgb or #rrggbb to { r, g, b } in 0–255. */
export function parseHexColor(hex: string): { r: number; g: number; b: number } {
  const normalized = assertValidHex(hex)
  if (normalized.length === 3) {
    const r = parseInt(normalized[0] + normalized[0], 16)
    const g = parseInt(normalized[1] + normalized[1], 16)
    const b = parseInt(normalized[2] + normalized[2], 16)
    return { r, g, b }
  }
  if (normalized.length === 6) {
    return {
      r: parseInt(normalized.slice(0, 2), 16),
      g: parseInt(normalized.slice(2, 4), 16),
      b: parseInt(normalized.slice(4, 6), 16)
    }
  }
  throw new Error(`Invalid hex color: ${hex}`)
}

/** CSS variable format used by Tailwind: "H S% L%" without hsl() wrapper. */
export function hexToHslComponents(hex: string): string {
  const { r, g, b } = parseHexColor(hex)
  const rNorm = r / 255
  const gNorm = g / 255
  const bNorm = b / 255
  const max = Math.max(rNorm, gNorm, bNorm)
  const min = Math.min(rNorm, gNorm, bNorm)
  const delta = max - min

  let h = 0
  let s = 0
  const l = (max + min) / 2

  if (delta !== 0) {
    s = l > 0.5 ? delta / (2 - max - min) : delta / (max + min)
    switch (max) {
      case rNorm:
        h = ((gNorm - bNorm) / delta + (gNorm < bNorm ? 6 : 0)) / 6
        break
      case gNorm:
        h = ((bNorm - rNorm) / delta + 2) / 6
        break
      default:
        h = ((rNorm - gNorm) / delta + 4) / 6
        break
    }
  }

  const hue = Math.round(h * 360)
  const sat = Math.round(s * 100)
  const light = Math.round(l * 100)
  return `${hue} ${sat}% ${light}%`
}

export function mixHex(colorA: string, colorB: string, weightB: number): string {
  const ca = parseHexColor(colorA)
  const cb = parseHexColor(colorB)
  const w = Math.min(1, Math.max(0, weightB))
  const r = Math.round(ca.r * (1 - w) + cb.r * w)
  const g = Math.round(ca.g * (1 - w) + cb.g * w)
  const blue = Math.round(ca.b * (1 - w) + cb.b * w)
  return `#${[r, g, blue].map((v) => v.toString(16).padStart(2, '0')).join('')}`
}

export function lightenHex(hex: string, amount: number): string {
  return mixHex(hex, '#ffffff', amount)
}

export function darkenHex(hex: string, amount: number): string {
  return mixHex(hex, '#000000', amount)
}

/** Normalize #rgb / #rrggbb to lowercase #rrggbb. */
export function normalizeHex(hex: string): string {
  let normalized = assertValidHex(hex).toLowerCase()
  if (normalized.length === 3) {
    normalized = normalized
      .split('')
      .map((ch) => ch + ch)
      .join('')
  }
  return `#${normalized}`
}

/** True when token color should be stored as an override (strict hex !== base). */
export function shouldOverrideToken(tokenHex: string, baseHex: string): boolean {
  return normalizeHex(tokenHex) !== normalizeHex(baseHex)
}

/** Channel span 0–255. Grey is 0; a usable ANSI ink is typically ≥ 48. */
export function hexChroma(hex: string): number {
  const { r, g, b } = parseHexColor(hex)
  return Math.max(r, g, b) - Math.min(r, g, b)
}

/** Hue in degrees [0, 360). Greys return 0. */
export function hexHue(hex: string): number {
  const { r, g, b } = parseHexColor(hex)
  const rNorm = r / 255
  const gNorm = g / 255
  const bNorm = b / 255
  const max = Math.max(rNorm, gNorm, bNorm)
  const min = Math.min(rNorm, gNorm, bNorm)
  const delta = max - min
  if (delta === 0) return 0
  let hue = 0
  if (max === rNorm) {
    hue = ((gNorm - bNorm) / delta + (gNorm < bNorm ? 6 : 0)) / 6
  } else if (max === gNorm) {
    hue = ((bNorm - rNorm) / delta + 2) / 6
  } else {
    hue = ((rNorm - gNorm) / delta + 4) / 6
  }
  return hue * 360
}

export function hueDistance(a: number, b: number): number {
  const delta = Math.abs(a - b) % 360
  return delta > 180 ? 360 - delta : delta
}
