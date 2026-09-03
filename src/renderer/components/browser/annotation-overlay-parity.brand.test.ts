/**
 * T-H10 — the annotation overlay's `window.__se_*` bridge, checked across
 * the two files that actually form it.
 *
 * `src-tauri/resources/annotation-overlay.js` defines the globals; the Rust in
 * `src-tauri/src/browser_tab_manager.rs` injects that file and then calls back
 * into those same globals from *separate* `webview.eval` strings. The two
 * halves are plain text in two different languages with no compiler, no type
 * system and no import edge between them — a rename on one side is invisible
 * to every existing test.
 *
 * Both sides are therefore read from disk and the names are *extracted*, never
 * written down here. A test that spelled `__se_render_markers` out as a
 * literal would be rewritten by the same repo-wide sed that broke the bridge,
 * and would stay green through the breakage. The only brand string this file
 * knows is the one it asks the brand seam for.
 *
 * Strict set equality between the two files does not hold and should not: the
 * Rust owns `__se_poller` end to end (its own injected snippet writes and
 * reads it, the overlay never sees it) and the overlay owns
 * `__se_remove_markers` end to end (it exports it and calls it itself).
 * What must hold is *closure*: no global may be referenced on one side without
 * an owner on the other, in either direction. The last three assertions below
 * are that property, and each goes red when a single name is changed on a
 * single side.
 */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { __resetBrandCanonicalOverride, brandCanonical } from '@shared/brand'
import { afterEach, describe, expect, it } from 'vitest'

const repoRoot = process.cwd()
const read = (relativePath: string): string => readFileSync(join(repoRoot, relativePath), 'utf8')

const OVERLAY_JS = 'src-tauri/resources/annotation-overlay.js'
const TAB_MANAGER_RS = 'src-tauri/src/browser_tab_manager.rs'

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

interface GlobalUsage {
  /** Names appearing as the target of `window.<name> = …`. */
  readonly written: ReadonlySet<string>
  /** Names appearing on `window` in any other position (call, guard, delete). */
  readonly read: ReadonlySet<string>
  /** Every name seen, regardless of position. */
  readonly all: ReadonlySet<string>
}

/**
 * Classify every `window.<brand-prefixed>` occurrence in one source text.
 *
 * The optional `=` group deliberately rejects `==`/`===`/`=>` so a comparison
 * (`window.__se_annotation_mode === 'select'`) is counted as a read rather
 * than as a definition.
 */
function scanWindowGlobals(source: string, prefix: string): GlobalUsage {
  const pattern = new RegExp(`window\\.(${escapeRegExp(prefix)}[A-Za-z0-9_]*)\\s*(=(?![=>]))?`, 'g')
  const written = new Set<string>()
  const read = new Set<string>()
  const all = new Set<string>()
  for (const match of source.matchAll(pattern)) {
    const name = match[1]
    all.add(name)
    if (match[2]) written.add(name)
    else read.add(name)
  }
  return { written, read, all }
}

/**
 * Every `__lowercase` identifier in a source text, whatever its role — window
 * global, element id, CSS class. Tauri's own `__TAURI__` /
 * `__TAURI_INTERNALS__` are excluded by the lowercase first character.
 */
function doubleUnderscoreIdentifiers(source: string): Set<string> {
  return new Set(source.match(/__[a-z][A-Za-z0-9_]*/g) ?? [])
}

const sorted = (names: Iterable<string>): string[] => [...names].sort()

afterEach(() => {
  __resetBrandCanonicalOverride()
})

describe('annotation overlay ↔ browser_tab_manager global-name parity', () => {
  const overlaySource = read(OVERLAY_JS)
  const rustSource = read(TAB_MANAGER_RS)
  const prefix = brandCanonical().domGlobalPrefix

  const overlay = scanWindowGlobals(overlaySource, prefix)
  const rust = scanWindowGlobals(rustSource, prefix)

  it('finds globals on both sides at the canonical prefix', () => {
    // Vacuity guard. Every assertion below compares two extracted sets; if the
    // prefix stopped matching, both sets would be empty and the comparisons
    // would pass while checking nothing.
    expect(overlay.all.size).toBeGreaterThan(0)
    expect(rust.all.size).toBeGreaterThan(0)
  })

  it('spells every double-underscore identifier at the canonical prefix', () => {
    // Covers the names the window scan cannot see — element ids
    // (`__se_annotation_layer`), the marker CSS class, and the local
    // snapshot variables. A half-finished rename leaves some at the old prefix
    // and shows up here as a non-empty list.
    const offenders = sorted(
      [
        ...doubleUnderscoreIdentifiers(overlaySource),
        ...doubleUnderscoreIdentifiers(rustSource)
      ].filter((name) => !name.startsWith(prefix))
    )
    expect(offenders).toEqual([])
  })

  it('never lets the Rust read a global the overlay does not export', () => {
    // `remove_annotation_overlay`, `render_markers`, `update_marker_selection`:
    // the Rust calls them but never defines them, so the overlay must.
    const readOnlyInRust = sorted([...rust.read].filter((name) => !rust.written.has(name)))
    const missingFromOverlay = readOnlyInRust.filter((name) => !overlay.written.has(name))
    expect(readOnlyInRust.length).toBeGreaterThan(0)
    expect(missingFromOverlay).toEqual([])
  })

  it('never lets the Rust write a global nothing on the overlay side reads', () => {
    // `annotation_mode` / `annotation_tab_id`: the Rust bootstrap sets them
    // immediately before the overlay source runs, and the overlay reads them.
    const writeOnlyInRust = sorted([...rust.written].filter((name) => !rust.read.has(name)))
    const unreadByOverlay = writeOnlyInRust.filter((name) => !overlay.all.has(name))
    expect(writeOnlyInRust.length).toBeGreaterThan(0)
    expect(unreadByOverlay).toEqual([])
  })

  it('never lets the overlay export a global no caller on either side uses', () => {
    // The reverse direction. An export nobody calls is either dead code or a
    // bridge whose far end was renamed; `remove_markers` survives this because
    // the overlay itself calls it.
    const exported = sorted(overlay.written)
    const uncalled = exported.filter((name) => !rust.read.has(name) && !overlay.read.has(name))
    expect(exported.length).toBeGreaterThan(0)
    expect(uncalled).toEqual([])
  })
})
