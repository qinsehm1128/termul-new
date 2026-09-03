/**
 * T-H07 — the two `localStorage` namespaces must survive the rename.
 *
 * The keys are replayed from a captured `localStorage` dump in
 * `src/__fixtures__/legacy-brand/` rather than written by the test, because a
 * test that writes `termul-store:…` and then reads it back is asserting
 * `STORAGE_PREFIX` against a copy of itself: one repo-wide sed rewrites the
 * write, the read and the constant together, the suite stays green, and every
 * web user loses their projects, editor layout and panel sizes on the next
 * reload. The dump is sha256-frozen
 * (`src/__fixtures__/legacy-brand-manifest.test.ts`), so the sides cannot move
 * together.
 *
 * Two namespaces live in the dump and both are covered:
 * - `termul-store:<ns>::<key>` — written by the `Store` stub in this module.
 * - `termul:<key>` — ad-hoc renderer keys. `readPersistedPanelSize` is the real
 *   read path behind `usePersistedPanelSize`, which is how `WorkspaceLayout`
 *   and `ResizableRail` load every one of the bare keys in the dump.
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
import { readPersistedPanelSize } from '@/hooks/use-persisted-panel-size'
import { Store } from './plugin-store'

const FIXTURE = join(process.cwd(), 'src/__fixtures__/legacy-brand/localstorage-dump.json')

/** The post-rename namespaces the persisted keys must remain reachable under. */
const CANONICAL_OVERRIDE = { storagePrefix: 'se-store:', storageKeyPrefix: 'se:' }

interface NamespacedEntry {
  namespace: string
  key: string
  /** The raw JSON text as it sits in `localStorage`. */
  raw: string
}

/** Replay a pre-rename install's `localStorage` into the jsdom one. */
function loadDump(): Record<string, string> {
  const dump = JSON.parse(readFileSync(FIXTURE, 'utf8')) as Record<string, string>
  for (const [key, value] of Object.entries(dump)) localStorage.setItem(key, value)
  return dump
}

/** Keys the `Store` stub owns, split back into `(namespace, key)`. */
function namespacedEntries(dump: Record<string, string>): NamespacedEntry[] {
  return Object.keys(dump)
    .filter((stored) => stored.startsWith(LEGACY.storagePrefix))
    .map((stored) => {
      const rest = stored.slice(LEGACY.storagePrefix.length)
      const delimiter = rest.indexOf('::')
      return {
        namespace: rest.slice(0, delimiter),
        key: rest.slice(delimiter + 2),
        raw: dump[stored]
      }
    })
}

/** Ad-hoc renderer keys (`termul:<key>`), which carry no `::` namespace. */
function bareKeys(dump: Record<string, string>): string[] {
  return Object.keys(dump).filter((stored) => stored.startsWith(LEGACY.storageKeyPrefix))
}

/** Keys neither namespace owns — the stub must leave them alone. */
function foreignKeys(dump: Record<string, string>): string[] {
  return Object.keys(dump).filter(
    (stored) =>
      !stored.startsWith(LEGACY.storagePrefix) && !stored.startsWith(LEGACY.storageKeyPrefix)
  )
}

afterEach(() => {
  __resetBrandCanonicalOverride()
})

describe('persisted localStorage namespaces across the rename', () => {
  it('still reads back every key a pre-rename install left behind', async () => {
    // Green today, and that is the point: it goes red the moment STORAGE_PREFIX
    // is renamed without a compatibility read behind it.
    __setBrandCanonicalOverride(CANONICAL_OVERRIDE)
    const dump = loadDump()
    const entries = namespacedEntries(dump)
    expect(entries.length).toBeGreaterThan(0)

    for (const entry of entries) {
      const store = await Store.load(entry.namespace)
      expect(await store.get(entry.key)).toEqual(JSON.parse(entry.raw))
    }

    // The prefix scan must see exactly its own namespace — not the bare
    // `termul:` keys, and not a third party's.
    for (const namespace of new Set(entries.map((entry) => entry.namespace))) {
      const store = await Store.load(namespace)
      const expected = entries
        .filter((entry) => entry.namespace === namespace)
        .map((entry) => entry.key)
        .sort()
      expect((await store.keys()).sort()).toEqual(expected)
    }

    for (const foreign of foreignKeys(dump)) {
      expect(localStorage.getItem(foreign)).toBe(dump[foreign])
    }

    const bare = bareKeys(dump)
    expect(bare.length).toBeGreaterThan(0)
    for (const legacyKey of bare) {
      expect(readPersistedPanelSize(legacyKey, 0, 0, 10_000)).toBe(
        Number.parseInt(dump[legacyKey], 10)
      )
    }
  })

  // LEDGER (Wave 4) — expected failure. `STORAGE_PREFIX` is a hardcoded
  // 'termul-store:' and the `Store` stub never consults `brandCanonical()`, so
  // a value written today still lands in the legacy namespace. Delete this
  // test, `.fails` and all, once the stub writes the canonical prefix (and
  // reads the legacy one).
  test.fails('writes new values into the post-rename namespace', async () => {
    __setBrandCanonicalOverride(CANONICAL_OVERRIDE)
    const dump = loadDump()
    const sample = namespacedEntries(dump)[0]
    const probeKey = `${sample.key}-brand-probe`
    const suffix = `${sample.namespace}::${probeKey}`

    const store = await Store.load(sample.namespace)
    await store.set(probeKey, JSON.parse(sample.raw))

    expect(localStorage.getItem(`${brandCanonical().storagePrefix}${suffix}`)).not.toBeNull()
    expect(localStorage.getItem(`${brandCanonical().storagePrefix}${suffix}`)).toBe(sample.raw)
    // FORBID-04: nothing may re-emit a legacy brand string.
    expect(localStorage.getItem(`${LEGACY.storagePrefix}${suffix}`)).toBeNull()
  })

  // LEDGER (Wave 4) — expected failure. The bare renderer keys are hardcoded
  // at their call sites (`WorkspaceLayout`, `FileExplorer`), so after the flip
  // the persisted sidebar / explorer / rail sizes are unreachable and every
  // panel snaps back to its default. Delete this test, `.fails` and all, once
  // a legacy bare key resolves under its canonical name — whether that is a
  // fallback inside the read path or a boot migration this test then invokes.
  test.fails('keeps every persisted bare key reachable under its canonical name', async () => {
    __setBrandCanonicalOverride(CANONICAL_OVERRIDE)
    const dump = loadDump()

    for (const legacyKey of bareKeys(dump)) {
      const canonicalKey = `${brandCanonical().storageKeyPrefix}${legacyKey.slice(
        LEGACY.storageKeyPrefix.length
      )}`
      expect(readPersistedPanelSize(canonicalKey, 0, 0, 10_000)).toBe(
        Number.parseInt(dump[legacyKey], 10)
      )
    }
  })
})
