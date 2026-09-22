import type { IpcResult } from './ipc.types'

export const UPDATE_COMPONENTS = ['renderer', 'guiNative', 'acpCore', 'terminalCore'] as const

export type UpdateComponent = (typeof UPDATE_COMPONENTS)[number]

export type UpdateComponentAction = 'preserve' | 'restart' | 'defer-if-active' | 'unsupported'

export interface UpdateComponentPolicyEntry {
  buildId: string
  action: UpdateComponentAction
}

export interface UpdateComponentPolicy {
  schemaVersion: 1
  targetVersion: string
  metadataState: 'declared' | 'legacy'
  components: Record<UpdateComponent, UpdateComponentPolicyEntry>
}

export type PendingUpdatePlanStatus =
  | 'prepared'
  | 'reconciling'
  | 'deferred'
  | 'completed'
  | 'failed'

export interface PendingUpdatePlan {
  schemaVersion: 1
  targetVersion: string
  componentPolicy: UpdateComponentPolicy
  status: PendingUpdatePlanStatus
  currentComponentBuildIds: Partial<Record<UpdateComponent, string>>
  requiredActions: UpdateComponent[]
  deferredComponents: UpdateComponent[]
  createdAt: string
  updatedAt: string
  lastError?: string
}

export function legacyUpdateComponentPolicy(version: string): UpdateComponentPolicy {
  const components = Object.fromEntries(
    UPDATE_COMPONENTS.map((component) => [
      component,
      {
        buildId: `legacy:${version}:${component}`,
        action: component === 'terminalCore' ? 'defer-if-active' : 'restart'
      }
    ])
  ) as Record<UpdateComponent, UpdateComponentPolicyEntry>

  return {
    schemaVersion: 1,
    targetVersion: version,
    metadataState: 'legacy',
    components
  }
}

export function parseUpdateComponentPolicy(value: unknown, version: string): UpdateComponentPolicy {
  if (value === undefined) return legacyUpdateComponentPolicy(version)
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error('Update component policy must be an object')
  }

  const candidate = value as {
    schemaVersion?: unknown
    targetVersion?: unknown
    metadataState?: unknown
    components?: unknown
  }
  if (candidate.schemaVersion !== 1) {
    throw new Error('Update component policy schemaVersion must be 1')
  }
  if (candidate.targetVersion !== version) {
    throw new Error('Update component policy targetVersion does not match the update version')
  }
  if (candidate.metadataState !== 'declared' && candidate.metadataState !== 'legacy') {
    throw new Error('Update component policy metadataState is invalid')
  }
  if (
    typeof candidate.components !== 'object' ||
    candidate.components === null ||
    Array.isArray(candidate.components)
  ) {
    throw new Error('Update component policy components must be an object')
  }

  const componentRecord = candidate.components as Record<string, unknown>
  const keys = Object.keys(componentRecord).sort()
  const expectedKeys = [...UPDATE_COMPONENTS].sort()
  if (
    keys.length !== expectedKeys.length ||
    keys.some((key, index) => key !== expectedKeys[index])
  ) {
    throw new Error('Update component policy components are incomplete or contain unknown entries')
  }

  const components = {} as Record<UpdateComponent, UpdateComponentPolicyEntry>
  for (const component of UPDATE_COMPONENTS) {
    const entry = componentRecord[component]
    if (typeof entry !== 'object' || entry === null || Array.isArray(entry)) {
      throw new Error(`Update component policy entry is invalid: ${component}`)
    }
    const candidateEntry = entry as { buildId?: unknown; action?: unknown }
    if (typeof candidateEntry.buildId !== 'string' || candidateEntry.buildId.trim() === '') {
      throw new Error(`Update component policy buildId is invalid: ${component}`)
    }
    if (
      candidateEntry.action !== 'preserve' &&
      candidateEntry.action !== 'restart' &&
      candidateEntry.action !== 'defer-if-active' &&
      candidateEntry.action !== 'unsupported'
    ) {
      throw new Error(`Update component policy action is invalid: ${component}`)
    }
    components[component] = {
      buildId: candidateEntry.buildId.trim(),
      action: candidateEntry.action
    }
  }

  return {
    schemaVersion: 1,
    targetVersion: version,
    metadataState: candidate.metadataState,
    components
  }
}

// Updater information for available updates
export interface UpdateInfo {
  version: string
  componentPolicy?: UpdateComponentPolicy
  // Optional: channel manifests may omit `pub_date`; producers must NOT
  // fabricate a "now" timestamp for a stale manifest (which would make it look
  // just-published). Stable/AUR producers always set it from real metadata.
  releaseDate?: string
  releaseNotes?: string
  isSecurityUpdate: boolean
  downloadUrl?: string
}

// Current state of the updater
export interface UpdateState {
  updateAvailable: boolean
  downloaded: boolean
  version: string | null
  isChecking: boolean
  isDownloading: boolean
  downloadProgress: DownloadProgress | null
  error: string | null
  lastChecked: string | null // ISO timestamp
  componentPolicy?: UpdateComponentPolicy
  pendingUpdatePlan?: PendingUpdatePlan | null
  isManualUpdateMode?: boolean
}

// Data stored when user skips a version
export interface SkipVersionData {
  version: string
  skippedAt: string // ISO timestamp
}

// Download progress information
export interface DownloadProgress {
  bytesPerSecond: number
  percent: number
  transferred: number
  total: number
}

// Error codes for updater operations
export const UpdaterErrorCodes = {
  NETWORK_ERROR: 'NETWORK_ERROR',
  OFFLINE: 'OFFLINE',
  UPDATE_NOT_AVAILABLE: 'UPDATE_NOT_AVAILABLE',
  DOWNLOAD_FAILED: 'DOWNLOAD_FAILED',
  INSTALL_FAILED: 'INSTALL_FAILED',
  INVALID_UPDATE_CHANNEL: 'INVALID_UPDATE_CHANNEL',
  PERMISSION_DENIED: 'PERMISSION_DENIED',
  DISK_SPACE_INSUFFICIENT: 'DISK_SPACE_INSUFFICIENT',
  INVALID_UPDATE_INFO: 'INVALID_UPDATE_INFO',
  UPDATE_CHECK_FAILED: 'UPDATE_CHECK_FAILED',
  UNKNOWN_ERROR: 'UNKNOWN_ERROR'
} as const

export type UpdaterErrorCode = (typeof UpdaterErrorCodes)[keyof typeof UpdaterErrorCodes]

// IPC channel definitions for updater
export type UpdaterIpcChannels = {
  'updater:checkForUpdates': () => IpcResult<UpdateInfo | null>
  'updater:downloadUpdate': () => IpcResult<void>
  'updater:installAndRestart': () => IpcResult<void>
  'updater:skipVersion': (version: string) => IpcResult<void>
  'updater:getState': () => IpcResult<UpdateState>
  'updater:setAutoUpdateEnabled': (enabled: boolean) => IpcResult<void>
  'updater:getAutoUpdateEnabled': () => IpcResult<boolean>
}

// Event types for main -> renderer communication
export type UpdateAvailableCallback = (info: UpdateInfo) => void
export type UpdateDownloadedCallback = (info: UpdateInfo) => void
export type DownloadProgressCallback = (progress: DownloadProgress) => void
export type UpdaterErrorCallback = (error: string, code: UpdaterErrorCode) => void

// Updater API exposed via preload
export interface UpdaterApi {
  checkForUpdates: () => Promise<IpcResult<UpdateInfo | null>>
  downloadUpdate: () => Promise<IpcResult<void>>
  installAndRestart: () => Promise<IpcResult<void>>
  skipVersion: (version: string) => Promise<IpcResult<void>>
  getState: () => Promise<IpcResult<UpdateState>>
  setAutoUpdateEnabled: (enabled: boolean) => Promise<IpcResult<void>>
  getAutoUpdateEnabled: () => Promise<IpcResult<boolean>>
  onUpdateAvailable: (callback: UpdateAvailableCallback) => () => void
  onUpdateDownloaded: (callback: UpdateDownloadedCallback) => () => void
  onDownloadProgress: (callback: DownloadProgressCallback) => () => void
  onError: (callback: UpdaterErrorCallback) => () => void
}
