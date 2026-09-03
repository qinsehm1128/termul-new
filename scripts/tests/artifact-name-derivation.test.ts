/**
 * T-H16 — distribution artifact names must have a single upstream.
 *
 * Tauri derives every DMG / MSI / NSIS / AppImage / deb / rpm file name from
 * `tauri.conf.json` → `productName`. Everything downstream — the installer, the
 * Homebrew cask, the release-prep scripts — must therefore *derive* its names
 * from that same upstream rather than carry an independent copy.
 *
 * The tests this replaces asserted each script's literal against a copy of
 * itself, so a repo-wide sed rewrote both sides and stayed green while the
 * artifact name and its consumer had actually drifted apart. Here both sides
 * are read from disk and one is *computed* from the other, so a rename that
 * misses a consumer is detectable.
 *
 * Currently RED, and legitimately so: several scripts still hardcode names that
 * cannot be derived from `productName` or the package name (most visibly the
 * Homebrew cask token `termul`, which matches neither). Wave 6 (T-B01..T-B05)
 * closes that; until then the failure is registered with `test.fails()` as part
 * of the self-liquidating red/green ledger.
 */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

const repoRoot = process.cwd()
const read = (relativePath: string): string => readFileSync(join(repoRoot, relativePath), 'utf8')
const readJson = (relativePath: string): Record<string, unknown> =>
  JSON.parse(read(relativePath)) as Record<string, unknown>

/** The one value Tauri turns into every bundle file name. */
function productName(): string {
  return readJson('src-tauri/tauri.conf.json').productName as string
}

/** Tauri replaces spaces with dots when building bundle file names. */
function bundleFileStem(): string {
  return productName().replaceAll(' ', '.')
}

/** `[section]` / `key = "value"` lookup good enough for Cargo.toml's top level. */
function cargoValue(section: string, key: string): string {
  const toml = read('src-tauri/Cargo.toml')
  const sectionBody = toml.split(`[${section}]`)[1] ?? ''
  const upToNextSection = sectionBody.split(/\n\[/)[0]
  const match = upToNextSection.match(new RegExp(`^${key}\\s*=\\s*"([^"]+)"`, 'm'))
  if (!match) throw new Error(`Cargo.toml [${section}] ${key} not found`)
  return match[1]
}

describe('artifact name derivation', () => {
  describe('identity sites agree with each other', () => {
    it('package.json, Cargo.toml and Cargo.lock carry the same package name', () => {
      const packageName = readJson('package.json').name as string
      expect(cargoValue('package', 'name')).toBe(packageName)
      expect(cargoValue('package', 'default-run')).toBe(packageName)
      expect(read('src-tauri/Cargo.lock')).toContain(`name = "${packageName}"`)
    })

    it('the crate lib name is the package name in snake_case plus _lib', () => {
      const packageName = readJson('package.json').name as string
      expect(cargoValue('lib', 'name')).toBe(`${packageName.replaceAll('-', '_')}_lib`)
    })

    it('package.json and tauri.conf.json agree on productName', () => {
      expect(readJson('package.json').productName).toBe(productName())
    })
  })

  describe('consumers derive their names rather than hardcoding them', () => {
    it('install.sh builds the macOS asset name from productName', () => {
      // `Termul.Manager_${version}_${suffix}.dmg` — the stem must be computed
      // from productName, not written out again.
      expect(read('scripts/install.sh')).toContain(`${bundleFileStem()}_\${normalized_version}_`)
    })

    it('install.sh installs the .app under the productName bundle name', () => {
      expect(read('scripts/install.sh')).toContain(`${productName()}.app`)
    })

    it('install.sh names the Linux binary after the package name', () => {
      const packageName = readJson('package.json').name as string
      expect(read('scripts/install.sh')).toContain(`/${packageName}"`)
      expect(read('scripts/install.sh')).toContain(`${packageName}.desktop`)
    })

    // REGISTERED RED (self-liquidating ledger entry). The cask token is
    // currently `termul`, which is neither the package name nor a transform of
    // productName — a rename driven off either upstream silently leaves it
    // behind. T-B04 makes it derived; this `.fails()` then starts failing
    // *because it passes*, forcing its own removal.
    it.fails('the Homebrew cask token is derived from the package name', () => {
      const packageName = readJson('package.json').name as string
      expect(read('scripts/release/homebrew.sh')).toContain(`cask "${packageName}" do`)
    })

    it('the Homebrew cask app and dmg names come from productName', () => {
      const homebrew = read('scripts/release/homebrew.sh')
      expect(homebrew).toContain(`app "${productName()}.app"`)
      expect(homebrew).toContain(`name "${productName()}"`)
      expect(homebrew).toContain(`${bundleFileStem()}_#{version}_#{arch}.dmg`)
    })

    it('the Homebrew zap list uses the shipped bundle identifier', () => {
      const identifier = readJson('src-tauri/tauri.conf.json').identifier as string
      expect(read('scripts/release/homebrew.sh')).toContain(identifier)
    })

    it('prepare-server-artifacts.mjs names the server binary from Cargo', () => {
      // Anchor on a line-start `[[bin]]`: the string also appears inside a
      // comment further up, and splitting on the bare token lands there.
      const serverBinary = read('src-tauri/Cargo.toml')
        .split(/^\[\[bin\]\]$/m)[1]
        ?.match(/name\s*=\s*"([^"]+)"/)?.[1]
      expect(serverBinary).toBeTruthy()
      expect(read('scripts/release/prepare-server-artifacts.mjs')).toContain(`'${serverBinary}'`)
    })
  })
})
