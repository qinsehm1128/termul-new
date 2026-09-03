/**
 * Brand-migration facade (T-MIG-UI).
 *
 * Two desktop-only commands sit behind this module:
 *
 * - `detect_legacy_brand_data` — read-only probe for data left on disk / in the
 *   keychain by the previous brand. Runs on every desktop start; the renderer
 *   uses its result to decide whether to prompt.
 * - `run_brand_migration` — the copy pass. It is NEVER triggered automatically;
 *   the user starts it from `BrandMigrationBanner`. It copies and never deletes.
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

export interface BrandMigrationApi {
  /**
   * Probe for legacy data. Returns `null` when there is nothing to decide
   * about — no Tauri runtime, or the probe itself failed. A `null` keeps the
   * banner off the screen; it never turns into a user-visible error, because a
   * failed *probe* is not a failed migration.
   */
  detectLegacyData(): Promise<LegacyDataDetection | null>
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
  async runMigration(): Promise<BrandMigrationReceipt> {
    throw new Error(`${IPC_COMMANDS.RUN} is desktop-only`)
  }
}

/** Singleton facade — Tauri IPC on the desktop, the inert impl in a browser. */
export const brandMigrationApi: BrandMigrationApi = isTauriContext()
  ? createTauriBrandMigrationApi()
  : browserBrandMigrationApi
