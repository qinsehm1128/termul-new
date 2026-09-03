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
import { afterEach, describe, expect, it, test } from 'vitest'
import { isKnownColorThemeId } from './apply-color-theme'
import { DEFAULT_COLOR_THEME_ID, getColorThemeDefinition } from './bundled-themes'

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

  // LEDGER (Wave 4) — expected failure. `BUNDLED_COLOR_THEMES` is keyed by a
  // hardcoded 'termul' and neither `getColorThemeDefinition` nor
  // `isKnownColorThemeId` consults `brandCanonical()`, so a persisted dark
  // theme cannot resolve to its post-rename identity. Delete this test,
  // `.fails` and all, once a legacy id resolves to the canonical theme.
  test.fails('resolves the persisted dark theme to its post-rename identity', () => {
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

  // LEDGER (Wave 4) — expected failure. Same root cause on the light twin,
  // which the default-theme fallback can never impersonate: it is not the
  // default. Delete this test, `.fails` and all, once a legacy light id
  // resolves to the canonical light theme.
  test.fails('resolves the persisted light theme to its post-rename identity', () => {
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
