/**
 * T-A08 — the two `localStorage` prefixes, across the language boundary.
 *
 * The prefixes are the only brand contract carried by *both* runtimes for the
 * same bytes. `src/shared/brand.ts` decides what the renderer writes; the Rust
 * copy in `src-tauri/src/brand.rs` is what `webview_storage_handoff::
 * is_app_owned_key` matches against when it decides which `localStorage` keys
 * to carry across a bundle-identifier rename. On macOS the WebView data store
 * is partitioned by identifier and cannot be moved, so that replay is the only
 * path the data has.
 *
 * A half-flip is therefore silent and destructive in one direction only: flip
 * the TypeScript side alone and the renderer starts writing keys the Rust side
 * no longer recognises as the app's own, so the next identifier change replays
 * nothing and the user's projects, editor layout and panel sizes are gone.
 * Nothing else catches it — S-04 records that there is no whole-field-set
 * TS<->Rust parity gate, only per-contract ones, so adding or drifting a field
 * trips nothing.
 *
 * Both value groups are compared, and the legacy half matters as much as the
 * canonical one: the compatibility reads on the two sides
 * (`acceptedBrandValues` in TypeScript, the `LEGACY` entries in
 * `is_app_owned_key`) are what keep pre-rename keys reachable, and they are
 * only equivalent while the two `LEGACY` copies agree.
 *
 * Every value is extracted from its own file on disk rather than written down
 * here — a literal in this file would make the test a copy of its own subject.
 */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { __resetBrandCanonicalOverride, brandCanonical, LEGACY } from '@shared/brand'
import { afterEach, describe, expect, it } from 'vitest'

const BRAND_RS = 'src-tauri/src/brand.rs'

/** The Rust field names, paired with the TypeScript ones they mirror. */
const FIELDS = [
  { rust: 'storage_prefix', ts: 'storagePrefix' },
  { rust: 'storage_key_prefix', ts: 'storageKeyPrefix' }
] as const

/**
 * One field out of one `BrandCanonical` literal in `brand.rs`.
 *
 * Anchored at the named constant's declaration so the identically-shaped
 * `LEGACY` and `DEFAULT_CANONICAL` blocks cannot satisfy each other's lookup —
 * which is the exact confusion this test exists to catch.
 */
function rustField(constant: 'LEGACY' | 'DEFAULT_CANONICAL', field: string): string {
  const source = readFileSync(join(process.cwd(), BRAND_RS), 'utf8')
  const start = source.indexOf(`pub const ${constant}: BrandCanonical`)
  if (start < 0) throw new Error(`${constant} not found in ${BRAND_RS}`)
  const block = source.slice(start, source.indexOf('};', start))
  const match = block.match(new RegExp(`\\b${field}: "([^"]*)",`))
  if (!match) throw new Error(`${field} not found in ${BRAND_RS}'s ${constant}`)
  return match[1]
}

afterEach(() => {
  __resetBrandCanonicalOverride()
})

describe('localStorage prefix parity across TypeScript and Rust', () => {
  it('reads a non-empty value from every carrier', () => {
    // Vacuity guard: a broken extractor would otherwise compare '' to '' and
    // report a parity it never checked.
    for (const { rust, ts } of FIELDS) {
      expect(rustField('DEFAULT_CANONICAL', rust).length).toBeGreaterThan(0)
      expect(rustField('LEGACY', rust).length).toBeGreaterThan(0)
      expect(brandCanonical()[ts].length).toBeGreaterThan(0)
      expect(LEGACY[ts].length).toBeGreaterThan(0)
    }
  })

  it('agrees on the value the app writes today', () => {
    const resolved = Object.fromEntries(
      FIELDS.flatMap(({ rust, ts }) => [
        [`TypeScript ${ts}`, brandCanonical()[ts]],
        [`Rust ${rust}`, rustField('DEFAULT_CANONICAL', rust)]
      ])
    )
    // Expected side taken from TypeScript — the renderer is what performs the
    // write — and broadcast per field, so a failure names the side that drifted.
    expect(resolved).toEqual(
      Object.fromEntries(
        FIELDS.flatMap(({ rust, ts }) => [
          [`TypeScript ${ts}`, brandCanonical()[ts]],
          [`Rust ${rust}`, brandCanonical()[ts]]
        ])
      )
    )
  })

  it('agrees on the value already on users disks', () => {
    const resolved = Object.fromEntries(
      FIELDS.flatMap(({ rust, ts }) => [
        [`TypeScript LEGACY ${ts}`, LEGACY[ts]],
        [`Rust LEGACY ${rust}`, rustField('LEGACY', rust)]
      ])
    )
    expect(resolved).toEqual(
      Object.fromEntries(
        FIELDS.flatMap(({ rust, ts }) => [
          [`TypeScript LEGACY ${ts}`, LEGACY[ts]],
          [`Rust LEGACY ${rust}`, LEGACY[ts]]
        ])
      )
    )
  })

  it('keeps the two prefixes mutually non-prefixing in every spelling', () => {
    // Structural, not cosmetic. Both namespaces are separated by nothing but a
    // `startsWith` scan — `Store.keys()` walks the store prefix, the panel-size
    // reads and `is_app_owned_key` walk the bare one. If either prefix were a
    // prefix of the other, every store key would also answer a bare-key scan
    // and the two namespaces would silently merge.
    for (const spelling of [brandCanonical(), LEGACY]) {
      expect(spelling.storagePrefix.startsWith(spelling.storageKeyPrefix)).toBe(false)
      expect(spelling.storageKeyPrefix.startsWith(spelling.storagePrefix)).toBe(false)
    }
  })
})
