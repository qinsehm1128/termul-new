/**
 * T-H14 — the three log paths published to users vs. the code that produces them.
 *
 * `.github/ISSUE_TEMPLATE/bug_report.yml` tells a reporter exactly where to
 * find the log file on each platform. Those paths are prose in a YAML file:
 * nothing compiles them, nothing imports them, and no existing test reads them.
 * The values they encode are owned by two other files entirely —
 * `src-tauri/src/logging.rs` names the log file (`LOG_FILE_NAME`) and
 * `src-tauri/tauri.conf.json` names the directory (`identifier`, which Tauri's
 * `app_log_dir()` interpolates per platform).
 *
 * A rename that touched the code and not the issue template would send every
 * bug reporter to a path that does not exist, and nothing would notice. So both
 * sides are read from disk and one is *computed* from the other: the template
 * is parsed for its three paths, and the three expected paths are assembled
 * from `log_file_name`, the `{}.log` template in `log_file_path`, and the
 * bundle identifier. No path in this file is written down as a literal.
 *
 * T-M04 moved the name itself into `src-tauri/src/brand.rs` — `logging.rs` now
 * reads `brand::canonical().log_file_name` rather than holding a literal, so a
 * Wave-5 flip is a one-line edit there. This file follows that indirection
 * instead of short-circuiting it: the expected name is read from `brand.rs`,
 * and `logging.rs` is checked to still be reading the seam. Skipping that
 * second check would let the two agree with each other while the shipped binary
 * wrote a third name entirely.
 *
 * The only thing this file does encode is the *shape* of `app_log_dir()` per
 * platform. That is Tauri's own platform convention, carries no brand string,
 * and cannot be rewritten by a rename of this app.
 */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

const repoRoot = process.cwd()
const read = (relativePath: string): string => readFileSync(join(repoRoot, relativePath), 'utf8')

const BUG_REPORT_YML = '.github/ISSUE_TEMPLATE/bug_report.yml'
const BRAND_RS = 'src-tauri/src/brand.rs'
const LOGGING_RS = 'src-tauri/src/logging.rs'
const TAURI_CONF = 'src-tauri/tauri.conf.json'

/**
 * `log_file_name: "…"` from `DEFAULT_CANONICAL` — the base name, no extension.
 *
 * Scoped to that struct literal specifically: `LEGACY` carries a
 * `log_file_name` of its own and is a permanent record of what is already on
 * user disks, so matching the first occurrence in the file would pin the
 * published path to the pre-rename value forever.
 */
function logFileName(): string {
  const source = read(BRAND_RS)
  const block = source.match(/DEFAULT_CANONICAL: BrandCanonical = BrandCanonical \{([\s\S]*?)\n\};/)
  if (!block) throw new Error(`DEFAULT_CANONICAL not found in ${BRAND_RS}`)
  const match = block[1].match(/^\s*log_file_name: "([^"]+)",$/m)
  if (!match) throw new Error(`log_file_name not found in DEFAULT_CANONICAL in ${BRAND_RS}`)
  return match[1]
}

/**
 * The extension `log_file_path` appends, taken from the `format!` that builds
 * it rather than assumed: `format!("{}.log", log_file_name())`.
 */
function logFileExtension(): string {
  const match = read(LOGGING_RS).match(/format!\("\{\}(\.[a-z]+)", log_file_name\(\)\)/)
  if (!match) throw new Error(`log_file_path format! not found in ${LOGGING_RS}`)
  return match[1]
}

/**
 * Whether `logging.rs` still gets the name from the brand seam.
 *
 * The parity above compares `brand.rs` with the issue template. That comparison
 * only says something about the shipped app while `logging.rs` is the seam's
 * consumer — reintroducing a literal there would leave both sides of the
 * comparison agreeing on a name nothing writes.
 */
function loggingReadsTheBrandSeam(): boolean {
  return /fn log_file_name\(\) -> &'static str \{\s*brand::canonical\(\)\.log_file_name\s*\}/.test(
    read(LOGGING_RS)
  )
}

/** The bundle identifier Tauri interpolates into every per-app OS directory. */
function bundleIdentifier(): string {
  return (JSON.parse(read(TAURI_CONF)) as { identifier: string }).identifier
}

/**
 * `- <OS>: \`<path>\`` bullets from the template's Logs field, keyed by OS.
 *
 * Parsed generically rather than by line number so reflowing the template does
 * not silently drop a platform from the comparison.
 */
function publishedLogPaths(): Map<string, string> {
  const paths = new Map<string, string>()
  for (const match of read(BUG_REPORT_YML).matchAll(/^\s*- (\w+): `([^`]+)`$/gm)) {
    paths.set(match[1], match[2])
  }
  return paths
}

/**
 * `app_log_dir()` per platform, from the Tauri path API:
 * Windows `{LOCALAPPDATA}\{identifier}\logs`, macOS
 * `{home}/Library/Logs/{identifier}`, Linux `{XDG_DATA_HOME}/{identifier}/logs`.
 */
function derivedLogPaths(): Map<string, string> {
  const id = bundleIdentifier()
  const file = `${logFileName()}${logFileExtension()}`
  return new Map([
    ['Windows', `%LOCALAPPDATA%\\${id}\\logs\\${file}`],
    ['macOS', `~/Library/Logs/${id}/${file}`],
    ['Linux', `~/.local/share/${id}/logs/${file}`]
  ])
}

describe('published log paths vs. the code that writes them', () => {
  const published = publishedLogPaths()
  const derived = derivedLogPaths()

  it('derives the file name through the brand seam the app actually reads', () => {
    expect(loggingReadsTheBrandSeam()).toBe(true)
  })

  it('publishes a path for every platform the app derives one for', () => {
    // Vacuity guard, and a real check in itself: dropping a platform bullet
    // from the template is as much a break as misspelling one.
    expect([...published.keys()].sort()).toEqual([...derived.keys()].sort())
  })

  it.each([...derived.keys()])('%s path matches character for character', (platform) => {
    expect(published.get(platform)).toBe(derived.get(platform))
  })
})
