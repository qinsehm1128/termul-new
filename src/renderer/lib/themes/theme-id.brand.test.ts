/**
 * T-H05 — a persisted color-theme id must survive the rename.
 *
 * The theme ids are read out of persisted `app-settings` snapshots in
 * `src/__fixtures__/legacy-brand/` rather than inlined, because an inline
 * `'termul'` is a copy of the bundled theme's own key: one repo-wide sed
 * rewrites the assertion, the bundle key and `DEFAULT_COLOR_THEME_ID` together,
 * the suite stays green, and every user who picked a theme silently gets a
 * different one. The fixtures are sha256-frozen
 * (`src/__fixtures__/legacy-brand-manifest.test.ts`), so the sides cannot move
 * together.
 *
 * `getColorThemeDefinition` falls back to the default theme *silently*, so
 * "resolves to something" proves nothing. Every assertion below pins the
 * resolved identity — id and familyId — and pairs it with
 * `isKnownColorThemeId`, which is exactly the predicate the fallback branch
 * fails. A fallback therefore stays detectable even after the default theme is
 * itself renamed to the canonical id.
 */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import {
  __resetBrandCanonicalOverride,
  __setBrandCanonicalOverride,
  brandCanonical,
  LEGACY
} from '@shared/brand'
import { afterEach, describe, expect, it } from 'vitest'
import { isKnownColorThemeId } from './apply-color-theme'
import { DEFAULT_COLOR_THEME_ID, getColorThemeDefinition, themePickerRows } from './bundled-themes'

const FIXTURE_ROOT = join(process.cwd(), 'src/__fixtures__/legacy-brand')

/** The `colorTheme` a pre-rename install wrote into its app settings. */
function persistedColorTheme(fixture: string): string {
  const settings = JSON.parse(readFileSync(join(FIXTURE_ROOT, fixture), 'utf8')) as {
    colorTheme: string
  }
  return settings.colorTheme
}

const DARK_FIXTURE = 'app-settings-theme-termul.json'
const LIGHT_FIXTURE = 'app-settings-theme-termul-light.json'

/** The post-rename identity the persisted ids must migrate to. */
const CANONICAL_OVERRIDE = { themeId: 'se', themeFamilyLight: 'se-light' }

afterEach(() => {
  __resetBrandCanonicalOverride()
})

describe('persisted color theme id across the rename', () => {
  it('still recognizes both persisted ids a pre-rename install could hold', () => {
    // Green today, and that is the point: it goes red the moment the bundled
    // keys are renamed without a compatibility read behind them, which is
    // precisely the silent-fallback regression.
    __setBrandCanonicalOverride(CANONICAL_OVERRIDE)
    expect(persistedColorTheme(DARK_FIXTURE)).toBe(LEGACY.themeId)
    expect(persistedColorTheme(LIGHT_FIXTURE)).toBe(LEGACY.themeFamilyLight)

    expect(isKnownColorThemeId(persistedColorTheme(DARK_FIXTURE))).toBe(true)
    expect(isKnownColorThemeId(persistedColorTheme(LIGHT_FIXTURE))).toBe(true)
  })

  // T-A03 landed: `getColorThemeDefinition` and `isKnownColorThemeId` both go
  // through a table re-keyed from `brandCanonical()`, so the persisted dark id
  // resolves to the canonical theme instead of the silent default fallback.
  it('resolves the persisted dark theme to its post-rename identity', () => {
    __setBrandCanonicalOverride(CANONICAL_OVERRIDE)
    const persisted = persistedColorTheme(DARK_FIXTURE)

    // Guards the silent fallback: after the flip the default theme is itself
    // the canonical id, so an id-only assertion would pass on a fallback.
    expect(isKnownColorThemeId(persisted)).toBe(true)

    const resolved = getColorThemeDefinition(persisted)
    expect(resolved.id).toBe(brandCanonical().themeId)
    expect(resolved.familyId).toBe(brandCanonical().themeId)
    expect(resolved.appearance).toBe('dark')
  })

  // T-A03 landed on the light twin too — the one the default-theme fallback
  // can never impersonate, because it is not the default.
  it('resolves the persisted light theme to its post-rename identity', () => {
    __setBrandCanonicalOverride(CANONICAL_OVERRIDE)
    const persisted = persistedColorTheme(LIGHT_FIXTURE)

    expect(isKnownColorThemeId(persisted)).toBe(true)

    const resolved = getColorThemeDefinition(persisted)
    expect(resolved.id).toBe(brandCanonical().themeFamilyLight)
    expect(resolved.familyId).toBe(brandCanonical().themeId)
    expect(resolved.id).not.toBe(DEFAULT_COLOR_THEME_ID)
    expect(resolved.appearance).toBe('light')
  })
})

/**
 * The bundled tables are keyed BY the theme id, so the flip is a re-key rather
 * than a value edit — and a re-key that is skipped leaves no literal to grep
 * for. These two run against the shipped values (no seam override) because the
 * bug they catch is a disagreement between two shipped tables.
 */
describe('theme picker rows agree with the resolver', () => {
  it('carries a self-resolving id on every row', () => {
    // `ThemePicker` highlights the row whose `themeId` equals the effective
    // theme id derived from the persisted setting, and that setting is
    // normalized through the resolver. A row keyed by a spelling the resolver
    // renames is a row that can never match — the active theme would show no
    // highlight at all.
    for (const row of themePickerRows()) {
      expect(getColorThemeDefinition(row.themeId).id).toBe(row.themeId)
    }
  })

  it('resolves both persisted ids onto ids the picker actually offers', () => {
    const rowIds = themePickerRows().map((row) => row.themeId)

    expect(rowIds).toContain(getColorThemeDefinition(persistedColorTheme(DARK_FIXTURE)).id)
    expect(rowIds).toContain(getColorThemeDefinition(persistedColorTheme(LIGHT_FIXTURE)).id)
  })
})
