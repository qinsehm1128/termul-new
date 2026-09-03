/**
 * T-H12 — the environment-variable surface, against the frozen inventory and
 * against the settings UI that documents it.
 *
 * Environment variables are the widest brand-bearing contract in the repo and
 * the one with the least type safety: 65 names spread across Rust, TypeScript,
 * shell installers and bats suites, every one of them a bare string that
 * nothing resolves, imports or checks. A rename that misses one produces no
 * error anywhere — the variable is simply never read again, and the feature it
 * gated silently stops being configurable.
 *
 * `src-tauri/tests/fixtures/legacy-brand/env-names.txt` is the frozen
 * pre-rename inventory of all 65, sha256-guarded and never edited — it still
 * spells every name at the legacy prefix, and `inventoryAt` is what re-prefixes
 * it for comparison. It is deliberately raw: the entries that read back as
 * `SE_ACP_`, `SE_PLAN_` and `SE_EXIT__10____SE_EXIT__20__` are fragments the
 * scan sees because the source builds those names by interpolation, and the
 * `SE_EXIT__*` family are terminal exit markers rather than variables at all.
 * They stay exactly as captured; a "tidied" inventory would be an inventory of
 * what someone thought was there.
 *
 * Three checks, in the order they depend on each other:
 *
 * 1. The live scan reproduces the frozen inventory at the *shipped* canonical
 *    prefix. This is what makes check 2's verdict trustworthy — without it, a
 *    red could just as easily mean the scanner broke.
 * 2. With the post-rename prefix injected through the brand seam, the same scan
 *    must still find all 65. This was registered as a red with `it.fails()`
 *    until T-A11 moved them; the marker came off in that same commit. It stays
 *    green only while every one of the 65 is spelled at the new prefix — a
 *    later edit that drops one, or that quietly reverts the seam in
 *    `brand.ts` while leaving the tree alone, puts it back to red.
 * 3. Every env name the settings UI documents must be one the code actually
 *    spells. A name that survives only in help text is R-07: the user is told
 *    to set a variable nothing reads.
 *
 * No env name is written down in this file. Both sides are read from disk, and
 * the expected set for check 2 is *computed* from the frozen fixture rather
 * than transcribed, so the sed that would break the contract cannot also
 * rewrite the assertion.
 */
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { extname, join, relative, sep } from 'node:path'
import {
  __resetBrandCanonicalOverride,
  __setBrandCanonicalOverride,
  brandCanonical,
  LEGACY
} from '@shared/brand'
import { afterEach, describe, expect, it } from 'vitest'

const repoRoot = process.cwd()
const read = (relativePath: string): string => readFileSync(join(repoRoot, relativePath), 'utf8')

const FIXTURE = 'src-tauri/tests/fixtures/legacy-brand/env-names.txt'
const LOCALE_FILES = [
  'src/renderer/locales/en/settings.json',
  'src/renderer/locales/zh-CN/settings.json'
]

/** Roots the frozen inventory was captured from. */
const SCAN_ROOTS = ['src-tauri/src', 'src-tauri/tests', 'src', 'scripts', 'vite.config.web.ts']

/** Text file types that can carry an env name: Rust, TS, JSON, shell, bats. */
const SCANNED_EXTENSIONS = new Set([
  '.rs',
  '.ts',
  '.tsx',
  '.js',
  '.mjs',
  '.cjs',
  '.json',
  '.sh',
  '.bash',
  '.bats',
  '.yml',
  '.yaml',
  '.toml'
])

/**
 * Files that hold a brand string by charter rather than by use.
 *
 * The two brand seams *define* the prefix, so counting their occurrences would
 * make the prefix its own first citizen. This test file is excluded for the
 * same reason a parity test may not be its own subject.
 */
const NON_PARTICIPATING_FILES = new Set([
  'src/shared/brand.ts',
  'src-tauri/src/brand.rs',
  'scripts/tests/env-name-parity.test.ts'
])

/** The frozen fixture roots — read by this test, never scanned by it. */
const FROZEN_FIXTURE_MARKER = 'fixtures/legacy-brand'

/**
 * Third-party identifiers that collide with a candidate brand prefix.
 *
 * `SE_FILE_OBJECT` is a `windows_sys` constant used by the Windows ACL check in
 * `src-tauri/src/web/auth.rs`. It is not ours and never becomes an env name, but
 * it does start with `SE_`. Listing it here rather than letting it pollute the
 * scan keeps the post-rename comparison honest; the test below proves the
 * exclusion is still earned, so it cannot rot into a place to hide a real name.
 */
const FOREIGN_PREFIX_COLLISIONS = ['SE_FILE_OBJECT']

/**
 * The post-rename prefix, written out here rather than read from the seam.
 *
 * Before T-A11 this was what the harness injected to force a real red; it now
 * pins the value the flip was supposed to land on, which a check that reads
 * `brandCanonical()` cannot do.
 */
const CANDIDATE_ENV_PREFIX = 'SE_'

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

/** Every scannable file under `SCAN_ROOTS`, as repo-relative POSIX paths. */
function listScannedFiles(): string[] {
  const found: string[] = []
  const walk = (dir: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const absolute = join(dir, entry.name)
      const relativePath = relative(repoRoot, absolute).split(sep).join('/')
      if (relativePath.includes(FROZEN_FIXTURE_MARKER)) continue
      if (entry.isDirectory()) {
        if (entry.name !== 'node_modules') walk(absolute)
        continue
      }
      if (!SCANNED_EXTENSIONS.has(extname(entry.name))) continue
      if (NON_PARTICIPATING_FILES.has(relativePath)) continue
      found.push(relativePath)
    }
  }
  for (const root of SCAN_ROOTS) {
    const absolute = join(repoRoot, root)
    if (statSync(absolute).isDirectory()) walk(absolute)
    else found.push(root)
  }
  return found.sort()
}

const scannedFiles = listScannedFiles()
const scannedText = new Map(scannedFiles.map((path) => [path, read(path)]))

/**
 * Occurrences of `<prefix><SCREAMING_SNAKE>` in one text.
 *
 * The leading `[^A-Za-z0-9]` guard is what makes the rule work for both
 * prefixes at once: it admits `__SE_EXIT__` (underscore before, exactly as the
 * frozen inventory captured its legacy-prefixed twin) while rejecting the `SE_`
 * buried inside `USE_…` or `RESPONSE_…`. The bare prefix itself is never a name.
 */
function namesIn(text: string, prefix: string): string[] {
  const pattern = new RegExp(`(?:^|[^A-Za-z0-9])(${escapeRegExp(prefix)}[A-Z0-9_]*)`, 'gm')
  return [...text.matchAll(pattern)].map((match) => match[1]).filter((name) => name !== prefix)
}

/** Every env name present anywhere in the scanned tree, at `prefix`. */
function namesInCode(prefix: string): Set<string> {
  const names = new Set<string>()
  for (const text of scannedText.values()) {
    for (const name of namesIn(text, prefix)) {
      if (!FOREIGN_PREFIX_COLLISIONS.includes(name)) names.add(name)
    }
  }
  return names
}

/** The frozen inventory, exactly as captured. */
function frozenInventory(): string[] {
  return read(FIXTURE)
    .split('\n')
    .filter((line) => line.trim() !== '')
}

/** The frozen inventory re-prefixed to `prefix` — the post-rename expectation. */
function inventoryAt(prefix: string): string[] {
  const legacyPrefix = LEGACY.envPrefix
  return frozenInventory()
    .map((name) => name.split(legacyPrefix).join(prefix))
    .sort()
}

const sorted = (names: Iterable<string>): string[] => [...names].sort()

afterEach(() => {
  __resetBrandCanonicalOverride()
})

describe('environment variable names', () => {
  it('scans a non-trivial tree, so the comparisons below are not vacuous', () => {
    expect(scannedFiles.length).toBeGreaterThan(500)
    expect(frozenInventory().length).toBeGreaterThan(0)
  })

  it('reproduces the frozen inventory at the shipped canonical prefix', () => {
    // Soundness guard for the check below: if this is green, the scanner sees
    // exactly the 65 names the fixture froze, so a red under an injected prefix
    // is the rename's absence and not the scanner's.
    expect(sorted(namesInCode(brandCanonical().envPrefix))).toEqual(
      inventoryAt(brandCanonical().envPrefix)
    )
  })

  it('uses the canonical env prefix for every name in the frozen inventory', () => {
    // Was RED until T-A11; the `it.fails()` marker came off in the commit that
    // moved the names. It pins the post-rename prefix as a literal rather than
    // reading it back from the seam, so it still says something the check above
    // does not: that check follows `brandCanonical()` wherever it goes, this one
    // asserts where it is supposed to have arrived.
    __setBrandCanonicalOverride({ envPrefix: CANDIDATE_ENV_PREFIX })
    const prefix = brandCanonical().envPrefix
    expect(sorted(namesInCode(prefix))).toEqual(inventoryAt(prefix))
  })

  it('keeps the foreign-collision exclusions earned', () => {
    // A stale entry here would be a place to quietly drop a real env name, so
    // each one must still be present in the tree as a third-party import.
    const stillImported = FOREIGN_PREFIX_COLLISIONS.filter((name) =>
      [...scannedText.values()].some((text) =>
        new RegExp(`use [^;]*windows_sys[^;]*\\b${escapeRegExp(name)}\\b`, 's').test(text)
      )
    )
    expect(stillImported).toEqual(FOREIGN_PREFIX_COLLISIONS)
  })
})

describe('environment variable names documented in the settings UI', () => {
  const prefix = brandCanonical().envPrefix

  /** Env names the settings help text tells a user about. */
  const documented = new Set(LOCALE_FILES.flatMap((path) => namesIn(read(path), prefix)))

  /**
   * Env names that appear inside a quoted string in production code.
   *
   * Quoting is what separates a call site from a mention: `env::var("X")` and
   * `resolve_sidecar_path("X", …)` are quoted, a `/// see X` doc comment is
   * not. Tests, bats suites and the locale bundles themselves are excluded —
   * a name that only a test spells is not a name the app reads.
   */
  const spelledInProduction = (() => {
    const quotedRun = /"((?:[^"\\]|\\.)*)"|'((?:[^'\\]|\\.)*)'|`((?:[^`\\]|\\.)*)`/g
    const names = new Set<string>()
    for (const [path, text] of scannedText) {
      if (/\.test\.tsx?$/.test(path)) continue
      if (path.startsWith('src-tauri/tests/')) continue
      if (path.endsWith('.bats')) continue
      if (path.includes('/__tests__/')) continue
      if (path.includes('/locales/')) continue
      for (const run of text.matchAll(quotedRun)) {
        for (const name of namesIn(run[1] ?? run[2] ?? run[3] ?? '', prefix)) names.add(name)
      }
    }
    return names
  })()

  it('documents at least one env name and does not count every name as read', () => {
    // Vacuity guard on both sides: an empty `documented` would make the subset
    // trivially true, and a `spelledInProduction` that swallowed the whole
    // inventory would make it unfalsifiable.
    expect(documented.size).toBeGreaterThan(0)
    expect(spelledInProduction.size).toBeLessThan(frozenInventory().length)
  })

  it('documents only env names production code actually spells', () => {
    // R-07: help text naming a variable no code reads sends the user to set
    // something with no effect, and survives a rename invisibly.
    expect(sorted([...documented].filter((name) => !spelledInProduction.has(name)))).toEqual([])
  })
})
