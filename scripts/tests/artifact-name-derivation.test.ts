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
 * Wave 6 (T-B01..T-B04) closed the last of these. The Homebrew cask token was
 * the loudest one — it was `termul`, matching neither upstream, and was carried
 * as a `test.fails()` ledger entry until T-B04 made it composed from the
 * PACKAGE_NAME definition. The marker is gone because the gate passes, not
 * because the assertion was removed.
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

/**
 * The stem a *published* release asset carries.
 *
 * Tauri bundles under the spaced product name (`Se Manager_0.5.9_aarch64.dmg`,
 * confirmed against a real `bun run build`); `prepare-platform-artifacts.mjs` →
 * `releaseAssetName()` replaces the spaces with dots when it stages assets for
 * the GitHub release. Every consumer here downloads the published asset, so the
 * dotted form is what they must compose.
 */
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

  /**
   * Each consumer holds exactly ONE definition of each identity and composes
   * every artifact name from it. Two things are checked, and both are needed:
   *
   * 1. the definition equals what the upstream says it must be — read from a
   *    *different* file, so a rename that misses this one is detectable;
   * 2. nothing downstream spells the artifact name out again — otherwise a
   *    correct definition can sit next to a stale copy that is what actually
   *    ships, which is exactly the drift the cask token used to carry.
   */
  describe('consumers derive their names rather than hardcoding them', () => {
    it('install.sh defines the product and package identity once, from the upstreams', () => {
      const installSh = read('scripts/install.sh')
      const packageName = readJson('package.json').name as string
      expect(installSh).toContain(`PRODUCT_NAME="${productName()}"`)
      expect(installSh).toContain(`PACKAGE_NAME="${packageName}"`)
      // The space -> dot transform lives in exactly one place.
      expect(installSh).toContain('BUNDLE_STEM="${PRODUCT_NAME// /.}"')
    })

    it('install.sh composes every artifact name from those definitions', () => {
      const installSh = read('scripts/install.sh')
      for (const composed of [
        'asset_name="${BUNDLE_STEM}_${normalized_version}_${suffix}.dmg"',
        'asset_name="${BUNDLE_STEM}_${normalized_version}_amd64.AppImage"',
        'app_source="${mount_dir}/${PRODUCT_NAME}.app"',
        'app_target="${applications_dir}/${PRODUCT_NAME}.app"',
        'local target_path="${bin_dir}/${PACKAGE_NAME}"',
        'local desktop_path="${desktop_dir}/${PACKAGE_NAME}.desktop"'
      ]) {
        expect(installSh).toContain(composed)
      }
      // No spelled-out bundle stem may survive alongside the definition. The
      // trailing `_` is what a bundle file name always carries and what
      // `PRODUCT_NAME="Se Manager"` never does.
      expect(installSh).not.toContain(`${bundleFileStem()}_`)
    })

    // Ledger entry struck by T-B04. The cask token was `termul` — neither the
    // package name nor a transform of productName — so a rename driven off
    // either upstream silently left it behind. It is now composed from the
    // PACKAGE_NAME definition, which the assertions below pin to package.json.
    it('the Homebrew cask token is derived from the package name', () => {
      const packageName = readJson('package.json').name as string
      const homebrew = read('scripts/release/homebrew.sh')
      expect(homebrew).toContain(`PACKAGE_NAME="${packageName}"`)
      expect(homebrew).toContain('cask "${PACKAGE_NAME}" do')
    })

    it('the Homebrew cask app and dmg names come from productName', () => {
      const homebrew = read('scripts/release/homebrew.sh')
      expect(homebrew).toContain(`PRODUCT_NAME="${productName()}"`)
      expect(homebrew).toContain('BUNDLE_STEM="${PRODUCT_NAME// /.}"')
      expect(homebrew).toContain('app "${PRODUCT_NAME}.app"')
      expect(homebrew).toContain('name "${PRODUCT_NAME}"')
      expect(homebrew).toContain('${BUNDLE_STEM}_#{version}_#{arch}.dmg')
      expect(homebrew).toContain('local arm_dmg="${BUNDLE_STEM}_${version}_aarch64.dmg"')
      expect(homebrew).toContain('local intel_dmg="${BUNDLE_STEM}_${version}_x64.dmg"')
      expect(homebrew).not.toContain(`${bundleFileStem()}_`)
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
      const prepare = read('scripts/release/prepare-server-artifacts.mjs')
      expect(prepare).toContain(`const DEFAULT_SERVER_BINARY = '${serverBinary}'`)
      // Both defaults reference the definition rather than repeating it.
      expect(prepare).toContain('binaryName = DEFAULT_SERVER_BINARY')
      expect(prepare).toContain("options['binary-name'] ?? DEFAULT_SERVER_BINARY")
    })
  })
})
