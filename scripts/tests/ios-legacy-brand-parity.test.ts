/**
 * T-H25 — iOS legacy-brand source-text parity.
 *
 * # What this is, and what it deliberately is not
 *
 * T-H18 wanted a behavioural iOS gate. It is blocked by two environment facts
 * this Session cannot fix in code, both measured rather than assumed:
 * `xcodebuild -list` reports a single target, `TermulRemote` (there is no
 * `TermulRemoteTests`), and `xcrun simctl list runtimes` is empty on this
 * machine. So the iOS gate is downgraded from behavioural to source-text parity
 * against a frozen fixture — the same honest move already accepted for the
 * keychain in T-H08.
 *
 * **This does not replace T-H18.** T-H18 stays blocked and iOS runtime coverage
 * stays recorded as `unverified`. Nothing here executes a line of Swift.
 *
 * What it *can* guard is the actual failure mode of T-M11 / T-A13 / T-A14 /
 * T-B08: one of the four `UserDefaults` keys, the keychain service, the
 * Application Support directory or the URL scheme gets missed, or gets set to a
 * value inconsistent with the brand module. Those are text-level mistakes and
 * this is a text-level gate.
 *
 * # Why it reads from disk
 *
 * `src/__fixtures__/legacy-brand/ios-defaults-dump.json` is the frozen record of
 * what a pre-rename install owns on a device, sha256-guarded by
 * `src/__fixtures__/legacy-brand-manifest.test.ts`. Every value below is
 * assembled from that fixture plus `LEGACY` / `brandCanonical()` — the key
 * *suffixes* come from the fixture and the *prefix* from the brand seam, so
 * neither side can be edited alone. No iOS string is written down in this file.
 *
 * The vendored `ios/Vendor/SwiftTerm` tree is excluded from every scan: it is
 * third-party source this rename does not own.
 */
import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import {
  __resetBrandCanonicalOverride,
  __setBrandCanonicalOverride,
  brandCanonical,
  LEGACY
} from '@shared/brand'
import { afterEach, describe, expect, it, test } from 'vitest'

const repoRoot = process.cwd()

/** The app's own Swift sources. `ios/Vendor/` is third-party and out of scope. */
const APP_ROOT = 'ios/TermulRemote/TermulRemote'
const INFO_PLIST = `${APP_ROOT}/Info.plist`
const FIXTURE = 'src/__fixtures__/legacy-brand/ios-defaults-dump.json'

/**
 * Post-rename iOS values. Derived from the locked identity: display name `Se`,
 * deep-link scheme `se://`, and the keychain service ruled in OD-02.
 */
const POST_RENAME = {
  iosDefaultsPrefix: 'se.',
  iosCacheDir: 'SeRemote',
  keychainPairingService: 'com.se-manager.remote.pairing',
  deepLinkScheme: 'se'
} as const

interface DefaultsEntry {
  readonly key: string
  readonly declaredIn: string
}

interface IosDump {
  readonly userDefaults: readonly DefaultsEntry[]
  readonly keychain: { readonly service: string; readonly declaredIn: string }
  readonly applicationSupportDirectory: { readonly name: string; readonly declaredIn: string }
  readonly urlScheme: {
    readonly value: string
    readonly declaredIn: string
    readonly registeredIn: string
  }
}

const read = (relativePath: string): string => readFileSync(join(repoRoot, relativePath), 'utf8')

function dump(): IosDump {
  return JSON.parse(read(FIXTURE)) as IosDump
}

/** Every `.swift` file the app itself owns. */
function appSwiftFiles(): string[] {
  const found: string[] = []
  const walk = (relativeDir: string): void => {
    for (const entry of readdirSync(join(repoRoot, relativeDir), { withFileTypes: true })) {
      const relative = `${relativeDir}/${entry.name}`
      if (entry.isDirectory()) {
        walk(relative)
        continue
      }
      if (entry.name.endsWith('.swift')) found.push(relative)
    }
  }
  walk(APP_ROOT)
  return found.sort()
}

/** Files that spell `value` as an exact quoted Swift string literal. */
function filesSpelling(value: string): string[] {
  const needle = `"${value}"`
  return appSwiftFiles().filter((relative) => read(relative).includes(needle))
}

/**
 * Every brand-bearing iOS value, as `(legacy, canonical, declaringFile)`.
 *
 * The `UserDefaults` suffixes come from the frozen fixture and the prefix from
 * the brand seam, so a canonical key is *computed* rather than transcribed.
 */
function sites(): { legacy: string; canonical: string; declaredIn: string; what: string }[] {
  const record = dump()
  const entries = record.userDefaults.map((entry) => {
    expect(entry.key.startsWith(LEGACY.iosDefaultsPrefix)).toBe(true)
    const suffix = entry.key.slice(LEGACY.iosDefaultsPrefix.length)
    return {
      legacy: entry.key,
      canonical: `${brandCanonical().iosDefaultsPrefix}${suffix}`,
      declaredIn: `${APP_ROOT}/${entry.declaredIn}`,
      what: `UserDefaults key ${suffix}`
    }
  })

  return [
    ...entries,
    {
      legacy: record.keychain.service,
      canonical: brandCanonical().keychainPairingService,
      declaredIn: `${APP_ROOT}/${record.keychain.declaredIn}`,
      what: 'keychain pairing service'
    },
    {
      legacy: record.applicationSupportDirectory.name,
      canonical: brandCanonical().iosCacheDir,
      declaredIn: `${APP_ROOT}/${record.applicationSupportDirectory.declaredIn}`,
      what: 'Application Support directory'
    },
    {
      legacy: record.urlScheme.value,
      canonical: brandCanonical().deepLinkScheme,
      declaredIn: `${APP_ROOT}/${record.urlScheme.declaredIn}`,
      what: 'deep-link URL scheme'
    }
  ]
}

/** The schemes `Info.plist` registers with the system. */
function registeredUrlSchemes(): string[] {
  const plist = read(INFO_PLIST)
  const block = /<key>CFBundleURLSchemes<\/key>\s*<array>([\s\S]*?)<\/array>/.exec(plist)
  expect(block, `${INFO_PLIST} no longer registers CFBundleURLSchemes`).not.toBeNull()
  return [...(block as RegExpExecArray)[1].matchAll(/<string>(.*?)<\/string>/g)].map(
    (match) => match[1]
  )
}

afterEach(() => {
  __resetBrandCanonicalOverride()
})

describe('iOS legacy-brand parity (source text only — no runtime evidence)', () => {
  it('records exactly the legacy values the brand module declares', () => {
    // Two independent sources: the frozen dump, and brand.ts. Editing either
    // alone turns this red, which is what makes the reds below trustworthy.
    const record = dump()
    expect(record.userDefaults.length).toBe(4)
    for (const entry of record.userDefaults) {
      expect(entry.key.startsWith(LEGACY.iosDefaultsPrefix)).toBe(true)
    }
    expect(record.keychain.service).toBe(LEGACY.keychainPairingService)
    expect(record.applicationSupportDirectory.name).toBe(LEGACY.iosCacheDir)
    expect(record.urlScheme.value).toBe(LEGACY.deepLinkScheme)
  })

  it('spells every legacy value exactly once in the app Swift sources', () => {
    // The T-H11 criterion, and F-07's reason for it: when the writer and the
    // reader both reference one constant, a string comparison has a single
    // source and can only certify itself. A second independent literal is a
    // second thing to miss.
    for (const site of sites()) {
      expect(filesSpelling(site.legacy), `${site.what} (${site.legacy})`).toEqual([site.declaredIn])
    }
  })

  it('still keeps the legacy values readable after the flip', () => {
    // Green today because the legacy value is the *only* value. It becomes
    // load-bearing when Wave 5 lands: deleting the legacy read strands every
    // device that has not re-paired.
    for (const site of sites()) {
      expect(read(site.declaredIn), `${site.what} lost its legacy read`).toContain(site.legacy)
    }
  })

  it('registers the scheme the parser compares against', () => {
    const record = dump()
    expect(registeredUrlSchemes()).toContain(record.urlScheme.value)
    expect(read(`${APP_ROOT}/${record.urlScheme.declaredIn}`)).toContain(
      `"${record.urlScheme.value}"`
    )
  })

  // Landed by T-M11: every site carries the post-rename value *and* keeps the
  // legacy one as a compatibility read. Both halves in one assertion on
  // purpose: flipping without the fallback loses the user's saved desks, their
  // pairing secrets and their cached transcripts, and keeping the fallback
  // without flipping is not a rename.
  //
  // CONFLICTS WITH the URL-scheme ledger entry below, and deliberately so —
  // this one requires `"se"` in `RemoteLink.swift`, that one requires its
  // absence. Only the scheme site is affected; the other six are independent.
  // T-A14 owns the reconciliation and must resolve both entries together.
  it('carries the post-rename value alongside a legacy read at every site', () => {
    __setBrandCanonicalOverride(POST_RENAME)
    const missing = sites()
      .filter((site) => {
        const source = read(site.declaredIn)
        return !source.includes(`"${site.canonical}"`) || !source.includes(`"${site.legacy}"`)
      })
      .map((site) => `${site.what} @ ${site.declaredIn}`)

    expect(missing).toEqual([])
  })

  // LEDGER (Wave 5) — expected failure. `Info.plist` and `RemoteLink.swift`
  // each hold their own copy of the scheme today, so the registration and the
  // comparison can drift apart: the system would still hand the app a
  // `termul://` URL that its own parser rejects, with no error naming a cause.
  // They must end up same-sourced — one Swift constant, and a plist value
  // driven from the build settings rather than typed twice.
  test.fails('sources the URL scheme once across Info.plist and the Swift parser', () => {
    __setBrandCanonicalOverride(POST_RENAME)
    const record = dump()
    const parser = `${APP_ROOT}/${record.urlScheme.declaredIn}`

    expect(registeredUrlSchemes()).toEqual([brandCanonical().deepLinkScheme])
    // The parser must not hold a second, independent copy of the same string.
    expect(read(parser).includes(`"${brandCanonical().deepLinkScheme}"`)).toBe(false)
  })
})
