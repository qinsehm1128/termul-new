/**
 * Single source of truth for every brand-bearing identifier that crosses a
 * persistence or wire boundary.
 *
 * Two constant groups, and the distinction between them is load-bearing:
 *
 * - `LEGACY_*` — the values that are already on users' disks / in their agent
 *   memories. They are **permanent**. They are only ever *read*; nothing in the
 *   app may write them again (FORBID-04). This module is the only legitimate
 *   home for a legacy brand literal outside the frozen fixture roots.
 * - `CANONICAL_*` — the values the app writes *today*. Renaming a contract is
 *   therefore a one-line edit here rather than a 400-file replacement, which is
 *   what makes "no repo-wide sed" a structural property instead of a slogan.
 *
 * `CANONICAL_*` is read through {@link brandCanonical}, a seam a test can
 * override. That lets a harness test inject the post-rename value while
 * production still emits the pre-rename one — which is how a Wave-1 red can be
 * a *real* red rather than a self-certifying assertion.
 *
 * Residual-scan whitelist: this file and `src-tauri/src/brand.rs` are the only
 * two non-fixture files permitted to contain a legacy brand string.
 */

/** Every canonical brand identifier the app writes. */
export interface BrandCanonical {
  /** `ConversationRecordV2.createdBy` discriminant. */
  readonly createdBy: string
  /** Markdown fence language carrying an ACP plan payload. */
  readonly planFence: string
  /** Bundled dark color-theme id (also the default theme id). */
  readonly themeId: string
  /** Bundled light color-theme family id. */
  readonly themeFamilyLight: string
  /** `terminalUrlOpenMode` enum member selecting the built-in browser. */
  readonly urlOpenMode: string
  /** `localStorage` namespace prefix used by the web plugin-store stub. */
  readonly storagePrefix: string
  /** `localStorage` prefix for ad-hoc renderer keys (`<prefix>some-key`). */
  readonly storageKeyPrefix: string
  /** Prefix of every custom DOM event name the renderer dispatches. */
  readonly eventPrefix: string
  /** Prefix of every CSS custom property the renderer sets. */
  readonly cssVarPrefix: string
  /** Prefix of every global/element id injected into third-party pages. */
  readonly domGlobalPrefix: string
  /** Negotiated binary WebSocket subprotocol. */
  readonly wsSubprotocol: string
  /** Prefix of every environment variable the app reads. */
  readonly envPrefix: string
  /** Per-user-repository workspace directory name. */
  readonly workspaceDir: string
  /** Short display name; also the `~/Documents/<name>` path component. */
  readonly displayName: string
  /** Full display name / product name. */
  readonly displayNameFull: string
  /** Desktop binary and npm/cargo package name. */
  readonly packageName: string
  /** Standalone headless server binary name. */
  readonly serverBinary: string
  /** Production bundle identifier. */
  readonly bundleId: string
  /** Development bundle identifier. */
  readonly bundleIdDev: string
  /** Deep-link URL scheme (without `://`). */
  readonly deepLinkScheme: string
  /** Base file name (no extension) of the desktop log file. */
  readonly logFileName: string
  /** `log`/`tracing` target prefix and `RUST_LOG` directive stem. */
  readonly logTarget: string
  /** Keychain service holding desktop general credentials. */
  readonly keychainService: string
  /** Keychain service holding SSH passwords and key passphrases. */
  readonly keychainSshService: string
  /** Keychain service holding the iOS pairing secret. */
  readonly keychainPairingService: string
  /** MCP server name exposed to agents. */
  readonly mcpServerName: string
  /** Name of the managed scheduled-tasks agent skill. */
  readonly skillName: string
  /** HTML marker identifying a skill file this app wrote. */
  readonly skillMarker: string
  /**
   * On-disk key of the managed-skill manifest's ownership flag
   * (`.se-manager/managed-skills.json`).
   *
   * Mirrors `brand.rs`'s `skill_manifest_key`. The Rust struct that owns this
   * file carries `#[serde(rename_all = "camelCase", deny_unknown_fields)]`, so
   * the field identifier *is* the JSON key and there is no literal to grep for;
   * `deny_unknown_fields` means an existing user manifest stops deserializing
   * the moment the key moves. Held here so both sides keep an identical field
   * set — nothing in the renderer reads this file.
   */
  readonly skillManifestKey: string
  /** frp `[[proxies]]` registration name. */
  readonly frpProxyName: string
  /** Standalone-server state root directory name (unix, lowercase). */
  readonly stateDir: string
  /** iOS `UserDefaults` key prefix. */
  readonly iosDefaultsPrefix: string
  /** iOS on-disk cache directory name. */
  readonly iosCacheDir: string
}

/**
 * Values already written to user disks. Permanent, read-only, never re-emitted.
 *
 * Migration and compatibility-read paths are the *only* legitimate consumers.
 */
export const LEGACY: BrandCanonical = {
  createdBy: 'termul',
  planFence: 'termul-plan',
  themeId: 'termul',
  themeFamilyLight: 'termul-light',
  urlOpenMode: 'termul',
  storagePrefix: 'termul-store:',
  storageKeyPrefix: 'termul:',
  eventPrefix: 'termul:',
  cssVarPrefix: '--termul-',
  domGlobalPrefix: '__termul',
  wsSubprotocol: 'termul-terminal-v2.binary',
  envPrefix: 'TERMUL_',
  workspaceDir: '.termul',
  displayName: 'Termul',
  displayNameFull: 'Termul Manager',
  packageName: 'termul-manager',
  serverBinary: 'termul-server',
  bundleId: 'com.termul-manager.app',
  bundleIdDev: 'com.termul-manager.app.dev',
  deepLinkScheme: 'termul',
  logFileName: 'termul',
  logTarget: 'termul',
  keychainService: 'com.termul.manager',
  keychainSshService: 'termul-ssh',
  keychainPairingService: 'com.termul.remote.pairing',
  mcpServerName: 'termul',
  skillName: 'termul-scheduled-tasks',
  skillMarker: '<!-- managed-by-termul:termul-scheduled-tasks -->',
  skillManifestKey: 'managedByTermul',
  frpProxyName: 'termul',
  stateDir: 'termul',
  iosDefaultsPrefix: 'termul.',
  iosCacheDir: 'TermulRemote'
} as const

/**
 * Values the app writes today.
 *
 * Wave 5 flips these one contract at a time. Until a contract's flip task
 * lands, its entry here still equals the corresponding {@link LEGACY} value —
 * that gap is precisely what makes the Wave-1 harness tests go red.
 */
const DEFAULT_CANONICAL: BrandCanonical = {
  createdBy: 'se-manager',
  planFence: 'se-plan',
  themeId: 'se',
  themeFamilyLight: 'se-light',
  urlOpenMode: 'se',
  storagePrefix: 'se-store:',
  storageKeyPrefix: 'se:',
  eventPrefix: 'se:',
  cssVarPrefix: '--se-',
  domGlobalPrefix: '__se',
  wsSubprotocol: 'se-terminal-v2.binary',
  envPrefix: 'SE_',
  workspaceDir: '.se-manager',
  displayName: 'Se',
  displayNameFull: 'Se Manager',
  packageName: 'se-manager',
  serverBinary: 'se-server',
  bundleId: 'com.se-manager.app',
  bundleIdDev: 'com.se-manager.app.dev',
  deepLinkScheme: 'termul',
  logFileName: 'se-manager',
  logTarget: 'termul',
  keychainService: 'com.se-manager.app',
  keychainSshService: 'com.se-manager.ssh',
  keychainPairingService: 'com.termul.remote.pairing',
  mcpServerName: 'se-manager',
  skillName: 'se-manager-scheduled-tasks',
  skillMarker: '<!-- managed-by-se-manager:se-manager-scheduled-tasks -->',
  skillManifestKey: 'managedBySeManager',
  frpProxyName: 'se-manager',
  stateDir: 'se-manager',
  iosDefaultsPrefix: 'termul.',
  iosCacheDir: 'TermulRemote'
} as const

let override: Partial<BrandCanonical> | null = null

/**
 * The canonical brand values in force right now.
 *
 * Always call this rather than capturing a field at module scope — a value
 * captured into a top-level `const` freezes before a test can override it.
 */
export function brandCanonical(): BrandCanonical {
  return override ? { ...DEFAULT_CANONICAL, ...override } : DEFAULT_CANONICAL
}

/**
 * Test seam: force some canonical values for the duration of a test.
 *
 * Production never calls this. Harness tests use it to inject the *post*-rename
 * value while production still emits the pre-rename one, so the resulting red
 * reflects a real missing capability instead of a stale literal.
 */
export function __setBrandCanonicalOverride(next: Partial<BrandCanonical> | null): void {
  override = next
}

/** Restore the shipped canonical values. */
export function __resetBrandCanonicalOverride(): void {
  override = null
}

/**
 * The values a compatibility read must accept for `field`, most-current first.
 *
 * A read that knows only the canonical value drops everything already on disk;
 * one that knows only the legacy value stops working the moment the contract
 * flips. This is the one shape that survives both, and it collapses to a single
 * entry while the two spellings are still equal — so no caller pays for a
 * second lookup before the flip.
 *
 * Compatibility *read* only. A caller that writes must name the value it writes
 * explicitly; FORBID-04 forbids emitting two spellings of the same contract.
 *
 * Call it at the point of use. A list captured into a module-level `const`
 * freezes before {@link __setBrandCanonicalOverride} can move it.
 */
export function acceptedBrandValues<K extends keyof BrandCanonical>(field: K): readonly string[] {
  const canonical = brandCanonical()[field]
  return canonical === LEGACY[field] ? [canonical] : [canonical, LEGACY[field]]
}

/**
 * {@link acceptedBrandValues} as a regexp alternation body, ready to drop into
 * a `(?:…)` group.
 *
 * Every value is escaped: brand identifiers carry `.` (`com.termul.manager`)
 * and `-` prefixes, and an unescaped `.` in an alternation quietly matches any
 * character. Build the `RegExp` in the function body that uses it — a pattern
 * hoisted to module scope freezes with the brand value baked in.
 */
export function acceptedBrandPattern<K extends keyof BrandCanonical>(field: K): string {
  return acceptedBrandValues(field)
    .map((value) => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))
    .join('|')
}
