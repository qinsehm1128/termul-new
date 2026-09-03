/**
 * T-G1A — user-imported color themes.
 *
 * The dedup assertions pin the *resolved identity*, not just "resolves to
 * something": `getColorThemeDefinition` falls back to the default theme
 * silently, so an id-only check would pass on a fallback. A colliding import
 * must leave the built-in palette in place, which is what the palette
 * comparison proves.
 */
import { brandCanonical } from '@shared/brand'
import type { IpcResult } from '@shared/types/ipc.types'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  BUNDLED_COLOR_THEMES,
  getColorThemeDefinition,
  hasColorThemeDefinition,
  themePickerRows
} from './bundled-themes'
import {
  getCustomColorTheme,
  getCustomColorThemes,
  setCustomColorThemes
} from './custom-theme-registry'
import {
  exportCustomColorTheme,
  importCustomColorTheme,
  loadCustomColorThemes,
  mergeCustomColorThemes,
  parseCustomColorTheme,
  validateCustomColorTheme
} from './custom-themes'
import type { ColorThemeDefinition } from './types'

const mocks = vi.hoisted(() => ({
  read: vi.fn(),
  write: vi.fn()
}))

vi.mock('@/lib/persistence-api', () => ({
  CUSTOM_THEMES_KEY: 'themes/custom',
  persistenceApi: { read: mocks.read, write: mocks.write }
}))

function ok<T>(data: T): IpcResult<T> {
  return { success: true, data }
}

/** A minimal valid theme; `overrides` are what a real export carries. */
function validTheme(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id: 'sunset',
    name: 'Sunset',
    appearance: 'dark',
    familyId: 'sunset',
    dark: {
      palette: {
        neutral: '#101010',
        ink: '#f0f0f0',
        primary: '#ff8844',
        accent: '#ff4488',
        success: '#44ff88',
        warning: '#ffcc44',
        error: '#ff4444',
        info: '#4488ff'
      }
    },
    ...overrides
  }
}

beforeEach(() => {
  setCustomColorThemes([])
  mocks.read.mockReset()
  mocks.write.mockReset()
  mocks.write.mockResolvedValue(ok(undefined))
})

describe('validateCustomColorTheme', () => {
  it('accepts a complete theme and normalizes it', () => {
    const result = validateCustomColorTheme(validTheme())

    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(result.theme.id).toBe('sunset')
    expect(result.theme.appearance).toBe('dark')
    expect(result.theme.dark.palette.primary).toBe('#ff8844')
  })

  it('rejects a theme missing a required palette key', () => {
    const theme = validTheme()
    const palette = (theme.dark as { palette: Record<string, string> }).palette
    delete palette.info

    const result = validateCustomColorTheme(theme)

    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error.code).toBe('missing-palette-key')
    expect(result.error.field).toBe('info')
  })

  it('rejects a palette value that is not a hex color', () => {
    const theme = validTheme()
    ;(theme.dark as { palette: Record<string, string> }).palette.primary = 'rebeccapurple'

    const result = validateCustomColorTheme(theme)

    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error.code).toBe('invalid-color')
    expect(result.error.field).toBe('primary')
  })

  it('rejects an appearance outside the enum', () => {
    const result = validateCustomColorTheme(validTheme({ appearance: 'sepia' }))

    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error.code).toBe('invalid-appearance')
  })

  it('rejects an id that belongs to a bundled theme', () => {
    const result = validateCustomColorTheme(validTheme({ id: 'dracula', familyId: 'dracula' }))

    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error.code).toBe('bundled-id-conflict')
  })

  it('rejects the brand theme id, which the bundled table keys off the seam', () => {
    const brandId = brandCanonical().themeId
    const result = validateCustomColorTheme(validTheme({ id: brandId, familyId: brandId }))

    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error.code).toBe('bundled-id-conflict')
  })

  it('keeps the 8-digit alpha hex a bundled export round-trips', () => {
    const result = validateCustomColorTheme(
      validTheme({
        dark: {
          palette: (validTheme().dark as { palette: Record<string, string> }).palette,
          overrides: { 'syntax-comment': '#e4e4e45e' }
        }
      })
    )

    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(result.theme.dark.overrides?.['syntax-comment']).toBe('#e4e4e45e')
  })
})

describe('mergeCustomColorThemes', () => {
  it('drops a stored theme whose id collides with a bundled one', () => {
    const collide = { ...validTheme(), id: 'nord', familyId: 'nord' } as ColorThemeDefinition
    const own = validTheme() as unknown as ColorThemeDefinition

    expect(mergeCustomColorThemes([collide, own]).map((theme) => theme.id)).toEqual(['sunset'])
  })

  it('keeps the first of two themes sharing an id', () => {
    const first = validTheme() as unknown as ColorThemeDefinition
    const second = { ...first, name: 'Sunset 2' }

    expect(mergeCustomColorThemes([first, second])).toEqual([first])
  })
})

describe('importCustomColorTheme', () => {
  it('puts a valid theme in the picker and makes it resolvable', async () => {
    const result = await importCustomColorTheme(JSON.stringify(validTheme()))

    expect(result.ok).toBe(true)
    expect(themePickerRows().map((row) => row.themeId)).toContain('sunset')
    expect(themePickerRows().find((row) => row.themeId === 'sunset')?.source).toBe('custom')
    expect(hasColorThemeDefinition('sunset')).toBe(true)
    expect(getColorThemeDefinition('sunset').dark.palette.primary).toBe('#ff8844')
  })

  it('persists under its own key, not inside the app settings blob', async () => {
    await importCustomColorTheme(JSON.stringify(validTheme()))

    expect(mocks.write).toHaveBeenCalledWith('themes/custom', {
      themes: [expect.objectContaining({ id: 'sunset' })]
    })
  })

  it('replaces an earlier import of the same id instead of duplicating it', async () => {
    await importCustomColorTheme(JSON.stringify(validTheme()))
    await importCustomColorTheme(JSON.stringify(validTheme({ name: 'Sunset II' })))

    expect(getCustomColorThemes().map((theme) => theme.name)).toEqual(['Sunset II'])
  })

  it('refuses a colliding id and leaves the built-in theme untouched', async () => {
    const builtIn = BUNDLED_COLOR_THEMES.dracula
    const result = await importCustomColorTheme(
      JSON.stringify(validTheme({ id: 'dracula', familyId: 'dracula' }))
    )

    expect(result.ok).toBe(false)
    expect(mocks.write).not.toHaveBeenCalled()
    expect(getCustomColorTheme('dracula')).toBeUndefined()
    expect(getColorThemeDefinition('dracula')).toBe(builtIn)
    expect(getColorThemeDefinition('dracula').dark.palette.primary).toBe('#bd93f9')
  })

  it('keeps the built-in winning even if a colliding theme reaches the registry', () => {
    const builtIn = BUNDLED_COLOR_THEMES.dracula
    // Bypasses validation on purpose: this pins the resolver's own ordering,
    // which is the guard that survives a future path into the registry.
    setCustomColorThemes([
      { ...validTheme(), id: 'dracula', familyId: 'dracula' } as unknown as ColorThemeDefinition
    ])

    expect(getColorThemeDefinition('dracula')).toBe(builtIn)
  })

  it('reports invalid JSON rather than throwing', async () => {
    const result = await importCustomColorTheme('{ not json')

    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error.code).toBe('invalid-json')
  })
})

describe('export round-trip', () => {
  it('re-imports an exported theme to an equal value', async () => {
    const imported = await importCustomColorTheme(
      JSON.stringify(
        validTheme({
          dark: {
            palette: (validTheme().dark as { palette: Record<string, string> }).palette,
            overrides: { 'syntax-keyword': '#ff8844' }
          }
        })
      )
    )
    expect(imported.ok).toBe(true)
    if (!imported.ok) return

    const parsed = parseCustomColorTheme(exportCustomColorTheme(imported.theme))

    expect(parsed.ok).toBe(true)
    if (!parsed.ok) return
    expect(parsed.theme).toEqual(imported.theme)
  })
})

describe('loadCustomColorThemes', () => {
  it('installs the persisted list into the registry', async () => {
    mocks.read.mockResolvedValue(ok({ themes: [validTheme()] }))

    await loadCustomColorThemes()

    expect(getCustomColorTheme('sunset')?.name).toBe('Sunset')
  })

  it('drops a persisted theme that now collides with a bundled id', async () => {
    mocks.read.mockResolvedValue(
      ok({ themes: [{ ...validTheme(), id: 'gruvbox', familyId: 'gruvbox' }, validTheme()] })
    )

    await loadCustomColorThemes()

    expect(getCustomColorThemes().map((theme) => theme.id)).toEqual(['sunset'])
  })

  it('treats a missing key as an empty list', async () => {
    mocks.read.mockResolvedValue({ success: false, error: 'nope', code: 'KEY_NOT_FOUND' })

    await expect(loadCustomColorThemes()).resolves.toEqual([])
  })
})
