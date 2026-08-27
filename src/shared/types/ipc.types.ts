// IPC Result pattern from architecture.md
import type { AcpCatalog } from './acp-catalog.types'
import type { ConversationId, ConversationRecordV2 } from './conversation.types'
import type {
  ConversationHostStatus,
  ConversationOpenOutcome,
  LegacyConversationKey,
  LegacyConversationResolution
} from './conversation-api.types'
import type {
  ConversationLifecycleOutcome,
  ConversationReplacementRequest
} from './conversation-lifecycle.types'
import type {
  RecoveryActionResult,
  ResolveRecoveryItemRequest
} from './conversation-recovery.types'
import type {
  SessionWorkspaceLoadOutcome,
  SessionWorkspaceV1,
  SessionWorkspaceWriteOutcome
} from './session-workspace.types'
import type { ConversationHistoryPageV1, GetSessionPayloadPageRequest } from './web-protocol.types'
import type { WorkspaceManifest, WriteOutcome } from './workspace-manifest.types'

export type IpcResult<T> =
  | { success: true; data: T }
  | { success: false; error: string; code: string }

export type IpcDataDecoder<T> = (value: unknown) => T

/**
 * Decode the exact runtime-neutral application envelope used by Tauri and HTTP.
 *
 * Only `{ success: true, data }` and `{ success: false, error, code }` are accepted. The supplied
 * domain decoder runs exactly once for success data; valid objects are returned by identity when
 * the decoder preserves the data identity.
 */
export function decodeIpcResult<T>(value: unknown, decodeData: IpcDataDecoder<T>): IpcResult<T> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new TypeError('IPC result must be an object')
  }
  const candidate = value as Record<string, unknown>
  const keys = Object.keys(candidate)
  if (candidate.success === true) {
    if (
      keys.length !== 2 ||
      !Object.prototype.hasOwnProperty.call(candidate, 'success') ||
      !Object.prototype.hasOwnProperty.call(candidate, 'data')
    ) {
      throw new TypeError('IPC success envelope must contain exactly success and data')
    }
    const data = decodeData(candidate.data)
    return data === candidate.data ? (value as IpcResult<T>) : { success: true, data }
  }
  if (candidate.success === false) {
    if (
      keys.length !== 3 ||
      !Object.prototype.hasOwnProperty.call(candidate, 'success') ||
      !Object.prototype.hasOwnProperty.call(candidate, 'error') ||
      !Object.prototype.hasOwnProperty.call(candidate, 'code') ||
      typeof candidate.error !== 'string' ||
      candidate.error.trim().length === 0 ||
      typeof candidate.code !== 'string' ||
      candidate.code.trim().length === 0
    ) {
      throw new TypeError('IPC failure envelope must contain exact non-empty error and code')
    }
    return value as IpcResult<T>
  }
  throw new TypeError('IPC result success must be a boolean discriminator')
}

// Terminal spawn options
export interface TerminalSpawnOptions {
  shell?: string
  cwd?: string
  env?: Record<string, string>
  cols?: number
  rows?: number
  // ADR-004.2: terminal-native agent launch.
  // When `program` is set, the PTY runs that executable directly (with `args`
  // as discrete argv entries) instead of resolving a login shell. The prompt is
  // always a single element of `args` — never shell-interpolated. When `program`
  // is omitted, spawn behavior is byte-for-byte identical to the shell path.
  /** Absolute path or PATH-resolvable executable to run instead of a shell. */
  program?: string
  /** argv tail; each element is passed as a discrete, unescaped argument. */
  args?: string[]
  /** Descriptive marker for the session type. Defaults to 'shell'. */
  kind?: 'shell' | 'agent' | 'ssh'
  /** Canonical primary ownership/authorization scope. Required for every
   * durable user terminal; only explicitly ephemeral SSH terminals omit it. */
  conversationId?: ConversationId
  /** Optional attribution/filter only. Never terminal ownership. */
  projectId?: string
  // Index signature to satisfy Tauri's InvokeArgs constraint
  [key: string]: unknown
}

// Terminal info returned after spawn
export interface TerminalInfo {
  id: string
  shell: string
  cwd: string
}

/**
 * CAP-3 spawn response: terminal info PLUS the issued claim credential.
 * Same flattened camelCase shape on both transports (desktop `terminal_spawn`
 * IpcResult data and web `spawn` reply data). Spawn is the ONLY issuance path.
 * Mirrors the Rust `SpawnedTerminal` serde shape exactly (id/shell/cwd/pid/
 * cols/rows/claim — pinned by the Rust serde shape tests).
 */
export interface SpawnedTerminal extends TerminalInfo {
  pid: number
  cols: number
  rows: number
  /** Unguessable host-issued lease credential (64 hex chars). Present only in
   * the spawn/rotate responses — never echoed by any other operation. */
  claim: string
}

/** Catalog event when any surface creates a host PTY. Never includes a claim. */
export interface TerminalSpawnedEvent {
  terminalId: string
  projectId?: string | null
  conversationId?: string | null
  cwd: string
  cols: number
  rows: number
  shell: string
}

/** Live PTY geometry owner. Phone parks the desktop size until restore. */
export type TerminalDisplayMode = 'phone' | 'desktop'

export interface TerminalDisplayModeState {
  mode: TerminalDisplayMode
  cols: number
  rows: number
}

export interface TerminalDisplayModeChangedEvent {
  terminalId: string
  mode: TerminalDisplayMode
  cols: number
  rows: number
}

export interface TerminalDisplayModeOptions {
  cols?: number
  rows?: number
  force?: boolean
}

/**
 * CAP-3 attach response — byte-identical camelCase shape on both transports
 * (desktop `terminal_attach` IpcResult data; web `attach` reply data).
 * Carries the replay cursor (`latestSeq`) + `gap` flag. NEVER carries a
 * claim: attach consumes the credential, it never issues one.
 */
export interface TerminalAttachResult {
  id: string
  shell: string
  cwd: string
  pid: number
  cols: number
  rows: number
  latestSeq: number
  gap: boolean
}

/**
 * What a completed attach says about the backlog it just replayed.
 *
 * The renderer keeps its own detached-output transcript, which covers exactly
 * the interval during which no renderer was mounted — the same interval the
 * host replays from the stored cursor. Writing both duplicates whole blocks of
 * output, so the two are made mutually exclusive and this is the discriminator:
 * `gap === false` means the host replay fully covered `(lastSeq, latestSeq]`
 * (`PtyInstance::subscribe_from`, src-tauri/src/pty/manager.rs:1331-1333) and
 * the transcript is redundant. `null` means no host replay happened at all —
 * a live-only handoff, or an already-reconciled record — and the transcript is
 * then the only source of continuity.
 */
export interface TerminalReplayCoverage {
  latestSeq: number
  gap: boolean
}

/**
 * Cold-renderer request for a host-authorized, one-time claim rotation.
 * The narrow request cannot override spawn authority, cwd, argv, or env.
 */
export interface TerminalResumeRequest {
  conversationId: ConversationId
  terminalId: string
  lastSeq: number
}

/**
 * Authenticated resume handoff. `claim` is response-only and memory-only; it
 * must never be added to SessionWorkspace or renderer persistence.
 */
export interface TerminalResumeGrant {
  terminal: TerminalAttachResult
  claim: string
}

/** CAP-3 rotate response: the fresh credential. */
export interface RotatedClaim {
  claim: string
}

// IPC channel definitions
export type ConversationIpcChannels = {
  'conversation:host_status': () => IpcResult<ConversationHostStatus>
  'conversation:list': () => IpcResult<ConversationRecordV2[]>
  'conversation:get': (conversationId: ConversationId) => IpcResult<ConversationRecordV2>
  'conversation:open': (conversationId: ConversationId) => IpcResult<ConversationOpenOutcome>
  'conversation:resolve_legacy_id': (
    request: LegacyConversationKey
  ) => IpcResult<LegacyConversationResolution>
  'conversation:workspace:get': (
    conversationId: ConversationId
  ) => IpcResult<SessionWorkspaceLoadOutcome>
  'conversation:workspace:write': (
    conversationId: ConversationId,
    basedRevision: number | null,
    workspace: SessionWorkspaceV1
  ) => IpcResult<SessionWorkspaceWriteOutcome>
  'conversation:recovery:resolve': (
    request: ResolveRecoveryItemRequest
  ) => IpcResult<RecoveryActionResult>
  'conversation:lifecycle:detach': (
    conversationId: ConversationId,
    expectedRevision: number
  ) => IpcResult<ConversationLifecycleOutcome>
  'conversation:lifecycle:rebind': (
    conversationId: ConversationId,
    expectedRevision: number
  ) => IpcResult<ConversationLifecycleOutcome>
  'conversation:lifecycle:suspend': (
    conversationId: ConversationId,
    expectedRevision: number
  ) => IpcResult<ConversationLifecycleOutcome>
  'conversation:lifecycle:replace': (
    conversationId: ConversationId,
    request: ConversationReplacementRequest,
    expectedRevision: number
  ) => IpcResult<ConversationLifecycleOutcome>
  'conversation:lifecycle:delete': (
    conversationId: ConversationId,
    expectedRevision: number
  ) => IpcResult<ConversationLifecycleOutcome>
}

export type AcpHistoryIpcChannels = {
  /** Compatibility full-payload command; large histories may return paging-required. */
  'acp:history:get': (sessionId: string) => IpcResult<unknown | null>
  'acp:history:get_page': (
    request: GetSessionPayloadPageRequest
  ) => IpcResult<ConversationHistoryPageV1>
}

export type TerminalIpcChannels = {
  'terminal:spawn': (options: TerminalSpawnOptions) => IpcResult<SpawnedTerminal>
  'terminal:resume': (request: TerminalResumeRequest) => IpcResult<TerminalResumeGrant>
  'terminal:attach': (
    terminalId: string,
    claim: string,
    lastSeq: number
  ) => IpcResult<TerminalAttachResult>
  'terminal:watch': (terminalId: string, lastSeq: number) => IpcResult<TerminalAttachResult>
  'terminal:rotate_claim': (terminalId: string, claim: string) => IpcResult<RotatedClaim>
  'terminal:revoke_claim': (terminalId: string, claim: string) => IpcResult<void>
  'terminal:write': (terminalId: string, data: string) => IpcResult<void>
  'terminal:resize': (terminalId: string, cols: number, rows: number) => IpcResult<void>
  'terminal:close_view': (terminalId: string) => IpcResult<void>
  'terminal:terminate': (terminalId: string) => IpcResult<void>
  /** @deprecated compatibility alias for terminal:terminate */
  'terminal:kill': (terminalId: string) => IpcResult<void>
}

// CAP-5 / Story 5: Workspace manifest IPC channels. Mirrors the three Tauri
// commands (`workspace_manifest_get` / `_write` / `_delete`) and the three
// HTTP routes (`GET /workspace/:projectId`, `POST /workspace/:projectId/write`,
// `POST /workspace/:projectId/delete`) — both transports return the SAME
// `IpcResult<...>` shape byte-for-byte. Conflict is a SUCCESS body variant of
// `WriteOutcome` (NOT an error code); an excluded-field payload (`envVars`,
// raw `claim`, `fullscreenPaneId`) fails serde `deny_unknown_fields` and maps
// to `VALIDATION_ERROR` with no state change.
//
// Patch 11: the channel keys use the colon-separated pattern
// (`workspace:manifest:get`, etc.) to mirror the existing
// `TerminalIpcChannels` (`terminal:spawn`, `terminal:attach`, …). The Tauri
// adapter's IPC_COMMANDS map uses the underscored Rust command names
// (`workspace_manifest_get`); the channel map keys are a documentation /
// type-safety surface, not the literal invoke() strings.
export type WorkspaceManifestIpcChannels = {
  'workspace:manifest:get': (projectId: string) => IpcResult<WorkspaceManifest | null>
  'workspace:manifest:write': (
    projectId: string,
    basedRevision: number | null,
    manifest: WorkspaceManifest
  ) => IpcResult<WriteOutcome>
  'workspace:manifest:delete': (projectId: string) => IpcResult<void>
}

// CAP-6 / Story 8: ACP catalog IPC channels. Mirrors the two Tauri commands
// (`acp_list_catalog` / `acp_set_catalog_opt_in`) and the two HTTP routes
// (`GET /acp/catalog` / `POST /acp/catalog/opt-in`) — both transports return
// the SAME `IpcResult<...>` shape byte-for-byte. The catalog is
// credential-free, path-free, read-only host introspection. The opt-in is a
// single boolean that gates CDN registry augmentation.
export type AcpCatalogIpcChannels = {
  'acp:catalog:list': (refresh?: boolean) => IpcResult<AcpCatalog>
  'acp:catalog:set_opt_in': (enabled: boolean) => IpcResult<void>
}

// CAP-6 / Story 9: ACP install IPC channel. Mirrors the Tauri command
// `acp_install_agent` and the HTTP route `POST /acp/install` — both transports
// return the SAME `IpcResult<InstallOutcome>` shape byte-for-byte. The request
// is `{ agentId }` only; the host resolves everything from the trusted catalog.
// The declared channel type is honest about the actual invoke payload shape:
// the Tauri adapter wraps `agentId` in a `request` object (Tauri's convention
// for single-struct-arg commands), so the channel signature reflects that.
import type { InstallOutcome } from './acp-install.types'
export type AcpInstallIpcChannels = {
  'acp:install:install_agent': (request: { agentId: string }) => IpcResult<InstallOutcome>
}

// Event types for main -> renderer communication
// Terminal data callback — receives binary data as Uint8Array (via Tauri Channel)
// Previously received string via event emitter; migrated to binary Channel API in ADR-002.2
export type TerminalDataCallback = (terminalId: string, data: Uint8Array) => void
export type TerminalScopedDataCallback = (data: Uint8Array) => void

/**
 * Ownership handle for the single live writer of one PTY.
 *
 * A PTY has exactly one primary consumer — the renderer that paints it into
 * xterm. Everything else that watches the byte stream (transcript capture,
 * diagnostics) is a sidecar and must never write to the terminal. Enforcing
 * that with a `Map<terminalId, handler>` makes a second concurrent writer
 * structurally impossible rather than something a runtime flag has to arbitrate.
 *
 * Binding is deferred because the spawn path only learns its PTY id after the
 * IPC round trip, so the handler has to exist before the id does.
 */
export interface PrimaryTerminalDataHandle {
  /** Take ownership of `terminalId`, evicting any previous owner. Idempotent. */
  bind: (terminalId: string) => void
  /** Release ownership, but only if this handle still holds it. */
  dispose: () => void
}
export type TerminalExitCallback = (terminalId: string, exitCode: number, signal?: number) => void
export type TerminalCwdChangedCallback = (terminalId: string, cwd: string) => void
export type TerminalGitBranchChangedCallback = (terminalId: string, branch: string | null) => void
export type TerminalGitStatusChangedCallback = (
  terminalId: string,
  status: GitStatus | null
) => void
export type TerminalExitCodeChangedCallback = (terminalId: string, exitCode: number) => void

// Git status interface
export interface GitStatus {
  modified: number
  staged: number
  untracked: number
  ahead: number
  behind: number
  hasChanges: boolean
}

export type GitFileStatus = 'added' | 'modified' | 'deleted' | 'renamed' | 'untracked' | 'staged'

export interface GitStatusDetail {
  path: string
  status: GitFileStatus
  staged: boolean
}

// A single commit row for the history/graph view.
export interface GitCommit {
  /** Full 40-char commit hash. */
  hash: string
  /** Abbreviated commit hash. */
  shortHash: string
  /** Parent full hashes, first-parent first. Empty for the root commit. */
  parents: string[]
  /** Ref decorations (branches, tags, HEAD) attached to this commit. */
  refs: string[]
  /** Author name. */
  author: string
  /** Author date in ISO 8601 strict format. */
  date: string
  /** Commit subject (first line of the message). */
  subject: string
}

// Context for the commit footer (branch, upstream, ahead/behind, last commit).
export interface GitCommitContext {
  branch: string | null
  hasUpstream: boolean
  ahead: number
  behind: number
  stagedCount: number
  hasHead: boolean
  lastSubject: string
  lastBody: string
}

export interface GitStashInfo {
  index: number
  name: string
  message: string
}

export interface GitApi {
  getStatus: (cwd: string) => Promise<GitStatusDetail[]>
  getDiff: (cwd: string, path: string, staged?: boolean) => Promise<string>
  stage: (cwd: string, path: string) => Promise<void>
  unstage: (cwd: string, path: string) => Promise<void>
  discard: (cwd: string, path: string) => Promise<void>
  getLog: (cwd: string, limit?: number) => Promise<GitCommit[]>
  commit: (cwd: string, summary: string, description?: string, amend?: boolean) => Promise<void>
  push: (cwd: string) => Promise<void>
  getCommitContext: (cwd: string) => Promise<GitCommitContext>
  init: (cwd: string) => Promise<void>
  checkoutBranch: (cwd: string, branch: string, isRemote?: boolean) => Promise<void>
  createBranch: (cwd: string, branch: string, startRef?: string) => Promise<void>
  stashSave: (cwd: string, message?: string, includeUntracked?: boolean) => Promise<void>
  stashList: (cwd: string) => Promise<GitStashInfo[]>
  stashApply: (cwd: string, index: number) => Promise<void>
  stashPop: (cwd: string, index: number) => Promise<void>
  stashDrop: (cwd: string, index: number) => Promise<void>
  branchList: (cwd: string) => Promise<string[]>
  branchSwitch: (cwd: string, name: string) => Promise<void>
  branchCreate: (cwd: string, name: string) => Promise<void>
}

// Terminal API exposed via preload
export interface TerminalApi {
  spawn: (options?: TerminalSpawnOptions) => Promise<IpcResult<SpawnedTerminal>>
  /**
   * Resume a passive SessionWorkspace terminal reference without spawning.
   * The host validates the Conversation scope, rotates a one-time claim, and
   * replays from `lastSeq`; the returned claim remains renderer-memory-only.
   */
  resume: (request: TerminalResumeRequest) => Promise<IpcResult<TerminalResumeGrant>>
  /**
   * CAP-3: attach to a terminal's output stream with terminalId + claim +
   * lastSeq. Verification is the gate — any failure (unknown terminal,
   * missing/wrong/revoked credential) resolves to the same generic
   * UNAUTHORIZED error with no terminal metadata or output.
   */
  attach: (
    terminalId: string,
    claim: string,
    lastSeq: number
  ) => Promise<IpcResult<TerminalAttachResult>>
  /**
   * Watch a live host PTY without presenting or rotating a claim. Desktop uses
   * this for terminals the phone created; the web adapter maps to `/terminal/ws`
   * `watch`.
   */
  watch?: (terminalId: string, lastSeq: number) => Promise<IpcResult<TerminalAttachResult>>
  /** Host catalog event when any surface creates a PTY. */
  onSpawned?: (callback: (event: TerminalSpawnedEvent) => void) => () => void
  /** CAP-3: possession-based rotation — old credential invalidated atomically. */
  rotateClaim: (terminalId: string, claim: string) => Promise<IpcResult<RotatedClaim>>
  /** CAP-3: revoke the credential; the PTY keeps running. */
  revokeClaim: (terminalId: string, claim: string) => Promise<IpcResult<void>>
  write: (terminalId: string, data: string) => Promise<IpcResult<void>>
  resize: (terminalId: string, cols: number, rows: number) => Promise<IpcResult<void>>
  /**
   * Phone takeover resizes the live PTY and parks desktop FitAddon.
   * Desktop mode restores the parked size. Optional on test doubles.
   */
  setDisplayMode?: (
    terminalId: string,
    mode: TerminalDisplayMode,
    options?: TerminalDisplayModeOptions
  ) => Promise<IpcResult<TerminalDisplayModeState>>
  /** Host event when phone/desktop geometry ownership changes. */
  onDisplayModeChanged?: (callback: (event: TerminalDisplayModeChangedEvent) => void) => () => void
  /** Close/detach the renderer view without destroying the PTY or claim. */
  closeView: (terminalId: string) => Promise<IpcResult<void>>
  /** The sole user-facing destructive terminal resource operation. */
  terminate: (terminalId: string) => Promise<IpcResult<void>>
  /** @deprecated compatibility alias for terminate. */
  kill: (terminalId: string) => Promise<IpcResult<void>>
  /**
   * Observe the byte stream of every PTY without owning any of them. Sidecars
   * are read-only: transcript capture, diagnostics, metrics. Painting a PTY is
   * `registerPrimaryTerminalData`, never this.
   */
  onData: (callback: TerminalDataCallback) => () => void
  /**
   * Subscribe to one terminal without delivering unrelated PTY chunks to the
   * renderer. Optional for third-party/test adapters; callers may fall back to
   * `onData` filtering.
   */
  onDataForTerminal?: (terminalId: string, callback: TerminalScopedDataCallback) => () => void
  /**
   * Claim the single live-writer slot for a PTY. Optional so third-party/test
   * adapters keep working; `subscribeTerminalData` degrades to
   * `onDataForTerminal`/`onData` when a transport does not implement it.
   */
  registerPrimaryTerminalData?: (callback: TerminalScopedDataCallback) => PrimaryTerminalDataHandle
  onExit: (callback: TerminalExitCallback) => () => void
  onCwdChanged: (callback: TerminalCwdChangedCallback) => () => void
  getCwd: (terminalId: string) => Promise<IpcResult<string | null>>
  onGitBranchChanged: (callback: TerminalGitBranchChangedCallback) => () => void
  getGitBranch: (terminalId: string) => Promise<IpcResult<string | null>>
  onGitStatusChanged: (callback: TerminalGitStatusChangedCallback) => () => void
  getGitStatus: (terminalId: string) => Promise<IpcResult<GitStatus | null>>
  onExitCodeChanged: (callback: TerminalExitCodeChangedCallback) => () => void
  getExitCode: (terminalId: string) => Promise<IpcResult<number | null>>
  updateOrphanDetection: (enabled: boolean, timeout: number | null) => Promise<IpcResult<void>>
}

// Error codes
export const IpcErrorCodes = {
  TERMINAL_NOT_FOUND: 'TERMINAL_NOT_FOUND',
  SPAWN_FAILED: 'SPAWN_FAILED',
  WRITE_FAILED: 'WRITE_FAILED',
  RESIZE_FAILED: 'RESIZE_FAILED',
  TERMINATE_FAILED: 'TERMINATE_FAILED',
  KILL_FAILED: 'KILL_FAILED',
  DIALOG_CANCELED: 'DIALOG_CANCELED',
  VALIDATION_ERROR: 'VALIDATION_ERROR',
  UNKNOWN_ERROR: 'UNKNOWN_ERROR',
  FILE_NOT_FOUND: 'FILE_NOT_FOUND',
  FILE_TOO_LARGE: 'FILE_TOO_LARGE',
  BINARY_FILE: 'BINARY_FILE',
  PERMISSION_DENIED: 'PERMISSION_DENIED',
  WATCH_FAILED: 'WATCH_FAILED',
  PATH_INVALID: 'PATH_INVALID',
  FILE_EXISTS: 'FILE_EXISTS',
  DELETE_FAILED: 'DELETE_FAILED',
  RENAME_FAILED: 'RENAME_FAILED',
  // Worktree error codes
  WORKTREE_NOT_FOUND: 'WORKTREE_NOT_FOUND',
  WORKTREE_EXISTS: 'WORKTREE_EXISTS',
  WORKTREE_CREATE_FAILED: 'WORKTREE_CREATE_FAILED',
  WORKTREE_REMOVE_FAILED: 'WORKTREE_REMOVE_FAILED',
  BRANCH_ALREADY_HAS_WORKTREE: 'BRANCH_ALREADY_HAS_WORKTREE',
  NOT_A_GIT_REPO: 'NOT_A_GIT_REPO',
  GIT_NOT_FOUND: 'GIT_NOT_FOUND',
  PATH_TOO_LONG: 'PATH_TOO_LONG',
  // Session persistence error codes
  SESSION_NOT_FOUND: 'SESSION_NOT_FOUND',
  SESSION_INVALID: 'SESSION_INVALID',
  SESSION_STORE_ERROR: 'SESSION_STORE_ERROR',
  // Data migration error codes
  MIGRATION_VERSION_INVALID: 'MIGRATION_VERSION_INVALID',
  MIGRATION_HISTORY_CORRUPT: 'MIGRATION_HISTORY_CORRUPT',
  MIGRATION_EXECUTION_FAILED: 'MIGRATION_EXECUTION_FAILED',
  MIGRATION_ALREADY_RUNNING: 'MIGRATION_ALREADY_RUNNING',
  MIGRATION_NOT_FOUND: 'MIGRATION_NOT_FOUND',
  ROLLBACK_FAILED: 'ROLLBACK_FAILED'
} as const

export type IpcErrorCode = (typeof IpcErrorCodes)[keyof typeof IpcErrorCodes]

// ============================================================================
// Worktree Types
// ============================================================================

export interface WorktreeInfo {
  name: string
  branch: string
  path: string
  headCommit: string
}

export interface BranchInfo {
  name: string
  isRemote: boolean
  isCurrent: boolean
  upstream?: string | null
  hasOtherWorktree: boolean
}

export interface DirtyStatus {
  modified: number
  staged: number
  untracked: number
  hasChanges: boolean
}

export interface RemoveResult {
  worktreePath: string
  success: boolean
  error?: string | null
}

export interface GitignoreDir {
  dirName: string
  exists: boolean
}

export interface SymlinkResult {
  path: string
  target: string
  status: 'created' | 'skipped' | 'failed'
  reason?: string
}

// Dialog API for file/directory selection
export interface DialogApi {
  selectDirectory: () => Promise<IpcResult<string>>
  selectFile: (options?: {
    filters?: Array<{ name: string; extensions: string[] }>
    title?: string
  }) => Promise<IpcResult<string>>
}

// Shell detection types
export interface ShellInfo {
  path: string
  name: string
  displayName: string
}

export interface DetectedShells {
  default: ShellInfo | null
  available: ShellInfo[]
}

// Shell API for renderer
export interface ShellApi {
  getAvailableShells: () => Promise<IpcResult<DetectedShells>>
}

// Persistence API for renderer
export interface PersistenceApi {
  read: <T>(key: string) => Promise<IpcResult<T>>
  write: <T>(key: string, data: T) => Promise<IpcResult<void>>
  writeDebounced: <T>(key: string, data: T) => Promise<IpcResult<void>>
  flushPendingWrites: () => Promise<IpcResult<void>>
  delete: (key: string) => Promise<IpcResult<void>>
}

// System API for renderer
export interface SystemApi {
  getHomeDirectory: () => Promise<IpcResult<string>>
  onPowerResume: (callback: () => void) => () => void
}

// Keyboard shortcut callback for main -> renderer communication
export type KeyboardShortcutCallback = (
  shortcut:
    | 'nextTerminal'
    | 'prevTerminal'
    | 'zoomIn'
    | 'zoomOut'
    | 'zoomReset'
    | 'sidebarToggle'
    | 'colorThemePicker'
) => void

// Keyboard API for renderer
export interface KeyboardApi {
  onShortcut: (callback: KeyboardShortcutCallback) => () => void
}

// Window maximize state callback for main -> renderer communication
export type WindowMaximizeChangedCallback = (isMaximized: boolean) => void

// App close coordination types
export type AppCloseResponse = 'close' | 'cancel'
export type AppCloseRequestedCallback = () => Promise<boolean>

// Window API for renderer
export interface WindowApi {
  minimize: () => void
  toggleMaximize: () => Promise<IpcResult<boolean>>
  close: () => void
  onMaximizeChange: (callback: WindowMaximizeChangedCallback) => () => void
  onCloseRequested: (callback: AppCloseRequestedCallback) => () => void
  respondToClose: (response: AppCloseResponse) => void
}

// Clipboard API for renderer
export interface ClipboardApi {
  readText: () => Promise<IpcResult<string>>
  writeText: (text: string) => Promise<IpcResult<void>>
  hasImage: () => Promise<IpcResult<boolean>>
}

// Visibility API for renderer to notify main process of visibility changes
export interface VisibilityApi {
  setVisibilityState: (isVisible: boolean) => Promise<IpcResult<void>>
}

/** Network bind scope for the embedded remote terminal server. */
export type RemoteBindMode = 'localhost' | 'all'

export type TunnelProviderKind = 'cloudflareQuick' | 'cloudflareNamed' | 'frp' | 'sshReverse'

export type RemotePublishMode = 'lan' | 'tunnel'

export interface TunnelConfigView {
  provider: TunnelProviderKind
  cloudflareNamedHostname: string | null
  cloudflareNamedLocalPort: number | null
  cloudflareNamedTokenSet: boolean
  frpServerAddr: string | null
  frpServerPort: number | null
  frpCustomDomain: string | null
  frpRemotePort: number | null
  frpPublicHttps: boolean
  frpTokenSet: boolean
  sshHost: string | null
  sshPort: number | null
  sshUser: string | null
  sshRemotePort: number | null
  sshPublicHostname: string | null
  sshPublicHttps: boolean
  sshPrivateKeySet: boolean
}

export interface TunnelConfigUpdate {
  provider: TunnelProviderKind
  cloudflareNamedHostname?: string | null
  cloudflareNamedLocalPort?: number | null
  /** Omit to leave unchanged; empty string clears the stored secret. */
  cloudflareNamedToken?: string | null
  frpServerAddr?: string | null
  frpServerPort?: number | null
  frpCustomDomain?: string | null
  frpRemotePort?: number | null
  frpPublicHttps?: boolean
  /** Omit to leave unchanged; empty string clears the stored secret. */
  frpToken?: string | null
  sshHost?: string | null
  sshPort?: number | null
  sshUser?: string | null
  sshRemotePort?: number | null
  sshPublicHostname?: string | null
  sshPublicHttps?: boolean
  sshPrivateKey?: string | null
}

export interface RemoteAccessIntent {
  wanted: boolean
  publishMode: RemotePublishMode
}

export interface TunnelConfigApi {
  get: () => Promise<IpcResult<TunnelConfigView>>
  set: (update: TunnelConfigUpdate) => Promise<IpcResult<TunnelConfigView>>
}

// Remote terminal server status (mirrors Rust remote::RemoteStatus)
export interface RemoteStatus {
  running: boolean
  url: string | null
  port: number | null
  /** `localhost` or `all` while running. */
  bindMode: RemoteBindMode | null
  /** `127.0.0.1` or `0.0.0.0` while running. */
  bindHost: string | null
  /** Public tunnel origin without a separately displayed credential. */
  tunnelUrl: string | null
  /** Active provider id while a tunnel is attached. */
  tunnelProvider?: string | null
  /** Credentialed scan/copy URL for the active publish mode. */
  accessUrl?: string | null
  /** Same-Wi-Fi origin without the bearer fragment. */
  lanUrl?: string | null
  /** Credentialed LAN URL. */
  lanAccessUrl?: string | null
  /** Credentialed tunnel URL. */
  tunnelAccessUrl?: string | null
  /** Which URL `accessUrl` currently represents. */
  publishMode?: RemotePublishMode | null
}

// Remote terminal server control API
export interface RemoteServerApi {
  start: (options?: { bindMode?: RemoteBindMode }) => Promise<IpcResult<RemoteStatus>>
  stop: () => Promise<IpcResult<RemoteStatus>>
  status: () => Promise<IpcResult<RemoteStatus>>
  intent: () => Promise<IpcResult<RemoteAccessIntent>>
  setIntent: (update: Partial<RemoteAccessIntent>) => Promise<IpcResult<RemoteAccessIntent>>
  rotateCredential: () => Promise<IpcResult<RemoteStatus>>
}

// Filesystem types re-exported for convenience
import type {
  DirectoryEntry,
  FileChangeEvent,
  FileContent,
  FileInfo,
  FileSearchResponse,
  SearchFileHit
} from './filesystem.types'

export type FileChangeCallback = (event: FileChangeEvent) => void

// Filesystem API for renderer
export type SearchStreamErrorCode =
  | 'QUERY_TOO_LONG'
  | 'PATH_VALIDATION_FAILED'
  | 'RG_SPAWN_FAILED'
  | 'RG_STDOUT_CAPTURE_FAILED'
  | 'RG_STREAM_FAILED'
  | (string & {})

export interface FilesystemApi {
  readDirectory: (dirPath: string) => Promise<IpcResult<DirectoryEntry[]>>
  readFile: (filePath: string) => Promise<IpcResult<FileContent>>
  getFileInfo: (filePath: string) => Promise<IpcResult<FileInfo>>
  searchContent: (
    scopeRoot: string,
    rootPath: string,
    query: string
  ) => Promise<IpcResult<FileSearchResponse>>
  searchContentStreamStart: (
    searchId: string,
    scopeRoot: string,
    rootPath: string,
    query: string
  ) => Promise<IpcResult<void>>
  searchContentStreamCancel: (searchId: string) => Promise<IpcResult<void>>
  onSearchContentBatch: (
    callback: (event: {
      searchId: string
      results: FileSearchResponse['results']
      truncated: boolean
    }) => void
  ) => () => void
  onSearchContentDone: (
    callback: (event: {
      searchId: string
      truncated: boolean
      scannedFiles: number
      failedFiles: number
      /** See `SearchStreamErrorCode` for possible values. */
      code?: SearchStreamErrorCode
      error?: string
    }) => void
  ) => () => void
  searchFileNamesStreamStart: (
    searchId: string,
    scopeRoot: string,
    rootPath: string,
    query: string,
    /** When true, surface ignored/hidden files with `ignored: true` (ADR 0003). */
    includeIgnored?: boolean
  ) => Promise<IpcResult<void>>
  searchFileNamesStreamCancel: (searchId: string) => Promise<IpcResult<void>>
  onSearchFileNamesBatch: (
    callback: (event: { searchId: string; files: SearchFileHit[]; truncated?: boolean }) => void
  ) => () => void
  onSearchFileNamesDone: (
    callback: (event: {
      searchId: string
      truncated: boolean
      totalFiles: number
      /**
       * Programmatic error code. One of:
       * - `QUERY_TOO_LONG`         — query exceeded `MAX_SEARCH_QUERY_LEN`.
       * - `PATH_VALIDATION_FAILED` — scope/root failed `validate_search_path`.
       * - `RG_SPAWN_FAILED`        — ripgrep binary failed to start.
       * - `RG_STDOUT_CAPTURE_FAILED` — stdout pipe could not be captured.
       * - `RG_STREAM_FAILED`       — stdout read failed, or rg exited with a
       *   non-zero status other than `1` (no matches).
       */
      code?: SearchStreamErrorCode
      error?: string
    }) => void
  ) => () => void
  writeFile: (filePath: string, content: string) => Promise<IpcResult<void>>
  createFile: (filePath: string, content?: string) => Promise<IpcResult<void>>
  createDirectory: (dirPath: string) => Promise<IpcResult<void>>
  deletePath: (path: string, options?: { recursive?: boolean }) => Promise<IpcResult<void>>
  renameFile: (oldPath: string, newPath: string) => Promise<IpcResult<void>>
  copyFile: (srcPath: string, destPath: string) => Promise<IpcResult<void>>
  watchDirectory: (dirPath: string) => Promise<IpcResult<void>>
  unwatchDirectory: (dirPath: string) => Promise<IpcResult<void>>
  onFileChanged: (callback: FileChangeCallback) => () => void
  onFileCreated: (callback: FileChangeCallback) => () => void
  onFileDeleted: (callback: FileChangeCallback) => () => void
}

export type {
  DirectoryEntry,
  FileChangeEvent,
  FileContent,
  FileInfo,
  FileSearchResponse,
  SearchFileHit
}

// ============================================================================
// Session Persistence Types
// ============================================================================

/**
 * Captured DEC private-mode state for terminal rehydration across refresh (R3).
 *
 * Mirrors Orca's `buildRehydrateSequences` mode set. Only the modes currently ON
 * are replayed (via `buildRehydrateSequences`) before the captured scrollback
 * content, so an alt-screen TUI (vim/tmux/less/htop) restores identically.
 * Modes are optional everywhere — absence degrades to content-only restore
 * (the pre-R3 behavior).
 */
export interface TerminalModes {
  /** Alt-screen on (DEC 1049/1047/47). Replay emits `\x1b[?1049h` (attrs reset first). */
  alternateScreen: boolean
  /** Bracketed-paste on (DEC 2004). Replay emits `\x1b[?2004h`. */
  bracketedPaste: boolean
  /** Application-cursor keys on (DEC 1). Replay emits `\x1b[?1h`. */
  applicationCursor: boolean
  /** Mouse tracking mode: `x10` (1000), `drag` (1002), `any` (1003); null = off. */
  mouseTracking?: 'x10' | 'drag' | 'any' | null
  /** SGR mouse encoding (DEC 1006). Replay emits `\x1b[?1006h`. */
  sgrMouseMode?: boolean
  /** SGR pixel mouse encoding (DEC 1016). Replay emits `\x1b[?1016h`. */
  sgrMousePixelsMode?: boolean
}

/**
 * Terminal session data for persistence
 * Subset of terminal instance with additional state for restoration
 */
export interface TerminalSession {
  id: string
  shell: string
  cwd: string
  history: string[]
  env?: Record<string, string>
  /**
   * Captured DEC private-mode snapshot (R3). Replayed before `history` on
   * restore so an alt-screen TUI screen/modes survive refresh. Optional:
   * absence (old save or capture unavailable) degrades to content-only restore.
   */
  modes?: TerminalModes
}

/**
 * Workspace state for persistence
 * Contains workspace configuration and active terminals
 */
export interface WorkspaceState {
  projectId: string
  activeTerminalId: string | null
  terminals: TerminalSession[]
}

/**
 * Complete session data structure
 * Contains all application state needed to restore session on app launch
 */
export interface SessionData {
  timestamp: string
  terminals: TerminalSession[]
  workspaces: WorkspaceState[]
}

/**
 * Session API for renderer
 * Handles session persistence operations (save, restore, clear, flush)
 */
export interface SessionApi {
  /**
   * Save complete session data
   */
  save: (sessionData: SessionData) => Promise<IpcResult<void>>

  /**
   * Restore session from disk
   */
  restore: () => Promise<IpcResult<SessionData>>

  /**
   * Clear saved session from disk
   */
  clear: () => Promise<IpcResult<void>>

  /**
   * Flush any pending auto-save operations
   */
  flush: () => Promise<IpcResult<void>>

  /**
   * Check if a saved session exists
   */
  hasSession: () => Promise<IpcResult<boolean>>
}

// ============================================================================
// Data Migration Types
// ============================================================================
//
// CANONICAL MIGRATION API CONTRACT
// =================================
// All layers must follow this contract for consistency.
//
// Method names (canonical):
// - getVersion()     - Get current schema version
// - getSchemaInfo()  - Get current and target schema versions
// - getHistory()     - Get migration history records
// - getRegistered()  - Get all registered migrations
// - runMigration()   - Run all pending migrations (singular!)
// - rollback()       - Rollback to a specific version
//
// Rust command names (snake_case):
// - data_migration_get_version
// - data_migration_get_schema_info
// - data_migration_get_history
// - data_migration_get_registered
// - data_migration_run_migrations
// - data_migration_rollback
//
// Error codes:
// - MIGRATION_VERSION_INVALID: Current version is corrupted
// - MIGRATION_HISTORY_CORRUPT: Migration history is corrupted
// - MIGRATION_EXECUTION_FAILED: A migration function failed
// - MIGRATION_ALREADY_RUNNING: Another migration is in progress
// - MIGRATION_NOT_FOUND: Requested migration version not found
// - ROLLBACK_FAILED: Rollback operation failed
// ============================================================================

/**
 * Migration record in history
 */
export interface MigrationRecord {
  version: string
  timestamp: string
  success: boolean
  error?: string
  duration?: number // in milliseconds
}

/**
 * Migration result
 */
export interface MigrationResult {
  version: string
  success: boolean
  error?: string
  duration: number
}

/**
 * Migration run result (can include partial results on failure)
 *
 * Note: The backend returns IpcResult<MigrationResult[]>, but we transform
 * it to MigrationRunResult to preserve partial results on failure.
 */
export type MigrationRunResult =
  | { success: true; data: MigrationResult[]; code?: never; error?: never }
  | {
      success: false
      error: string
      code: string
      partialResults?: MigrationResult[]
    }

/**
 * Schema version info
 */
export interface SchemaVersion {
  current: string
  target: string
}

/**
 * Registered migration info
 */
export interface MigrationInfo {
  version: string
  description: string
}

/**
 * Rollback request payload
 *
 * This type defines the structure for rollback requests.
 * Tauri automatically flattens single-struct parameters when invoking.
 *
 * The Rust side defines:
 * ```rust
 * #[derive(Debug, Clone, Deserialize)]
 * #[serde(rename_all = "camelCase")]
 * pub struct RollbackRequest {
 *     pub version: String,
 * }
 *
 * #[tauri::command]
 * pub async fn data_migration_rollback(request: RollbackRequest, ...) -> Result<IpcResult<()>, String>
 * ```
 *
 * Invoke from TypeScript:
 * ```ts
 * // Tauri flattens single-struct parameters automatically
 * invoke('data_migration_rollback', { version: '1.2.0' })
 * ```
 *
 * Note: For multi-parameter commands, you would wrap in a payload object.
 * Single-struct parameters are flattened for convenience.
 */
export interface RollbackRequest {
  version: string
}

/**
 * @deprecated Use RollbackRequest from the canonical contract instead.
 * This is an alias for backward compatibility.
 */
export type RollbackRequestPayload = RollbackRequest

/**
 * Canonical Migration API Contract
 *
 * This interface defines the contract that all layers (Tauri, Electron)
 * must implement for data migration operations.
 *
 * Implementation notes:
 * - getVersion returns "0.0.0" for fresh installs (no migrations run)
 * - runMigration returns MigrationRunResult (not IpcResult) to preserve partial results
 * - rollback accepts a version string and returns IpcResult<void>
 *
 * All methods use the IpcResult<T> pattern for consistent error handling,
 * except runMigration which uses MigrationRunResult to include partial results.
 */
export interface MigrationApi {
  /**
   * Get current schema version
   *
   * Returns the currently applied schema version.
   * Returns "0.0.0" for fresh installs (no migrations have been run).
   *
   * @returns IpcResult with version string (e.g., "1.2.3")
   */
  getVersion: () => Promise<IpcResult<string>>

  /**
   * Get schema version info (current and target versions)
   *
   * Returns both the current version and the target (latest registered) version.
   * Useful for checking if migrations are pending (current < target).
   *
   * @returns IpcResult with SchemaVersion containing current and target
   */
  getSchemaInfo: () => Promise<IpcResult<SchemaVersion>>

  /**
   * Get migration history
   *
   * Returns an array of all migration records including both successful
   * and failed migrations. Each record contains version, timestamp,
   * success status, optional error message, and duration.
   *
   * @returns IpcResult with array of MigrationRecord
   */
  getHistory: () => Promise<IpcResult<MigrationRecord[]>>

  /**
   * Get all registered migrations
   *
   * Returns info about all available migrations without running them.
   * Useful for displaying available/pending migrations to the user.
   *
   * @returns IpcResult with array of MigrationInfo
   */
  getRegistered: () => Promise<IpcResult<MigrationInfo[]>>

  /**
   * Run all pending migrations
   *
   * Executes all migrations from current version to latest registered version.
   * Returns an array of migration results, one for each migration executed.
   *
   * IMPORTANT: Returns MigrationRunResult (not IpcResult) to preserve
   * partial results when some migrations succeed but others fail.
   *
   * Error codes:
   * - MIGRATION_VERSION_INVALID: Current version is corrupted
   * - MIGRATION_HISTORY_CORRUPT: Migration history is corrupted
   * - MIGRATION_EXECUTION_FAILED: A migration function failed
   * - MIGRATION_ALREADY_RUNNING: Another migration is in progress
   *
   * @returns MigrationRunResult with success status and migration results
   */
  runMigration: () => Promise<MigrationRunResult>

  /**
   * Rollback to a specific version
   *
   * Reverts the database to the specified version by running rollback
   * functions for migrations newer than the target version.
   *
   * Note: Requires migrations to have rollback functions registered.
   *
   * Error codes:
   * - MIGRATION_NOT_FOUND: Target version not found in migrations
   * - ROLLBACK_FAILED: Rollback function failed or not available
   *
   * @param version - Version to rollback to (e.g., "1.2.0")
   * @returns IpcResult<void>
   */
  rollback: (version: string) => Promise<IpcResult<void>>
}
