/**
 * User-imported color themes: JSON import / export, validation, dedup, and
 * persistence.
 *
 * Shaped after the custom-agent store (`lib/agents/custom-agents.ts`): a thin
 * persisted list merged *behind* the built-ins, where a bundled id always wins
 * so a stored file can never shadow a shipped theme.
 *
 * That rule is enforced three times over, deliberately:
 *   1. {@link validateCustomColorTheme} rejects a colliding id at import, so
 *      the user gets an error instead of a silently ignored file;
 *   2. {@link mergeCustomColorThemes} drops one that is already on disk (from
 *      an older build, or a theme id the brand seam has since claimed);
 *   3. `getColorThemeDefinition` probes the bundled table first, so even a
 *      registry populated by some future path cannot win the lookup.
 */

import { runtimeT } from '@/i18n/runtime'
import { CUSTOM_THEMES_KEY, persistenceApi } from '@/lib/persistence-api'
import { hasBundledColorTheme } from './bundled-themes'
import { getCustomColorThemes, setCustomColorThemes } from './custom-theme-registry'
import type {
  ColorThemeDefinition,
  ThemeAppearance,
  ThemePalette,
  ThemeSyntaxOverrides
} from './types'

/**
 * Palette keys `ThemePalette` declares required. A theme missing any of them
 * would render with `undefined` in a CSS variable.
 */
const REQUIRED_PALETTE_KEYS = [
  'neutral',
  'ink',
  'primary',
  'accent',
  'success',
  'warning',
  'error',
  'info'
] as const satisfies readonly (keyof ThemePalette)[]

/** Palette keys `ThemePalette` declares optional — still colors when present. */
const OPTIONAL_PALETTE_KEYS = [
  'diffAdd',
  'diffDelete',
  'interactive'
] as const satisfies readonly (keyof ThemePalette)[]

/**
 * Syntax override keys `ThemeSyntaxOverrides` declares.
 *
 * Listed rather than derived because TypeScript types are not reachable at
 * runtime, and validation has to know the allowed set anyway: an import keeps
 * only these, so a typo'd key is dropped instead of stored forever.
 */
const SYNTAX_OVERRIDE_KEYS = [
  'syntax-comment',
  'syntax-keyword',
  'syntax-function',
  'syntax-string',
  'syntax-primitive',
  'syntax-variable',
  'syntax-property',
  'syntax-type',
  'syntax-constant',
  'syntax-operator',
  'syntax-punctuation'
] as const satisfies readonly (keyof ThemeSyntaxOverrides)[]

const THEME_APPEARANCES = ['light', 'dark'] as const satisfies readonly ThemeAppearance[]

/**
 * `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`.
 *
 * The alpha forms are not decorative: the bundled `cursor` theme's
 * `syntax-comment` is `#e4e4e45e`, so a 6-digit-only rule would reject a
 * round-tripped export of a shipped theme.
 */
const HEX_COLOR = /^#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/

export type CustomThemeErrorCode =
  | 'invalid-json'
  | 'invalid-shape'
  | 'invalid-id'
  | 'invalid-name'
  | 'invalid-appearance'
  | 'missing-palette-key'
  | 'invalid-color'
  | 'bundled-id-conflict'

export interface CustomThemeError {
  code: CustomThemeErrorCode
  /** Translated, user-facing. */
  message: string
  /** The offending key, for the codes that name one. */
  field?: string
}

export type CustomThemeResult =
  | { ok: true; theme: ColorThemeDefinition }
  | { ok: false; error: CustomThemeError }

function fail(code: CustomThemeErrorCode, message: string, field?: string): CustomThemeResult {
  return { ok: false, error: { code, message, field } }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === 'string' && value.trim().length > 0
}

/**
 * Copy the declared palette keys across, dropping anything else.
 *
 * `missing` names the first required key the source does not carry as a
 * string; the palette is only handed back once every one of them was assigned,
 * which is the invariant behind the closing assertion.
 */
function readPalette(
  source: Record<string, unknown>
): { palette: ThemePalette; missing?: never } | { palette?: never; missing: keyof ThemePalette } {
  const values: Partial<ThemePalette> = {}
  for (const key of REQUIRED_PALETTE_KEYS) {
    const value = source[key]
    if (typeof value !== 'string') return { missing: key }
    values[key] = value
  }
  for (const key of OPTIONAL_PALETTE_KEYS) {
    const value = source[key]
    if (typeof value === 'string') values[key] = value
  }
  return { palette: values as ThemePalette }
}

/** Keep only the declared syntax overrides, so a typo'd key is not stored. */
function readOverrides(source: unknown): ThemeSyntaxOverrides | undefined {
  if (!isRecord(source)) return undefined
  const overrides: ThemeSyntaxOverrides = {}
  for (const key of SYNTAX_OVERRIDE_KEYS) {
    const value = source[key]
    if (typeof value === 'string') overrides[key] = value
  }
  return overrides
}

/**
 * Validate an imported theme and normalize it into a `ColorThemeDefinition`.
 *
 * Returns the normalized theme rather than the input: the caller stores what
 * was validated, not the raw object, so an unknown extra key can never reach
 * the registry.
 */
export function validateCustomColorTheme(input: unknown): CustomThemeResult {
  if (!isRecord(input)) {
    return fail(
      'invalid-shape',
      runtimeT('shell', 'themes.errors.invalidShape', 'Theme must be a JSON object')
    )
  }

  if (!isNonEmptyString(input.id)) {
    return fail(
      'invalid-id',
      runtimeT('shell', 'themes.errors.invalidId', 'Theme "id" must be a non-empty string'),
      'id'
    )
  }
  const id = input.id.trim()

  if (hasBundledColorTheme(id)) {
    return fail(
      'bundled-id-conflict',
      runtimeT(
        'shell',
        'themes.errors.bundledIdConflict',
        'Theme id "{{id}}" belongs to a built-in theme. Rename it and import again.',
        { id }
      ),
      'id'
    )
  }

  if (!isNonEmptyString(input.name)) {
    return fail(
      'invalid-name',
      runtimeT('shell', 'themes.errors.invalidName', 'Theme "name" must be a non-empty string'),
      'name'
    )
  }

  if (
    typeof input.appearance !== 'string' ||
    !THEME_APPEARANCES.includes(input.appearance as ThemeAppearance)
  ) {
    return fail(
      'invalid-appearance',
      runtimeT(
        'shell',
        'themes.errors.invalidAppearance',
        'Theme "appearance" must be "light" or "dark"'
      ),
      'appearance'
    )
  }
  const appearance = input.appearance as ThemeAppearance

  const variant = input.dark
  if (!isRecord(variant) || !isRecord(variant.palette)) {
    return fail(
      'invalid-shape',
      runtimeT('shell', 'themes.errors.missingPalette', 'Theme is missing its "dark.palette"'),
      'dark.palette'
    )
  }

  const read = readPalette(variant.palette)
  if (read.missing !== undefined) {
    return fail(
      'missing-palette-key',
      runtimeT('shell', 'themes.errors.missingPaletteKey', 'Palette is missing "{{key}}"', {
        key: read.missing
      }),
      read.missing
    )
  }

  const palette = read.palette
  const overrides = readOverrides(variant.overrides)

  for (const [key, value] of [...Object.entries(palette), ...Object.entries(overrides ?? {})]) {
    if (!HEX_COLOR.test(value)) {
      return fail(
        'invalid-color',
        runtimeT('shell', 'themes.errors.invalidColor', '"{{key}}" is not a hex color: {{value}}', {
          key,
          value
        }),
        key
      )
    }
  }

  const familyId = isNonEmptyString(input.familyId) ? input.familyId.trim() : id

  return {
    ok: true,
    theme: {
      id,
      name: input.name.trim(),
      appearance,
      familyId,
      dark: overrides ? { palette, overrides } : { palette }
    }
  }
}

/** Parse export JSON and validate it in one step. */
export function parseCustomColorTheme(json: string): CustomThemeResult {
  let parsed: unknown
  try {
    parsed = JSON.parse(json)
  } catch {
    return fail(
      'invalid-json',
      runtimeT('shell', 'themes.errors.invalidJson', 'File is not valid JSON')
    )
  }
  return validateCustomColorTheme(parsed)
}

/** The JSON text an export writes; {@link parseCustomColorTheme} reads it back. */
export function exportCustomColorTheme(theme: ColorThemeDefinition): string {
  return JSON.stringify(theme, null, 2)
}

/**
 * Bundled ids always win: a stored theme sharing one is dropped rather than
 * merged, and a later duplicate of an earlier custom id loses to the first.
 *
 * The bundled id set comes from `hasBundledColorTheme`, which is derived from
 * `BUNDLED_COLOR_THEMES`' own keys plus the brand-seam aliases — so there is no
 * second list to keep in step with the table.
 */
export function mergeCustomColorThemes(
  custom: readonly ColorThemeDefinition[]
): ColorThemeDefinition[] {
  const seen = new Set<string>()
  const merged: ColorThemeDefinition[] = []
  for (const theme of custom) {
    if (hasBundledColorTheme(theme.id) || seen.has(theme.id)) continue
    seen.add(theme.id)
    merged.push(theme)
  }
  return merged
}

interface PersistedCustomThemes {
  themes: ColorThemeDefinition[]
}

/** Load persisted custom themes into the registry (empty on first run). */
export async function loadCustomColorThemes(): Promise<readonly ColorThemeDefinition[]> {
  const result = await persistenceApi.read<PersistedCustomThemes>(CUSTOM_THEMES_KEY)
  if (result.success && Array.isArray(result.data?.themes)) {
    const themes = mergeCustomColorThemes(result.data.themes)
    setCustomColorThemes(themes)
    return themes
  }
  if (!result.success && result.code === 'KEY_NOT_FOUND') {
    setCustomColorThemes([])
    return []
  }
  return getCustomColorThemes()
}

async function saveCustomColorThemes(themes: readonly ColorThemeDefinition[]): Promise<void> {
  const merged = mergeCustomColorThemes(themes)
  const payload: PersistedCustomThemes = { themes: merged }
  const result = await persistenceApi.write(CUSTOM_THEMES_KEY, payload)
  if (!result.success) {
    throw new Error(
      result.error || runtimeT('shell', 'themes.errors.saveFailed', 'Failed to save custom themes')
    )
  }
  setCustomColorThemes(merged)
}

/**
 * Validate, store, and register a theme from export JSON.
 *
 * Re-importing an id already in the registry replaces it, matching
 * `upsertCustomAgent`: an import is how the user edits a theme they authored.
 */
export async function importCustomColorTheme(json: string): Promise<CustomThemeResult> {
  const result = parseCustomColorTheme(json)
  if (!result.ok) return result

  const existing = getCustomColorThemes().filter((theme) => theme.id !== result.theme.id)
  await saveCustomColorThemes([...existing, result.theme])
  return result
}

/** Remove a custom theme. No-op for unknown / bundled ids. */
export async function deleteCustomColorTheme(themeId: string): Promise<void> {
  const existing = getCustomColorThemes()
  const next = existing.filter((theme) => theme.id !== themeId)
  if (next.length !== existing.length) {
    await saveCustomColorThemes(next)
  }
}
