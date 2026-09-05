/**
 * Brand-migration facade (T-MIG-UI).
 *
 * Three desktop-only commands sit behind this module:
 *
 * - `detect_legacy_brand_data` — read-only probe for data left on disk / in the
 *   keychain by the previous brand. Runs on every desktop start.
 * - `brand_migration_last_run` — read-only journal lookup. It is what decides
 *   whether to prompt: detection reports that legacy data *exists*, which stays
 *   true forever because the merge never deletes anything, so only the journal
 *   can say whether work is still owed.
 * - `run_brand_migration` — the copy pass. It is NEVER triggered automatically;
 *   the user starts it from `BrandMigrationBanner` or from Settings → Data
 *   migration. It copies and never deletes, and is safe to repeat.
 *
 * `sshKnownHosts` is deliberately NOT part of `run_brand_migration`. That one
 * root is migrated unconditionally during startup because host-key checking is
 * fail-closed without it; `detect_legacy_brand_data` only *reports* what that
 * startup pass did. Re-running the prompt must not re-run it.
 *
 * Browser/remote clients have no legacy desktop root of their own, so the
 * browser impl reports "nothing to migrate" instead of throwing — the banner
 * then renders nothing at all on that surface (AGENTS.md: gate platform-only
 * capability with `isTauriContext()` and give the unsupported state a real
 * shape rather than a throwing stub).
 *
 * Both commands return the house `IpcResult<T>` envelope from Rust, like every
 * other Tauri facade here. The two outcomes are deliberately asymmetric:
 * a failed *probe* is not a failed migration, so detection degrades to `null`
 * plus a warn rather than painting an error over the workspace; a failed *run*
 * throws so the banner can show it.
 */

import type { IpcResult } from '@shared/types/ipc.types'
import { type InvokeArgs, invoke } from '@tauri-apps/api/core'
import { logFrontendError } from './log-api'
import { isTauriContext } from './tauri-runtime'

/** Every legacy root the detector knows how to look for. */
export type LegacySignalKind =
  | 'appDataDir'
  | 'documentsWorkspace'
  | 'standaloneStateRoot'
  | 'keychainService'
  | 'localStorage'
  | 'repoWorkspaceDir'
  | 'sshKnownHosts'

/** One probed root. `present: false` means the detector looked and found nothing. */
export interface LegacyDataSignal {
  kind: LegacySignalKind
  /** Host-rendered human label; the renderer does not re-derive it. */
  label: string
  /** Absolute path, or `null` for non-path roots (keychain, localStorage). */
  path: string | null
  present: boolean
}

/** Outcome of the unconditional startup migration of SSH `known_hosts`. */
export type SshKnownHostsStatus =
  | { state: 'migrated' }
  | { state: 'skipped' }
  | { state: 'notApplicable' }
  | { state: 'failed'; reason: string }

/** Payload of `detect_legacy_brand_data`. */
export interface LegacyDataDetection {
  hasLegacyData: boolean
  signals: LegacyDataSignal[]
  /** Result of the STARTUP migration — already done, not part of the prompt. */
  sshKnownHosts: SshKnownHostsStatus
  /** macOS-only privacy-grant notice; `null` on every other platform. */
  tccNotice: string | null
}

/** Per-root outcome in a `run_brand_migration` receipt. */
export type BrandMigrationRootStatus = 'migrated' | 'skipped' | 'notApplicable' | 'failed'

export interface BrandMigrationRootReceipt {
  kind: LegacySignalKind
  label: string
  status: BrandMigrationRootStatus
  reason: string | null
}

/** Payload of `run_brand_migration`. */
export interface BrandMigrationReceipt {
  roots: BrandMigrationRootReceipt[]
}

/**
 * A plan root with no `LegacySignalKind` of its own, carried in the journal
 * because it has no receipt row to live in (the abandoned registry cache, the
 * macOS privacy grants, the frp proxy name).
 */
export interface BrandMigrationNotice {
  id: string
  status: BrandMigrationRootStatus
  detail: string
}

/** One recorded pass, as `brand_migration_last_run` returns it. */
export interface BrandMigrationRun {
  runId: string
  startedAtUtc: string
  roots: BrandMigrationRootReceipt[]
  notices: BrandMigrationNotice[]
}

/** Whether `run` left anything unfinished. */
export function hasFailedRoots(run: BrandMigrationRun | null): boolean {
  return run !== null && run.roots.some((root) => root.status === 'failed')
}

export interface BrandMigrationApi {
  /**
   * Probe for legacy data. Returns `null` when there is nothing to decide
   * about — no Tauri runtime, or the probe itself failed. A `null` keeps the
   * banner off the screen; it never turns into a user-visible error, because a
   * failed *probe* is not a failed migration.
   */
  detectLegacyData(): Promise<LegacyDataDetection | null>
  /**
   * The most recent recorded pass, or `null` when the merge has never run on
   * this host.
   *
   * This — not `detectLegacyData` — is what answers "is anything still owed".
   * Every legacy root survives the merge on purpose, so `hasLegacyData` stays
   * true forever and a prompt keyed on it alone would return at every start for
   * the rest of the install's life. Degrades to `null` on any failure, for the
   * same reason detection does.
   */
  lastRun(): Promise<BrandMigrationRun | null>
  /**
   * Run the user-initiated copy pass. Rejects when the host reports a failure
   * so the caller can surface it; the caller owns the retry decision.
   */
  runMigration(): Promise<BrandMigrationReceipt>
}

/**
 * IPC command names. These MUST match the `#[tauri::command]` declarations on
 * the Rust side.
 */
const IPC_COMMANDS = {
  DETECT: 'detect_legacy_brand_data',
  LAST_RUN: 'brand_migration_last_run',
  RUN: 'run_brand_migration'
} as const

/**
 * Invoke Tauri IPC commands that return `IpcResult<T>` from Rust.
 *
 * Same wrapper every other Tauri facade in this directory carries (see
 * `tauri-data-migration-api.ts`, `tauri-acp-install-api.ts`): a thrown invoke
 * becomes `{ success: false, code: 'INVOKE_ERROR' }` so no caller ever sees an
 * exception escape the IPC layer.
 */
async function invokeIpc<T>(command: string, args?: InvokeArgs): Promise<IpcResult<T>> {
  try {
    return await invoke<IpcResult<T>>(command, args)
  } catch (error) {
    return {
      success: false,
      error: error instanceof Error ? error.message : String(error),
      code: 'INVOKE_ERROR'
    }
  }
}

/** Build the Tauri IPC impl of [`BrandMigrationApi`]. */
export function createTauriBrandMigrationApi(): BrandMigrationApi {
  return {
    async detectLegacyData(): Promise<LegacyDataDetection | null> {
      if (!isTauriContext()) return null
      const result = await invokeIpc<LegacyDataDetection>(IPC_COMMANDS.DETECT)
      if (!result.success) {
        void logFrontendError({
          level: 'warn',
          source: 'brand-migration.detect',
          message: `${IPC_COMMANDS.DETECT} failed: ${result.error} (${result.code})`
        })
        return null
      }
      return result.data
    },
    async lastRun(): Promise<BrandMigrationRun | null> {
      if (!isTauriContext()) return null
      const result = await invokeIpc<BrandMigrationRun | null>(IPC_COMMANDS.LAST_RUN)
      if (!result.success) {
        void logFrontendError({
          level: 'warn',
          source: 'brand-migration.lastRun',
          message: `${IPC_COMMANDS.LAST_RUN} failed: ${result.error} (${result.code})`
        })
        return null
      }
      return result.data
    },
    async runMigration(): Promise<BrandMigrationReceipt> {
      if (!isTauriContext()) {
        throw new Error(`${IPC_COMMANDS.RUN} requires the Tauri runtime`)
      }
      const result = await invokeIpc<BrandMigrationReceipt>(IPC_COMMANDS.RUN)
      if (!result.success) {
        throw new Error(result.error)
      }
      return result.data
    }
  }
}

/**
 * Browser/remote impl. There is no legacy desktop data reachable from a browser
 * client, so detection reports "nothing" and the migration is refused loudly
 * rather than silently reporting an empty success.
 */
export const browserBrandMigrationApi: BrandMigrationApi = {
  async detectLegacyData(): Promise<LegacyDataDetection | null> {
    return null
  },
  async lastRun(): Promise<BrandMigrationRun | null> {
    return null
  },
  async runMigration(): Promise<BrandMigrationReceipt> {
    throw new Error(`${IPC_COMMANDS.RUN} is desktop-only`)
  }
}

/** Singleton facade — Tauri IPC on the desktop, the inert impl in a browser. */
export const brandMigrationApi: BrandMigrationApi = isTauriContext()
  ? createTauriBrandMigrationApi()
  : browserBrandMigrationApi
