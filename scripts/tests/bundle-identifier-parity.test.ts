/**
 * T-A22 — the bundle identifier, from `tauri.conf.json` down to every consumer.
 *
 * # Why this exists
 *
 * Tauri derives `app_data_dir()`, the log directory, the Preferences plist, the
 * WebKit store and the TCC identity from one value: `tauri.conf.json` →
 * `identifier`. Renaming it moves seven macOS roots, three Linux roots and two
 * Windows roots at once and resets every privacy grant, and none of that
 * announces itself — the app simply starts up looking at an empty tree.
 *
 * The consumers that used to carry their own copy of the identifier are exactly
 * the ones that could not notice: `mobile-host-probe` hunts for
 * `remote-tunnel/secrets.json` under a hardcoded path, and the macOS privacy
 * pane renders whatever the Rust probe reports. A stale copy in either one is
 * silent — the probe just finds no secrets, and the pane just shows a string
 * nobody cross-checks.
 *
 * So every assertion here reads the identifier from the **config** and computes
 * the consumer's value from it. The comparison has two independent sources on
 * purpose: `expect(CONST, 'same literal')` cannot fail, and a repo-wide sed
 * rewrites both sides of a literal-vs-literal check and stays green.
 *
 * `LEGACY.bundleId` stays where it is forever — `legacy_appdata::carry_forward`
 * reads it to find the pre-rename tree — so the residual scan below rejects the
 * legacy spelling only in files that are *not* a compatibility read.
 */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { brandCanonical, LEGACY } from '@shared/brand'
import { describe, expect, it } from 'vitest'
import { appDataCandidates } from '../app-data-roots'

const repoRoot = process.cwd()
const read = (relativePath: string): string => readFileSync(join(repoRoot, relativePath), 'utf8')

const BASE_CONFIG = 'src-tauri/tauri.conf.json'
const PROD_CONFIG = 'src-tauri/tauri.conf.prod.json'
const DEV_CONFIG = 'src-tauri/tauri.conf.dev.json'
const RUST_BRAND = 'src-tauri/src/brand.rs'
const HOMEBREW = 'scripts/release/homebrew.sh'

/** The one value Tauri turns into every per-user root. */
function configIdentifier(relativePath: string): string {
  const identifier = (JSON.parse(read(relativePath)) as { identifier?: unknown }).identifier
  expect(typeof identifier, `${relativePath} declares no identifier`).toBe('string')
  return identifier as string
}

/**
 * A `DEFAULT_CANONICAL` field from `brand.rs`, so the Rust half is checked from
 * the TypeScript suite. `LEGACY` is declared first in that file, hence the slice
 * — matching on the whole file would read the legacy value and pass for the
 * wrong reason once the two spellings differ.
 */
function rustCanonical(field: string): string {
  const source = read(RUST_BRAND)
  const canonicalBlock = source.slice(source.indexOf('pub const DEFAULT_CANONICAL'))
  expect(canonicalBlock, `${RUST_BRAND} declares no DEFAULT_CANONICAL`).not.toBe('')
  const match = canonicalBlock.match(new RegExp(`^\\s*${field}: "([^"]+)",`, 'm'))
  expect(match, `${RUST_BRAND} DEFAULT_CANONICAL has no ${field}`).not.toBeNull()
  return (match as RegExpMatchArray)[1]
}

describe('bundle identifier parity', () => {
  describe('the config is the single upstream', () => {
    it('brand.ts carries the identifier tauri.conf.json actually ships', () => {
      expect(brandCanonical().bundleId).toBe(configIdentifier(BASE_CONFIG))
      expect(brandCanonical().bundleIdDev).toBe(configIdentifier(DEV_CONFIG))
    })

    it('the prod overlay does not disagree with the base config', () => {
      // `bun run build` merges tauri.conf.prod.json over the base. An overlay
      // that redeclared a different identifier would ship release builds under
      // a root no other surface computes.
      expect(configIdentifier(PROD_CONFIG)).toBe(configIdentifier(BASE_CONFIG))
    })

    it('brand.rs and brand.ts name the same two identifiers', () => {
      // The Rust half resolves app_data_dir and the TypeScript half resolves
      // everything the renderer and the scripts touch. Drift between them is a
      // renderer pointing at a root the backend never writes.
      expect(rustCanonical('bundle_id')).toBe(brandCanonical().bundleId)
      expect(rustCanonical('bundle_id_dev')).toBe(brandCanonical().bundleIdDev)
    })

    it('keeps the two install channels distinct', () => {
      // prod and dev are two installs a user may hold at once, with genuinely
      // different contents. Collapsing them would let a dev experiment merge
      // into real user data.
      expect(brandCanonical().bundleId).not.toBe(brandCanonical().bundleIdDev)
    })
  })

  describe('consumers derive rather than copy', () => {
    it('mobile-host-probe searches the roots the config actually produces', () => {
      const identifier = configIdentifier(BASE_CONFIG)
      const identifierDev = configIdentifier(DEV_CONFIG)
      const home = '/Users/demo'

      expect(appDataCandidates(home)).toEqual([
        join(home, 'Library', 'Application Support', identifierDev),
        join(home, 'Library', 'Application Support', identifier),
        join(home, '.local', 'share', identifierDev),
        join(home, '.local', 'share', identifier)
      ])
    })

    it('spells no bundle identifier literally outside the brand modules', () => {
      // The consumer surfaces, and both spellings. The legacy one matters as
      // much as the current one: a stale literal that happens to be right today
      // is the same defect one rename later.
      const consumers = [
        'scripts/app-data-roots.ts',
        'scripts/mobile-host-probe.ts',
        'scripts/mobile-host-probe.check.ts',
        'src/renderer/lib/macos-permissions-api.ts',
        'src/renderer/lib/__tests__/macos-permissions-api.web.test.ts',
        'src/renderer/components/settings/MacosPermissionsSettings.tsx',
        'src/renderer/components/settings/MacosPermissionsSettings.test.tsx'
      ]
      const forbidden = [
        brandCanonical().bundleId,
        brandCanonical().bundleIdDev,
        LEGACY.bundleId,
        LEGACY.bundleIdDev
      ]

      const offenders: string[] = []
      for (const relative of consumers) {
        const source = read(relative)
        for (const value of forbidden) {
          if (source.includes(value)) offenders.push(`${relative} :: ${value}`)
        }
      }
      expect(offenders).toEqual([])
    })
  })

  describe('uninstall reaches the pre-rename roots too', () => {
    it('the Homebrew zap list names every root under both identifiers', () => {
      // Migration copies and never deletes, so a machine that ran the app on
      // both sides of the rename holds two complete sets of per-user roots.
      // `zap` is the one explicitly user-requested deletion, and naming only
      // the current identifier would strand the older set forever.
      const entries = [...read(HOMEBREW).matchAll(/^\s*"(~\/Library\/[^"]+)",$/gm)].map(
        (match) => match[1]
      )
      expect(entries.length, `${HOMEBREW} declares no zap trash entries`).toBeGreaterThan(0)

      const canonical = brandCanonical().bundleId
      const current = entries.filter((entry) => entry.includes(canonical))
      expect(current.length, `${HOMEBREW} names no root under ${canonical}`).toBeGreaterThan(0)

      // The legacy set is *computed* from the current one, so a root added to
      // one identifier and forgotten on the other is a red rather than a
      // silently half-covered uninstall.
      const expectedLegacy = current
        .map((entry) => entry.replaceAll(canonical, LEGACY.bundleId))
        .sort()
      expect(
        [...new Set(entries)].filter((entry) => entry.includes(LEGACY.bundleId)).sort()
      ).toEqual(expectedLegacy)
    })
  })
})
