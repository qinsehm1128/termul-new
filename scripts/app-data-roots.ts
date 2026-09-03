/**
 * The desktop `app_data_dir()` roots, derived from the bundle identifier.
 *
 * Split out of `mobile-host-probe.ts` so it can be typechecked and asserted on:
 * that file uses Bun globals the repo has no types for, so importing it from a
 * test would drag `Bun.file` / `Bun.spawn` into a program that cannot see them.
 * This module is dependency-free on purpose.
 *
 * Tauri derives the directory from `tauri.conf.json` → `identifier`, so the
 * identifiers come from the brand module rather than being typed again here;
 * `scripts/tests/bundle-identifier-parity.test.ts` pins the brand module to the
 * config. A literal copy is what let the probe keep pointing at the pre-rename
 * root after the identifier moved.
 */
import { homedir } from 'node:os'
import { join } from 'node:path'
import { brandCanonical } from '../src/shared/brand'

/** macOS and Linux app-data roots, dev first so a running dev build wins. */
export function appDataCandidates(home = homedir()): string[] {
  const { bundleId, bundleIdDev } = brandCanonical()
  return [
    join(home, 'Library', 'Application Support', bundleIdDev),
    join(home, 'Library', 'Application Support', bundleId),
    join(home, '.local', 'share', bundleIdDev),
    join(home, '.local', 'share', bundleId)
  ]
}
