import { mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, test } from 'vitest'
// @ts-expect-error The production helper is intentionally plain ESM for direct workflow use.
import { mergeUpdaterManifests, requiredPlatformKeys } from './merge-updater-manifests.mjs'

const version = '1.2.3'
const assetName = (suffix: string) => `Termul-${version}-${suffix}.bin`
const record = (suffix: string) => ({
  url: `https://github.com/qinsehm1128/termul-new/releases/download/v${version}/${assetName(suffix)}`,
  signature: `signature-${suffix}`
})

async function fixtureDir() {
  return mkdtemp(join(tmpdir(), 'termul-updater-manifest-'))
}

async function writeManifest(
  path: string,
  platforms: Record<string, unknown>,
  manifestVersion = version,
  assetNames = Object.keys(platforms).flatMap((key) => [assetName(key), `${assetName(key)}.sig`])
) {
  await writeFile(path, JSON.stringify({ version: manifestVersion, assetNames, platforms }))
}

function completePlatforms() {
  return Object.fromEntries(requiredPlatformKeys.map((key: string) => [key, record(key)]))
}

function completeAssetNames() {
  return requiredPlatformKeys.flatMap((key: string) => [assetName(key), `${assetName(key)}.sig`])
}

/**
 * A representative key the merged manifest must carry, derived from the required
 * set instead of named literally. Several tests below only need "some required
 * key" to delete or corrupt; naming one meant trimming the build matrix left them
 * asserting about a key the manifest no longer has. Excludes the server key,
 * which has its own dedicated test.
 */
const sampleKey: string = requiredPlatformKeys.find((key: string) => key !== 'linux-x86_64-server')

const workflowDir = resolve(dirname(fileURLToPath(import.meta.url)), '../../.github/workflows')
const releaseWorkflowPath = join(workflowDir, 'release.yml')
const nightlyWorkflowPath = join(workflowDir, 'nightly.yml')

async function matrixPlatformsOf(workflowPath: string): Promise<string[]> {
  const workflow = await readFile(workflowPath, 'utf8')
  return [...workflow.matchAll(/^ {10}- platform: (\S+)$/gm)].map((match) => match[1])
}

/**
 * Updater keys each desktop target contributes, written out here rather than
 * imported from `prepare-platform-artifacts.mjs` on purpose — an independent
 * statement of the mapping is what makes the assertion below worth anything.
 */
const updaterKeysByPlatform: Record<string, string[]> = {
  'windows-x64': ['windows-x86_64', 'windows-x86_64-msi', 'windows-x86_64-nsis'],
  'linux-x64': ['linux-x86_64', 'linux-x86_64-appimage', 'linux-x86_64-deb', 'linux-x86_64-rpm'],
  'macos-aarch64': ['darwin-aarch64', 'darwin-aarch64-app'],
  'macos-x64': ['darwin-x86_64', 'darwin-x86_64-app']
}

describe('mergeUpdaterManifests', () => {
  test('authoritatively merges all historical Tauri platform keys', async () => {
    const dir = await fixtureDir()
    const inputs = await Promise.all(
      Object.entries(completePlatforms()).map(async ([key, value], index) => {
        const path = join(dir, `${index}.json`)
        await writeManifest(path, { [key]: value })
        return path
      })
    )
    const outputPath = join(dir, 'latest.json')

    const merged = await mergeUpdaterManifests({
      inputPaths: inputs,
      outputPath,
      version,
      notes: 'notes',
      pubDate: '2026-01-01T00:00:00.000Z'
    })

    expect(Object.keys(merged.platforms).sort()).toEqual([...requiredPlatformKeys].sort())
    expect(JSON.parse(await readFile(outputPath, 'utf8'))).toEqual(merged)
  })

  /**
   * `requiredPlatformKeys` is the gate that fails a release when a target stops
   * producing artifacts, so it has to track the build matrix. Nothing else here
   * does: every other fixture derives from `requiredPlatformKeys`, so dropping a
   * key just shrinks the whole suite consistently — it stays green while the
   * published manifest silently loses a platform. This pins the two together.
   */
  test('requiredPlatformKeys covers exactly the release matrix plus the server target', async () => {
    const matrixPlatforms = await matrixPlatformsOf(releaseWorkflowPath)
    expect(matrixPlatforms.length).toBeGreaterThan(0)

    const expected = matrixPlatforms.flatMap((platform) => {
      const keys = updaterKeysByPlatform[platform]
      if (!keys) {
        throw new Error(`release.yml builds ${platform} but this test has no updater keys for it`)
      }
      return keys
    })
    // The standalone `termul-server` is not a matrix entry — it has its own job.
    expected.push('linux-x86_64-server')

    expect([...requiredPlatformKeys].sort()).toEqual(expected.sort())
  })

  /**
   * Nightly publishes updater manifests through the same merge + the same gate.
   * If it built a platform the release matrix does not, the nightly channel would
   * offer updates for a target that stable can never follow up on; if it built
   * fewer, the nightly manifest would fail the gate at publish time instead of at
   * review time.
   */
  test('the nightly build matrix matches the release build matrix', async () => {
    const [release, nightly] = await Promise.all([
      matrixPlatformsOf(releaseWorkflowPath),
      matrixPlatformsOf(nightlyWorkflowPath)
    ])
    expect(nightly.length).toBeGreaterThan(0)
    expect([...nightly].sort()).toEqual([...release].sort())
  })

  test('rejects a missing updater platform', async () => {
    const dir = await fixtureDir()
    const platforms = completePlatforms()
    delete platforms[sampleKey]
    const input = join(dir, 'manifest.json')
    await writeManifest(input, platforms)

    await expect(
      mergeUpdaterManifests({
        inputPaths: [input],
        outputPath: join(dir, 'latest.json'),
        version,
        notes: 'notes',
        pubDate: '2026-01-01T00:00:00.000Z'
      })
    ).rejects.toThrow(`Missing required updater platforms: ${sampleKey}`)
  })

  test.each([
    ['empty url', { url: '', signature: 'signature' }, 'url must be a nonempty string'],
    ['missing signature', { url: record(sampleKey).url }, 'signature must be a nonempty string'],
    ['non-object record', 'broken', 'must be an object']
  ])('rejects malformed updater entries: %s', async (_name, malformed, error) => {
    const dir = await fixtureDir()
    const platforms = completePlatforms()
    platforms[sampleKey] = malformed as never
    const input = join(dir, 'manifest.json')
    await writeManifest(input, platforms, version, completeAssetNames())

    await expect(
      mergeUpdaterManifests({
        inputPaths: [input],
        outputPath: join(dir, 'latest.json'),
        version,
        notes: 'notes',
        pubDate: '2026-01-01T00:00:00.000Z'
      })
    ).rejects.toThrow(error)
  })

  test('rejects conflicting duplicate updater records', async () => {
    const dir = await fixtureDir()
    const first = join(dir, 'first.json')
    const second = join(dir, 'second.json')
    await writeManifest(first, completePlatforms(), version, completeAssetNames())
    await writeManifest(second, { 'windows-x86_64': record('different') }, version, [
      assetName('different'),
      `${assetName('different')}.sig`
    ])

    await expect(
      mergeUpdaterManifests({
        inputPaths: [first, second],
        outputPath: join(dir, 'latest.json'),
        version,
        notes: 'notes',
        pubDate: '2026-01-01T00:00:00.000Z'
      })
    ).rejects.toThrow('Conflicting duplicate updater record for windows-x86_64')
  })

  test.each([
    [
      'a different release tag',
      `https://github.com/qinsehm1128/termul-new/releases/download/v9.9.9/${assetName(sampleKey)}`,
      'must target the current v1.2.3 GitHub release'
    ],
    [
      'an uncollected asset',
      `https://github.com/qinsehm1128/termul-new/releases/download/v${version}/uncollected.bin`,
      'references uncollected release asset uncollected.bin'
    ]
  ])('rejects updater URLs referencing %s', async (_name, url, error) => {
    const dir = await fixtureDir()
    const platforms = completePlatforms()
    platforms[sampleKey] = { ...record(sampleKey), url }
    const input = join(dir, 'manifest.json')
    await writeManifest(input, platforms, version, completeAssetNames())

    await expect(
      mergeUpdaterManifests({
        inputPaths: [input],
        outputPath: join(dir, 'latest.json'),
        version,
        notes: 'notes',
        pubDate: '2026-01-01T00:00:00.000Z'
      })
    ).rejects.toThrow(error)
  })

  test('rejects a manifest without the collected signature asset', async () => {
    const dir = await fixtureDir()
    const input = join(dir, 'manifest.json')
    const assets = completeAssetNames().filter(
      (name: string) => name !== `${assetName(sampleKey)}.sig`
    )
    await writeManifest(input, completePlatforms(), version, assets)

    await expect(
      mergeUpdaterManifests({
        inputPaths: [input],
        outputPath: join(dir, 'latest.json'),
        version,
        notes: 'notes',
        pubDate: '2026-01-01T00:00:00.000Z'
      })
    ).rejects.toThrow(
      `Updater record ${sampleKey} is missing collected signature asset ${assetName(sampleKey)}.sig`
    )
  })

  test('requires the linux-x86_64-server platform key covering the server target', async () => {
    const dir = await fixtureDir()
    const platforms = completePlatforms()
    delete platforms['linux-x86_64-server']
    const input = join(dir, 'manifest.json')
    await writeManifest(input, platforms, version, completeAssetNames())

    await expect(
      mergeUpdaterManifests({
        inputPaths: [input],
        outputPath: join(dir, 'latest.json'),
        version,
        notes: 'notes',
        pubDate: '2026-01-01T00:00:00.000Z'
      })
    ).rejects.toThrow('Missing required updater platforms: linux-x86_64-server')
  })

  describe('channel and release-tag selection', () => {
    test('derives the nightly moving tag for the nightly channel', async () => {
      const dir = await fixtureDir()
      const nightlyVersion = '0.0.0-nightly.20260807.abc1234'
      const nightlyAssetNames = requiredPlatformKeys.flatMap((key: string) => [
        assetName(key),
        `${assetName(key)}.sig`
      ])
      const nightlyPlatforms = Object.fromEntries(
        requiredPlatformKeys.map((key: string) => [
          key,
          {
            url: `https://github.com/qinsehm1128/termul-new/releases/download/nightly/${assetName(key)}`,
            signature: `signature-${key}`
          }
        ])
      )
      const input = join(dir, 'manifest.json')
      await writeManifest(input, nightlyPlatforms, nightlyVersion, nightlyAssetNames)

      const outputPath = join(dir, 'latest-nightly.json')
      const merged = await mergeUpdaterManifests({
        inputPaths: [input],
        outputPath,
        version: nightlyVersion,
        notes: 'nightly notes',
        pubDate: '2026-08-07T00:00:00.000Z',
        channel: 'nightly'
      })

      expect(merged.platforms['linux-x86_64-server'].url).toBe(
        `https://github.com/qinsehm1128/termul-new/releases/download/nightly/${assetName('linux-x86_64-server')}`
      )
      expect(JSON.parse(await readFile(outputPath, 'utf8'))).toEqual(merged)
    })

    test('accepts a nightly manifest without windows-x86_64-msi (NSIS-only build)', async () => {
      const dir = await fixtureDir()
      const nightlyVersion = '0.0.0-nightly.20260807.abc1234'
      const nightlyKeys = requiredPlatformKeys.filter((key: string) => key !== 'windows-x86_64-msi')
      const nightlyAssetNames = nightlyKeys.flatMap((key: string) => [
        assetName(key),
        `${assetName(key)}.sig`
      ])
      const nightlyPlatforms = Object.fromEntries(
        nightlyKeys.map((key: string) => [
          key,
          {
            url: `https://github.com/qinsehm1128/termul-new/releases/download/nightly/${assetName(key)}`,
            signature: `signature-${key}`
          }
        ])
      )
      const input = join(dir, 'manifest.json')
      await writeManifest(input, nightlyPlatforms, nightlyVersion, nightlyAssetNames)

      const outputPath = join(dir, 'latest-nightly.json')
      const merged = await mergeUpdaterManifests({
        inputPaths: [input],
        outputPath,
        version: nightlyVersion,
        notes: 'nightly nsis-only notes',
        pubDate: '2026-08-07T00:00:00.000Z',
        channel: 'nightly'
      })

      expect(merged.platforms['windows-x86_64-nsis']).toBeDefined()
      expect(merged.platforms['windows-x86_64-msi']).toBeUndefined()
    })

    test('rejects a nightly manifest that targets the versioned tag instead of the nightly tag', async () => {
      const dir = await fixtureDir()
      const nightlyVersion = '0.0.0-nightly.20260807.abc1234'
      const nightlyAssetNames = completeAssetNames()
      const nightlyPlatforms = completePlatforms()
      // URL uses v<version> (the versioned tag) but the nightly channel expects
      // the moving `nightly` tag.
      nightlyPlatforms[sampleKey] = {
        url: `https://github.com/qinsehm1128/termul-new/releases/download/v${nightlyVersion}/${assetName(sampleKey)}`,
        signature: `signature-${sampleKey}`
      }
      const input = join(dir, 'manifest.json')
      await writeManifest(input, nightlyPlatforms, nightlyVersion, nightlyAssetNames)

      await expect(
        mergeUpdaterManifests({
          inputPaths: [input],
          outputPath: join(dir, 'latest-nightly.json'),
          version: nightlyVersion,
          notes: 'notes',
          pubDate: '2026-08-07T00:00:00.000Z',
          channel: 'nightly'
        })
      ).rejects.toThrow('must target the current nightly GitHub release')
    })

    test('accepts an explicit tag override regardless of channel', async () => {
      const dir = await fixtureDir()
      const rcVersion = '0.5.0-rc.1'
      const rcAssetNames = completeAssetNames()
      const rcPlatforms = Object.fromEntries(
        requiredPlatformKeys.map((key: string) => [
          key,
          {
            url: `https://github.com/qinsehm1128/termul-new/releases/download/v${rcVersion}/${assetName(key)}`,
            signature: `signature-${key}`
          }
        ])
      )
      const input = join(dir, 'manifest.json')
      await writeManifest(input, rcPlatforms, rcVersion, rcAssetNames)

      const merged = await mergeUpdaterManifests({
        inputPaths: [input],
        outputPath: join(dir, 'latest-insider.json'),
        version: rcVersion,
        notes: 'rc notes',
        pubDate: '2026-08-07T00:00:00.000Z',
        channel: 'insider',
        tag: `v${rcVersion}`
      })

      expect(merged.platforms['linux-x86_64-server'].url).toContain(
        `/releases/download/v${rcVersion}/`
      )
    })
  })
})
