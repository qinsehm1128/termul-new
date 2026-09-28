import {
  type AgentSessionBinding,
  type ConversationAggregateMutationOutcome,
  type ConversationId,
  type ConversationRecordV2,
  type ExecutionTarget,
  type ProjectAttachment,
  parseAgentSessionBinding,
  parseConversationId,
  parseConversationRecordV2
} from './conversation.types'
import { parseRecoveryItemV1, type RecoveryItemV1 } from './conversation-recovery.types'
import type { IpcResult } from './ipc.types'
import {
  parseSessionWorkspaceLoadOutcome,
  type SessionWorkspaceLoadOutcome
} from './session-workspace.types'

export type {
  RecoveryAction,
  RecoveryActionResult,
  ResolveRecoveryItemRequest
} from './conversation-recovery.types'

export const LEGACY_CONVERSATION_SOURCE_KINDS = [
  'legacyStorageKey',
  'legacyAgentSessionId',
  'legacyChatHistoryId'
] as const

export type LegacyConversationSourceKind = (typeof LEGACY_CONVERSATION_SOURCE_KINDS)[number]

export interface LegacyConversationKey {
  sourceKind: LegacyConversationSourceKind
  value: string
}

export interface LegacyConversationResolution {
  conversationId: ConversationId
  canonicalRoute: `#/c/${string}`
}

export type ConversationHostKind = 'desktop' | 'standalone'
export type ConversationHostState = 'ready' | 'migrating' | 'hybrid' | 'recovery' | 'error'

export type ConversationMigrationPhase =
  | 'detected'
  | 'quiescing'
  | 'inventoried'
  | 'staging'
  | 'verifying'
  | 'cutoverPending'
  | 'committed'
  | 'observationWindow'
  | 'rollbackPending'
  | 'rolledBack'
  | 'finalized'

export type ConversationReaderPrecedence =
  | 'legacyOnly'
  | 'conversationV2First'
  | 'hybridLegacyFirst'
  | 'conversationV2Only'

export interface ConversationHostStatus {
  hostKind: ConversationHostKind
  state: ConversationHostState
  code: string
  migrationPhase: ConversationMigrationPhase
  readerPrecedence: ConversationReaderPrecedence
  recoveryItemCount: number
  recoveryItems: RecoveryItemV1[]
}

export interface ConversationOpenOutcome {
  conversation: ConversationRecordV2
  workspace: SessionWorkspaceLoadOutcome
}

export interface ConversationBindingSnapshot {
  conversationId: ConversationId
  binding: AgentSessionBinding | null
}

export type ConversationApplicationRequestType =
  | 'conversation_host_status'
  | 'list_conversations'
  | 'get_conversation'
  | 'get_conversation_binding'
  | 'open_conversation'
  | 'resolve_legacy_conversation_id'
  | 'get_session_workspace'
  | 'write_session_workspace'
  | 'resolve_recovery_item'
  | 'attach_project'
  | 'detach_project'
  | 'update_execution_target'

export interface ConversationApi {
  getHostStatus(): Promise<IpcResult<ConversationHostStatus>>
  listConversations(): Promise<IpcResult<ConversationRecordV2[]>>
  getConversation(conversationId: ConversationId): Promise<IpcResult<ConversationRecordV2>>
  getCurrentBinding(conversationId: ConversationId): Promise<IpcResult<ConversationBindingSnapshot>>
  openConversation(conversationId: ConversationId): Promise<IpcResult<ConversationOpenOutcome>>
  renameConversation(
    conversationId: ConversationId,
    title: string
  ): Promise<IpcResult<ConversationRecordV2>>
  resolveLegacyConversationId(
    key: LegacyConversationKey
  ): Promise<IpcResult<LegacyConversationResolution>>
  attachProject(
    conversationId: ConversationId,
    expectedRevision: number,
    attachment: ProjectAttachment
  ): Promise<IpcResult<ConversationAggregateMutationOutcome>>
  detachProject(
    conversationId: ConversationId,
    expectedRevision: number
  ): Promise<IpcResult<ConversationAggregateMutationOutcome>>
  updateExecutionTarget(
    conversationId: ConversationId,
    expectedRevision: number,
    executionTarget: ExecutionTarget
  ): Promise<IpcResult<ConversationAggregateMutationOutcome>>
  subscribeHostStatus(listener: () => void): () => void
}

type RuntimeRecord = Record<string, unknown>

function runtimeRecord(value: unknown, label: string): RuntimeRecord {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new TypeError(`${label} must be an object`)
  }
  return value as RuntimeRecord
}

function exactKeys(value: RuntimeRecord, required: readonly string[]): void {
  const allowed = new Set(required)
  if (
    Object.keys(value).some((key) => !allowed.has(key)) ||
    required.some((key) => !Object.prototype.hasOwnProperty.call(value, key))
  ) {
    throw new TypeError('conversation API payload has missing or unknown fields')
  }
}

function nonEmptyString(value: unknown, label: string): string {
  if (typeof value !== 'string' || value.length === 0) {
    throw new TypeError(`${label} must be a non-empty string`)
  }
  return value
}

function nonNegativeInteger(value: unknown, label: string): number {
  if (!Number.isSafeInteger(value) || Number(value) < 0) {
    throw new TypeError(`${label} must be a non-negative safe integer`)
  }
  return Number(value)
}

/** Validate the exact Conversation host-status payload without cloning it. */
export function parseConversationHostStatus(value: unknown): ConversationHostStatus {
  const candidate = runtimeRecord(value, 'conversationHostStatus')
  exactKeys(candidate, [
    'hostKind',
    'state',
    'code',
    'migrationPhase',
    'readerPrecedence',
    'recoveryItemCount',
    'recoveryItems'
  ])
  if (!['desktop', 'standalone'].includes(String(candidate.hostKind))) {
    throw new TypeError('conversationHostStatus hostKind is invalid')
  }
  if (!['ready', 'migrating', 'hybrid', 'recovery', 'error'].includes(String(candidate.state))) {
    throw new TypeError('conversationHostStatus state is invalid')
  }
  nonEmptyString(candidate.code, 'conversationHostStatus.code')
  if (
    ![
      'detected',
      'quiescing',
      'inventoried',
      'staging',
      'verifying',
      'cutoverPending',
      'committed',
      'observationWindow',
      'rollbackPending',
      'rolledBack',
      'finalized'
    ].includes(String(candidate.migrationPhase))
  ) {
    throw new TypeError('conversationHostStatus migrationPhase is invalid')
  }
  if (
    !['legacyOnly', 'conversationV2First', 'hybridLegacyFirst', 'conversationV2Only'].includes(
      String(candidate.readerPrecedence)
    )
  ) {
    throw new TypeError('conversationHostStatus readerPrecedence is invalid')
  }
  const recoveryItemCount = nonNegativeInteger(
    candidate.recoveryItemCount,
    'conversationHostStatus.recoveryItemCount'
  )
  if (!Array.isArray(candidate.recoveryItems)) {
    throw new TypeError('conversationHostStatus recoveryItems must be an array')
  }
  for (const item of candidate.recoveryItems) parseRecoveryItemV1(item)
  if (candidate.recoveryItems.length !== recoveryItemCount) {
    throw new TypeError('conversationHostStatus recoveryItemCount does not match recoveryItems')
  }
  return value as ConversationHostStatus
}

/** Validate an exact list response without cloning the array or its records. */
export function parseConversationRecordV2Array(value: unknown): ConversationRecordV2[] {
  if (!Array.isArray(value)) throw new TypeError('conversation list must be an array')
  for (const record of value) parseConversationRecordV2(record)
  return value as ConversationRecordV2[]
}

/** Validate the exact Conversation open payload without cloning it. */
export function parseConversationOpenOutcome(value: unknown): ConversationOpenOutcome {
  const candidate = runtimeRecord(value, 'conversationOpenOutcome')
  exactKeys(candidate, ['conversation', 'workspace'])
  const conversation = parseConversationRecordV2(candidate.conversation)
  const workspace = parseSessionWorkspaceLoadOutcome(candidate.workspace)
  if ('conversationId' in workspace && workspace.conversationId !== conversation.conversationId) {
    throw new TypeError('Conversation open workspace belongs to another Conversation')
  }
  if (
    workspace.status === 'loaded' &&
    workspace.workspace.conversationId !== conversation.conversationId
  ) {
    throw new TypeError('Conversation open workspace belongs to another Conversation')
  }
  return value as ConversationOpenOutcome
}

/** Validate the current Conversation ACP binding snapshot without cloning it. */
export function parseConversationBindingSnapshot(value: unknown): ConversationBindingSnapshot {
  const candidate = runtimeRecord(value, 'conversationBindingSnapshot')
  exactKeys(candidate, ['conversationId', 'binding'])
  parseConversationId(typeof candidate.conversationId === 'string' ? candidate.conversationId : '')
  if (candidate.binding !== null) parseAgentSessionBinding(candidate.binding)
  return value as ConversationBindingSnapshot
}

/** Validate an exact legacy resolution and canonical route without cloning it. */
export function parseLegacyConversationResolution(value: unknown): LegacyConversationResolution {
  const candidate = runtimeRecord(value, 'legacyConversationResolution')
  exactKeys(candidate, ['conversationId', 'canonicalRoute'])
  if (typeof candidate.conversationId !== 'string') {
    throw new TypeError('legacy resolution conversationId is invalid')
  }
  const conversationId = parseConversationId(candidate.conversationId)
  if (candidate.canonicalRoute !== `#/c/${conversationId}`) {
    throw new TypeError('legacy resolution canonicalRoute is invalid')
  }
  return value as LegacyConversationResolution
}
