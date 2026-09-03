#!/usr/bin/env node
/**
 * Regenerate the sha256 manifests guarding the frozen legacy-brand fixture roots.
 *
 * The manifests are the structural guarantee behind "no repo-wide sed": a sed
 * that rewrote the fixtures alongside production code would break every hash,
 * and a sha256 is a hex constant containing no brand string — so the same sed
 * cannot repair it.
 *
 * Run this only when *deliberately* adding or changing a fixture. Never run it
 * to make a red manifest test go green; that red means a fixture changed, and
 * the fixtures are frozen (FORBID-03).
 *
 *   node scripts/gen-legacy-brand-manifest.mjs
 */
import { createHash } from 'node:crypto'
import { readdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, relative, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')

/** Fixture roots and the manifest each one owns. */
export const FIXTURE_ROOTS = [
  'src/__fixtures__/legacy-brand',
  'src-tauri/tests/fixtures/legacy-brand'
]

export const MANIFEST_NAME = 'MANIFEST.sha256'

/** Every fixture file under `root`, as repo-relative POSIX paths, sorted. */
export function listFixtureFiles(root) {
  const absoluteRoot = resolve(repoRoot, root)
  const found = []
  const walk = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true }).sort((a, b) =>
      a.name < b.name ? -1 : 1
    )) {
      const absolute = join(dir, entry.name)
      if (entry.isDirectory()) {
        walk(absolute)
        continue
      }
      if (entry.name === MANIFEST_NAME) continue
      found.push(relative(absoluteRoot, absolute).split(sep).join('/'))
    }
  }
  walk(absoluteRoot)
  return found.sort()
}

export function hashFixture(root, relativePath) {
  const bytes = readFileSync(resolve(repoRoot, root, relativePath))
  return createHash('sha256').update(bytes).digest('hex')
}

/** `<sha256>  <path>` lines, sorted by path — the exact committed manifest text. */
export function renderManifest(root) {
  return `${listFixtureFiles(root)
    .map((relativePath) => `${hashFixture(root, relativePath)}  ${relativePath}`)
    .join('\n')}\n`
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  for (const root of FIXTURE_ROOTS) {
    const target = resolve(repoRoot, root, MANIFEST_NAME)
    writeFileSync(target, renderManifest(root))
    console.log(`wrote ${relative(repoRoot, target)}`)
  }
}
