/**
 * macOS privacy (TCC) probe facade.
 *
 * All native access stays behind this module per the project's adapter-boundary
 * rule — the settings panel imports from here, never `invoke` directly. There is
 * no web/remote counterpart on purpose: the answer describes the machine the
 * desktop app is installed on, and a browser client has nothing to report.
 */

import { invoke } from '@tauri-apps/api/core'
import { isTauriContext } from './tauri-runtime'

/** Ids are minted in `src-tauri/src/macos_permissions.rs` and mirrored here. */
export type PermissionId =
  | 'fullDiskAccess'
  | 'accessibility'
  | 'screenRecording'
  | 'inputMonitoring'
  | 'localNetwork'
  | 'desktopFolder'
  | 'documentsFolder'
  | 'downloadsFolder'

export type PermissionState = 'granted' | 'denied' | 'unknown' | 'notProbed' | 'notRequired'

export interface PermissionProbe {
  id: PermissionId
  state: PermissionState
  /** Raw evidence — an errno name, a path, an API return code. */
  detail: string | null
  /** True when running this probe can make macOS show a system prompt. */
  active: boolean
}

export type SigningKind = 'developerId' | 'appleDevelopment' | 'adhoc' | 'unsigned' | 'unknown'

export interface SigningIdentity {
  kind: SigningKind
  teamId: string | null
  /** False when every rebuild produces a new code identity, discarding grants. */
  grantsSurviveRebuild: boolean
}

export interface PermissionReport {
  supported: boolean
  osVersion: string | null
  bundleId: string | null
  signing: SigningIdentity | null
  probes: PermissionProbe[]
}

/** The report shown when there is nothing to report — never a thrown error. */
const UNSUPPORTED: PermissionReport = {
  supported: false,
  osVersion: null,
  bundleId: null,
  signing: null,
  probes: []
}

/**
 * Read the current grants.
 *
 * `active` names the probes the user has consented to run despite their side
 * effect (a system prompt); everything else comes back as `notProbed`. Pass an
 * empty list — the default — for the passive sweep that is safe on mount.
 */
export async function fetchPermissionReport(
  active: readonly PermissionId[] = []
): Promise<PermissionReport> {
  if (!isTauriContext()) return UNSUPPORTED
  return await invoke<PermissionReport>('macos_permissions_report_command', {
    active: [...active]
  })
}

/**
 * Open the System Settings pane that governs `id`.
 *
 * Takes an id rather than a URL so the backend keeps the only copy of the
 * scheme-bearing deep links.
 */
export async function openPrivacyPane(id: PermissionId): Promise<void> {
  if (!isTauriContext()) return
  await invoke('macos_open_privacy_pane_command', { id })
}
