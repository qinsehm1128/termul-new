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
import { afterEach, describe, expect, it } from 'vitest'

const repoRoot = process.cwd()

/** The app's own Swift sources. `ios/Vendor/` is third-party and out of scope. */
const APP_ROOT = 'ios/TermulRemote/TermulRemote'
const INFO_PLIST = `${APP_ROOT}/Info.plist`
const PBXPROJ = 'ios/TermulRemote/TermulRemote.xcodeproj/project.pbxproj'
const FIXTURE = 'src/__fixtures__/legacy-brand/ios-defaults-dump.json'

/**
 * Post-rename iOS values. Derived from the locked identity: display name `Se`,
 * deep-link scheme `se://`, and the keychain service ruled in OD-02.
 */
const POST_RENAME = {
  iosDefaultsPrefix: 'se.',
  iosCacheDir: 'SeRemote',
  keychainPairingService: 'com.se-manager.remote.pairing',
  deepLinkScheme: 'se',
  displayName: 'Se'
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

/**
 * `relativePath`'s Swift source with comments stripped.
 *
 * Every assertion below is about what the app *does*, and a doc comment that
 * merely names a constant is not that. This is not a hypothetical distinction:
 * the first draft of the same-origin test asserted that the parser mentions
 * `CFBundleURLTypes`, and it stayed green when the entire lookup was deleted,
 * because the doc comment above it still said the word.
 *
 * Whole-line comments go, which is where doc comments live. A trailing comment
 * is stripped only from a line that holds no `"`, so a string literal such as
 * `"https://…"` can never be truncated at its own `//` — the conservative
 * direction, since the residue can only produce a false red, never a false
 * green.
 */
function swiftCode(relativePath: string): string {
  return read(relativePath)
    .replace(/\/\*[\s\S]*?\*\//g, ' ')
    .split('\n')
    .map((line) => {
      if (line.trimStart().startsWith('//')) return ''
      return line.includes('"') ? line : line.replace(/\/\/.*$/, '')
    })
    .join('\n')
}

/** Files that spell `value` as an exact quoted Swift string literal, in code. */
function filesSpelling(value: string): string[] {
  const needle = `"${value}"`
  return appSwiftFiles().filter((relative) => swiftCode(relative).includes(needle))
}

/**
 * Every iOS value that names something already sitting on a user's device, as
 * `(legacy, canonical, declaringFile)`.
 *
 * The `UserDefaults` suffixes come from the frozen fixture and the prefix from
 * the brand seam, so a canonical key is *computed* rather than transcribed.
 *
 * The deep-link scheme is deliberately **not** here, and the omission is the
 * substantive difference between it and these six. These six address data the
 * app itself wrote — chosen language, saved desks, pairing secrets, cached
 * transcripts — so dropping the legacy spelling silently destroys it, and every
 * assertion below therefore demands both spellings. A URL scheme addresses
 * nothing: it is a name the app claims from the system, and T-A14's locked
 * decision is to stop claiming the old one. Folding it in here would have forced
 * an assertion that contradicts that decision, so it gets its own two tests at
 * the bottom of this file instead — stricter, not laxer: exactly one spelling,
 * in exactly one place, and the legacy one nowhere.
 */
function persistenceSites(): {
  legacy: string
  canonical: string
  declaredIn: string
  what: string
}[] {
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
    }
  ]
}

/**
 * Every value the Xcode project bakes into the generated `Info.plist` under
 * `INFOPLIST_KEY_<key>`, one per build configuration.
 *
 * These are the *base* values. A localized `InfoPlist.xcstrings` entry overrides
 * them for a matched locale, which is exactly what made the residual below hard
 * to see.
 */
function infoPlistBuildSettings(key: string): string[] {
  return [...read(PBXPROJ).matchAll(new RegExp(`INFOPLIST_KEY_${key} = (.*);`, 'g'))].map((match) =>
    match[1].replace(/^"|"$/g, '')
  )
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
    for (const site of persistenceSites()) {
      expect(filesSpelling(site.legacy), `${site.what} (${site.legacy})`).toEqual([site.declaredIn])
    }
  })

  it('still keeps the legacy values readable after the flip', () => {
    // Green today because the legacy value is the *only* value. It becomes
    // load-bearing when Wave 5 lands: deleting the legacy read strands every
    // device that has not re-paired.
    for (const site of persistenceSites()) {
      expect(swiftCode(site.declaredIn), `${site.what} lost its legacy read`).toContain(site.legacy)
    }
  })

  // Landed by T-M11: every persistence site carries the post-rename value *and*
  // keeps the legacy one as a compatibility read. Both halves in one assertion
  // on purpose: flipping without the fallback loses the user's saved desks,
  // their pairing secrets and their cached transcripts, and keeping the fallback
  // without flipping is not a rename.
  it('carries the post-rename value alongside a legacy read at every site', () => {
    __setBrandCanonicalOverride(POST_RENAME)
    const missing = persistenceSites()
      .filter((site) => {
        const source = swiftCode(site.declaredIn)
        return !source.includes(`"${site.canonical}"`) || !source.includes(`"${site.legacy}"`)
      })
      .map((site) => `${site.what} @ ${site.declaredIn}`)

    expect(missing).toEqual([])
  })

  // Was the Wave-5 ledger red, resolved by T-A14. `Info.plist` and
  // `RemoteLink.swift` each used to hold their own copy of the scheme, so the
  // registration and the comparison could drift apart: the system would hand the
  // app a URL that its own parser then rejects, with no error naming a cause.
  // The parser now derives the scheme from `CFBundleURLTypes` in its own bundle,
  // so there is one spelling and it is the registered one.
  it('sources the URL scheme once, from the registration the parser reads', () => {
    __setBrandCanonicalOverride(POST_RENAME)
    const record = dump()
    const parser = `${APP_ROOT}/${record.urlScheme.declaredIn}`
    const source = swiftCode(parser)

    expect(registeredUrlSchemes()).toEqual([brandCanonical().deepLinkScheme])
    // The parser must not hold a second, independent copy of the same string...
    expect(source).not.toContain(`"${brandCanonical().deepLinkScheme}"`)
    // ...and the reason it does not is that it reads the registration. Without
    // this half, deleting the scheme check outright would satisfy the half above.
    expect(source, `${parser} no longer reads its own registration`).toContain(
      'forInfoDictionaryKey: "CFBundleURLTypes"'
    )
  })

  // T-A14's locked decision, and the one contract in this file that gets no
  // compatibility read. A pre-rename link a user saved outside the app — a
  // Safari bookmark, a message thread — stops opening. That is affordable only
  // because the desktop never generated one (R-OQ3): the pairing QR and the copy
  // button both hand out an `https` access URL, which never reaches the scheme
  // comparison. Asserting the absence in both places is what stops the old
  // scheme from being quietly restored on one side alone.
  it('drops the legacy scheme entirely rather than accepting both', () => {
    const record = dump()
    expect(registeredUrlSchemes()).not.toContain(record.urlScheme.value)
    expect(filesSpelling(record.urlScheme.value)).toEqual([])
  })

  // Correction U-02. `InfoPlist.xcstrings` was flipped to `Se` early, and that
  // is what hid this: a localized value wins for a matched locale, so en and
  // zh-Hans already displayed `Se` while the *base* fallback in the build
  // settings still said `Termul`. Measured on the built product rather than
  // inferred — `TermulRemote.app/Info.plist` carried
  // `CFBundleDisplayName = Termul` next to localized tables reading `Se`.
  //
  // Guarded here because it was owned by no task until Wave 5: the display name
  // is not one of the seven contracts above, so nothing else in this file would
  // have noticed it. Directory and target renaming remains T-B08's.
  it('bakes the post-rename display name into the base Info.plist fallback', () => {
    __setBrandCanonicalOverride(POST_RENAME)
    const displayNames = infoPlistBuildSettings('CFBundleDisplayName')

    // Guards the regexp itself: an empty match set would pass every loop below.
    expect(displayNames.length, `${PBXPROJ} sets no base display name`).toBeGreaterThan(0)
    for (const value of displayNames) {
      expect(value).toBe(brandCanonical().displayName)
    }

    const cameraReasons = infoPlistBuildSettings('NSCameraUsageDescription')
    expect(cameraReasons.length, `${PBXPROJ} sets no camera usage description`).toBeGreaterThan(0)
    for (const value of cameraReasons) {
      expect(value).toContain(brandCanonical().displayName)
      expect(value).not.toContain(LEGACY.displayName)
    }
  })
})
