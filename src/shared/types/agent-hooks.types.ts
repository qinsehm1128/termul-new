/** Host-owned hook contracts. Wire fields are snake_case; times are Unix milliseconds. */

/** Validated by the host: ^[a-z][a-z0-9-]{0,63}$. Builtin IDs remain reserved. */
export type AdapterId = string
export type HookKind =
  | 'session_start'
  | 'session_end'
  | 'prompt_submitted'
  | 'tool_start'
  | 'tool_end'
  | 'needs_approval'
  | 'blocked'
  | 'idle'
  | 'turn_done'
  | 'error'
export type DerivedState = 'busy' | 'idle' | 'needs_approval' | 'blocked' | 'error' | 'ended'
export type AdapterStatus =
  | 'not_installed'
  | 'installed'
  | 'outdated'
  | 'conflict'
  | 'source_missing'
  | 'invalid'
export type RestoreState =
  | 'pending_confirmation'
  | 'confirmed'
  | 'unconfirmed'
  | 'stale'
  | 'interrupted'
  | 'ended'
  | 'snapshot_missing'
export type HookErrorCode =
  | 'VALIDATION_ERROR'
  | 'FORBIDDEN'
  | 'CONFLICT'
  | 'UNSUPPORTED_PLATFORM'
  | 'UNSUPPORTED_AGENT_CHAT'
  | 'IDENTITY_STALE'
  | 'PLAN_EXPIRED'
  | 'INTERNAL_ERROR'
export interface HookErrorDto {
  code: HookErrorCode
  /** Safe summary only: no token, raw payload, full path or argv. */
  message: string
}
export type HookAction = 'install' | 'uninstall'
export type AdapterOrigin = 'builtin' | 'custom'
export type HookIdentitySource = 'hook' | 'process' | 'legacy-unconfirmed'
export interface HookDiagnosticDto {
  code: string
  message: string
}
export interface HookEventCapabilityDto {
  native_event: string
  /** Conditional native events (e.g. Notification) can yield multiple kinds. */
  hook_kinds: HookKind[]
  enabled_by_default: boolean
  available: boolean
  min_schema_version: string | null
  min_delivery_version: string | null
}
export interface HookCapabilitiesDto {
  chat: boolean
  resume: boolean
  events: HookEventCapabilityDto[]
}
export interface AdapterStatusDto {
  id: AdapterId
  display_name: string
  /** Missing/invalid sources have no candidate version. */
  version: string | null
  origin: AdapterOrigin
  state: AdapterStatus
  installed_version: string | null
  install_id: string | null
  capabilities: HookCapabilitiesDto
  diagnostics: HookDiagnosticDto[]
}
export interface HookPreviewRequest {
  adapter_id: AdapterId
  /** Install also means an explicitly requested upgrade. */
  action: HookAction
  /** Native event name -> opt-in; omitted entries use manifest defaults. */
  event_options: Record<string, boolean>
}
export interface HookFileDiffDto {
  path: string
  /** null means absent; contents are exact UTF-8 except redacted credentials. */
  before: string | null
  after: string | null
  mode: string | null
}
export interface HookKeyDiffDto {
  path: string
  key: string[]
  before: string | null
  after: string | null
}
export interface HookPreviewDto {
  adapter_id: AdapterId
  action: HookAction
  preview_id: string
  fingerprint: string
  files: HookFileDiffDto[]
  keys: HookKeyDiffDto[]
  capabilities: HookCapabilitiesDto
  diagnostics: HookDiagnosticDto[]
}
/** Manage-only CAS request. No plugin confirmation flow in this release. */
export interface HookConfirmRequest {
  adapter_id: AdapterId
  preview_id: string
  fingerprint: string
}
export interface HookMutationResult {
  status: AdapterStatusDto
  receipt_revision: number
}
/** Identity only: no hook timestamps, tools, busy state or runtime revision. */
export interface HookIdentityDto {
  adapter_id: AdapterId
  agent_session_id: string
  transcript_path: string | null
  cwd: string | null
  source: HookIdentitySource
  descriptor_id: string | null
  /** Host-generated display snapshot; never a spawn authorization. */
  resume_argv: string[] | null
}
export interface HookStateDto {
  instance_id: string
  terminal_id: string
  generation: number
  adapter_install_id: string
  agent_session_id: string
  project_id: string
  state: DerivedState
  identity: HookIdentityDto
  restore_state: RestoreState
  revision: number
}
export interface HookStateTombstone {
  instance_id: string
  terminal_id: string
  revision: number
}
export interface HookStateSyncRequest {
  instance_id?: string | null
  since_revision?: number | null
}
/** Global revision gaps are legal; advance to current_revision, not the last row. */
export type HookStateSyncResponse =
  | {
      kind: 'snapshot'
      instance_id: string
      current_revision: number
      states: HookStateDto[]
      /** Always empty for a snapshot; replace the previous authorized view. */
      tombstones: HookStateTombstone[]
    }
  | {
      kind: 'delta'
      instance_id: string
      current_revision: number
      states: HookStateDto[]
      tombstones: HookStateTombstone[]
    }
export interface ResumePlanRequest {
  descriptor_id: string
  project_id: string
  expected_generation: number
}
export interface ResumePlanDto {
  plan_id: string
  descriptor_id: string
  expected_generation: number
  expected_identity: HookIdentityDto
  display_argv: string[]
  expires_at: number
}
export interface ResumeSpawnRequest {
  plan_id: string
  descriptor_id: string
  expected_generation: number
}
export interface ResumeSpawnResult {
  pty_id: string
  generation: number
  attempt_id: string
  pending_confirmation: boolean
  restore_state: RestoreState
}

/** @deprecated Builtin-only discovery; use AdapterStatusDto and AdapterId. */
export const HOOK_AGENT_KINDS = ['claude-code', 'pi', 'qin-code'] as const
/** @deprecated Use AdapterId; custom adapters are valid too. */
export type AgentKind = (typeof HOOK_AGENT_KINDS)[number]
/** @deprecated Legacy installer status; use AdapterStatus. */
export type HookInstallState =
  | 'installed'
  | 'outdated'
  | 'not_installed'
  | 'agent_not_found'
  | 'error'

/** @deprecated Use AdapterStatusDto. */
export interface HookInstallStatus {
  agent: AgentKind
  state: HookInstallState
  config_path: string
  detail: string | null
  cli_path?: string | null
  version?: string | null
}

/** @deprecated Use HookKind (same ten wire values). */
export type AgentHookKind = HookKind

/** @deprecated Raw event transport is replaced by HookStateSyncResponse. */
export interface AgentHookEvent {
  sequence: number
  terminal_id: string
  project_id: string
  agent: AgentKind
  session_id: string
  transcript_path?: string
  kind: AgentHookKind
  at: number
  tool_name?: string
  ok?: boolean
}

/** @deprecated Migrate callers to the revision facade and adapter DTOs. */
export interface AgentHooksApi {
  status(): Promise<HookInstallStatus[]>
  preview(agent: AgentKind): Promise<string>
  install(agent: AgentKind): Promise<HookInstallStatus>
  uninstall(agent: AgentKind): Promise<HookInstallStatus>
  events(after?: number): Promise<AgentHookEvent[]>
}
