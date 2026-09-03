/**
 * Runtime-neutral Conversation identity, lifecycle, and resource-reference contracts.
 *
 * ConversationId is allocated by Se before ACP session creation. ACP session ids remain
 * opaque replaceable bindings and must never be used as Conversation identity or path keys.
 */

export const CONVERSATION_SCHEMA_VERSION = 2 as const
export const PROJECT_ATTACHMENT_SCHEMA_VERSION = 1 as const
export const AGENT_SESSION_BINDING_SCHEMA_VERSION = 1 as const
export const TERMINAL_RESOURCE_REF_SCHEMA_VERSION = 1 as const

declare const conversationIdBrand: unique symbol

/** Canonical lowercase-hyphenated Se-owned UUID. */
export type ConversationId = string & { readonly [conversationIdBrand]?: true }

const canonicalConversationId = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/

/** True only for the canonical path spelling accepted by Rust `ConversationId`. */
export function isConversationId(value: string): value is ConversationId {
  return canonicalConversationId.test(value)
}

/** Parse a canonical Conversation path id without UUID version or variant restrictions. */
export function parseConversationId(value: string): ConversationId {
  if (!isConversationId(value)) {
    throw new Error('conversationId must be a canonical lowercase-hyphenated UUID')
  }
  return value
}

/** UTC date partition derived only from immutable createdAtUtc. */
export interface CreationPartition {
  year: number
  month: number
  day: number
  path: string
}

/** Explicit execution choice; it never changes the independent workspaceCwd. */
export type ExecutionTarget =
  | { kind: 'workspace' }
  | { kind: 'project_root'; projectId: string; projectRoot: string }
  | {
      kind: 'worktree'
      projectId: string
      worktreePath: string
      worktreeBranch: string
    }

/** Optional project attribution/context. Attaching it never mutates workspaceCwd. */
export interface ProjectAttachment {
  schemaVersion: typeof PROJECT_ATTACHMENT_SCHEMA_VERSION
  projectId: string
  attachedAtUtc: string
  projectPathSnapshot: string
  worktreePath: string | null
  worktreeBranch: string | null
}

export const AGENT_SESSION_BINDING_STATES = ['active', 'detached', 'suspended', 'replaced'] as const

export type AgentSessionBindingState = (typeof AGENT_SESSION_BINDING_STATES)[number]

/**
 * Replaceable binding to one external ACP session.
 *
 * agentSessionId is deliberately opaque and may contain non-UUID characters. executionCwd is
 * the resolved explicit target and is independent from the Conversation workspaceCwd.
 */
export interface AgentSessionBinding {
  schemaVersion: typeof AGENT_SESSION_BINDING_SCHEMA_VERSION
  bindingId: string
  agentSessionId: string
  runtimeAgentId: string
  stableAgentNamespace: string
  executionCwd: string
  boundAtUtc: string
  state: AgentSessionBindingState
}

/**
 * Non-owning reference to a PtyManager-owned terminal resource.
 *
 * Raw terminal claims, environment values, credentials, and terminal output are never persisted.
 */
export interface TerminalResourceRef {
  schemaVersion: typeof TERMINAL_RESOURCE_REF_SCHEMA_VERSION
  terminalId: string
  projectId?: string | null
}

export const CONVERSATION_LIFECYCLE_STATES = [
  'allocating_workspace',
  'initializing_agent',
  'ready',
  'agent_failed',
  'recovery_required',
  'deleted'
] as const

export type ConversationLifecycleState = (typeof CONVERSATION_LIFECYCLE_STATES)[number]

export const CONVERSATION_ERROR_CODES = [
  'CONVERSATION_INVALID_ID',
  'CONVERSATION_INVALID_CREATED_AT',
  'CONVERSATION_UNSUPPORTED_SCHEMA',
  'CONVERSATION_NOT_FOUND',
  'CONVERSATION_CORRUPT',
  'CONVERSATION_PATH_ESCAPE',
  'CONVERSATION_SYMLINK_COMPONENT',
  'CONVERSATION_DURABILITY_FAILED',
  'CONVERSATION_CREATE_FAILED',
  'CONVERSATION_BIND_FAILED',
  'CONVERSATION_CONFLICT',
  'CONVERSATION_BINDING_NOT_FOUND',
  'CONVERSATION_BINDING_NOT_ACTIVE',
  'CONVERSATION_BINDING_NOT_DETACHED',
  'CONVERSATION_BINDING_NOT_ADDRESSABLE',
  'CONVERSATION_LIVE_RESOURCES',
  'CONVERSATION_RECOVERY_REQUIRED',
  'CONVERSATION_DURABILITY_UNSUPPORTED',
  'LEGACY_COMPATIBILITY_READ_ONLY',
  'VALIDATION_ERROR'
] as const

export type ConversationErrorCode = (typeof CONVERSATION_ERROR_CODES)[number]

export type ConversationTitleSource =
  | 'background_generated'
  | 'agent_supplied'
  | 'derived_first_message'
  | 'local_alias'

/** Canonical Conversation metadata record. Binding and resource history are stored separately. */
export interface ConversationRecordV2 {
  schemaVersion: typeof CONVERSATION_SCHEMA_VERSION
  conversationId: ConversationId
  createdAtUtc: string
  creationPartition: CreationPartition
  workspaceCwd: string
  executionTarget: ExecutionTarget
  projectAttachment: ProjectAttachment | null
  lifecycleState: ConversationLifecycleState
  lastSeq: number
  createdBy: 'termul'
  title?: string | null
  titleSource?: ConversationTitleSource | null
}

export type ConversationAggregateMutationAction =
  | 'attachProject'
  | 'detachProject'
  | 'updateExecutionTarget'

export interface ConversationIdentitySnapshot {
  conversationId: ConversationId
  createdAtUtc: string
  creationPartition: CreationPartition
  workspaceCwd: string
}

export interface ConversationAttachProjectRequest {
  expectedRevision: number
  attachment: ProjectAttachment
}

export interface ConversationDetachProjectRequest {
  expectedRevision: number
}

export interface ConversationUpdateExecutionTargetRequest {
  expectedRevision: number
  executionTarget: ExecutionTarget
}

export interface ConversationAggregateMutationOutcome {
  status: 'updated'
  action: ConversationAggregateMutationAction
  conversationId: ConversationId
  previousRevision: number
  revision: number
  identityBefore: ConversationIdentitySnapshot
  identityAfter: ConversationIdentitySnapshot
  projectAttachment: ProjectAttachment | null
  executionTarget: ExecutionTarget
  conversation: ConversationRecordV2
}

type RuntimeRecord = Record<string, unknown>

const canonicalUuid = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/
const canonicalUtcMillis = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/

function runtimeRecord(value: unknown, label: string): RuntimeRecord {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new TypeError(`${label} must be an object`)
  }
  return value as RuntimeRecord
}

function exactKeys(
  value: RuntimeRecord,
  required: readonly string[],
  optional: readonly string[] = []
): void {
  const allowed = new Set([...required, ...optional])
  if (Object.keys(value).some((key) => !allowed.has(key))) {
    throw new TypeError('object contains an unknown field')
  }
  if (required.some((key) => !Object.prototype.hasOwnProperty.call(value, key))) {
    throw new TypeError('object is missing a required field')
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

function canonicalTimestamp(value: unknown, label: string): string {
  const timestamp = nonEmptyString(value, label)
  const parsed = new Date(timestamp)
  if (
    !canonicalUtcMillis.test(timestamp) ||
    Number.isNaN(parsed.getTime()) ||
    parsed.toISOString() !== timestamp
  ) {
    throw new TypeError(`${label} must be canonical RFC3339 milliseconds UTC`)
  }
  return timestamp
}

function canonicalRuntimeUuid(value: unknown, label: string): string {
  const uuid = nonEmptyString(value, label)
  if (!canonicalUuid.test(uuid)) throw new TypeError(`${label} must be a canonical UUID`)
  return uuid
}

function parseCreationPartition(value: unknown, createdAtUtc: string): CreationPartition {
  const candidate = runtimeRecord(value, 'creationPartition')
  exactKeys(candidate, ['year', 'month', 'day', 'path'])
  const year = nonNegativeInteger(candidate.year, 'creationPartition.year')
  const month = nonNegativeInteger(candidate.month, 'creationPartition.month')
  const day = nonNegativeInteger(candidate.day, 'creationPartition.day')
  const path = nonEmptyString(candidate.path, 'creationPartition.path')
  const timestamp = new Date(createdAtUtc)
  const expectedPath = `${timestamp.getUTCFullYear().toString().padStart(4, '0')}/${String(
    timestamp.getUTCMonth() + 1
  ).padStart(2, '0')}/${String(timestamp.getUTCDate()).padStart(2, '0')}`
  if (
    year !== timestamp.getUTCFullYear() ||
    month !== timestamp.getUTCMonth() + 1 ||
    day !== timestamp.getUTCDate() ||
    path !== expectedPath
  ) {
    throw new TypeError('creationPartition must match createdAtUtc')
  }
  return value as CreationPartition
}

/** Validate an exact project attachment response without cloning it. */
export function parseProjectAttachment(value: unknown): ProjectAttachment {
  const candidate = runtimeRecord(value, 'projectAttachment')
  exactKeys(candidate, [
    'schemaVersion',
    'projectId',
    'attachedAtUtc',
    'projectPathSnapshot',
    'worktreePath',
    'worktreeBranch'
  ])
  if (candidate.schemaVersion !== PROJECT_ATTACHMENT_SCHEMA_VERSION) {
    throw new TypeError('projectAttachment schemaVersion is unsupported')
  }
  nonEmptyString(candidate.projectId, 'projectAttachment.projectId')
  canonicalTimestamp(candidate.attachedAtUtc, 'projectAttachment.attachedAtUtc')
  nonEmptyString(candidate.projectPathSnapshot, 'projectAttachment.projectPathSnapshot')
  const worktreePath = candidate.worktreePath
  const worktreeBranch = candidate.worktreeBranch
  if (worktreePath !== null) nonEmptyString(worktreePath, 'projectAttachment.worktreePath')
  if (worktreeBranch !== null) nonEmptyString(worktreeBranch, 'projectAttachment.worktreeBranch')
  if ((worktreePath === null) !== (worktreeBranch === null)) {
    throw new TypeError('projectAttachment worktreePath/worktreeBranch must be paired')
  }
  return value as ProjectAttachment
}

/** Validate one exact execution target response without cloning it. */
export function parseExecutionTarget(value: unknown): ExecutionTarget {
  const candidate = runtimeRecord(value, 'executionTarget')
  switch (candidate.kind) {
    case 'workspace':
      exactKeys(candidate, ['kind'])
      break
    case 'project_root':
      exactKeys(candidate, ['kind', 'projectId', 'projectRoot'])
      nonEmptyString(candidate.projectId, 'executionTarget.projectId')
      nonEmptyString(candidate.projectRoot, 'executionTarget.projectRoot')
      break
    case 'worktree':
      exactKeys(candidate, ['kind', 'projectId', 'worktreePath', 'worktreeBranch'])
      nonEmptyString(candidate.projectId, 'executionTarget.projectId')
      nonEmptyString(candidate.worktreePath, 'executionTarget.worktreePath')
      nonEmptyString(candidate.worktreeBranch, 'executionTarget.worktreeBranch')
      break
    default:
      throw new TypeError('executionTarget kind is invalid')
  }
  return value as ExecutionTarget
}

/** Validate one exact opaque ACP binding response without cloning it. */
export function parseAgentSessionBinding(value: unknown): AgentSessionBinding {
  const candidate = runtimeRecord(value, 'agentSessionBinding')
  exactKeys(candidate, [
    'schemaVersion',
    'bindingId',
    'agentSessionId',
    'runtimeAgentId',
    'stableAgentNamespace',
    'executionCwd',
    'boundAtUtc',
    'state'
  ])
  if (candidate.schemaVersion !== AGENT_SESSION_BINDING_SCHEMA_VERSION) {
    throw new TypeError('agentSessionBinding schemaVersion is unsupported')
  }
  canonicalRuntimeUuid(candidate.bindingId, 'agentSessionBinding.bindingId')
  nonEmptyString(candidate.agentSessionId, 'agentSessionBinding.agentSessionId')
  nonEmptyString(candidate.runtimeAgentId, 'agentSessionBinding.runtimeAgentId')
  nonEmptyString(candidate.stableAgentNamespace, 'agentSessionBinding.stableAgentNamespace')
  nonEmptyString(candidate.executionCwd, 'agentSessionBinding.executionCwd')
  canonicalTimestamp(candidate.boundAtUtc, 'agentSessionBinding.boundAtUtc')
  if (!AGENT_SESSION_BINDING_STATES.includes(candidate.state as AgentSessionBindingState)) {
    throw new TypeError('agentSessionBinding state is invalid')
  }
  return value as AgentSessionBinding
}

/** Validate one exact canonical Conversation record without cloning it. */
export function parseConversationRecordV2(value: unknown): ConversationRecordV2 {
  const candidate = runtimeRecord(value, 'conversation')
  exactKeys(
    candidate,
    [
      'schemaVersion',
      'conversationId',
      'createdAtUtc',
      'creationPartition',
      'workspaceCwd',
      'executionTarget',
      'projectAttachment',
      'lifecycleState',
      'lastSeq',
      'createdBy'
    ],
    ['title', 'titleSource']
  )
  if (candidate.title !== undefined && candidate.title !== null) {
    nonEmptyString(candidate.title, 'conversation.title')
  }
  if (candidate.schemaVersion !== CONVERSATION_SCHEMA_VERSION) {
    throw new TypeError('conversation schemaVersion is unsupported')
  }
  if (typeof candidate.conversationId !== 'string') {
    throw new TypeError('conversationId must be a canonical lowercase-hyphenated UUID')
  }
  parseConversationId(candidate.conversationId)
  const createdAtUtc = canonicalTimestamp(candidate.createdAtUtc, 'conversation.createdAtUtc')
  parseCreationPartition(candidate.creationPartition, createdAtUtc)
  nonEmptyString(candidate.workspaceCwd, 'conversation.workspaceCwd')
  parseExecutionTarget(candidate.executionTarget)
  if (candidate.projectAttachment !== null) parseProjectAttachment(candidate.projectAttachment)
  if (
    !CONVERSATION_LIFECYCLE_STATES.includes(candidate.lifecycleState as ConversationLifecycleState)
  ) {
    throw new TypeError('conversation lifecycleState is invalid')
  }
  nonNegativeInteger(candidate.lastSeq, 'conversation.lastSeq')
  if (candidate.createdBy !== 'termul') throw new TypeError('conversation createdBy is invalid')
  return value as ConversationRecordV2
}

/** Validate one exact immutable identity snapshot without cloning it. */
export function parseConversationIdentitySnapshot(value: unknown): ConversationIdentitySnapshot {
  const candidate = runtimeRecord(value, 'conversationIdentity')
  exactKeys(candidate, ['conversationId', 'createdAtUtc', 'creationPartition', 'workspaceCwd'])
  if (typeof candidate.conversationId !== 'string') {
    throw new TypeError('conversationIdentity conversationId is invalid')
  }
  parseConversationId(candidate.conversationId)
  const createdAtUtc = canonicalTimestamp(
    candidate.createdAtUtc,
    'conversationIdentity.createdAtUtc'
  )
  parseCreationPartition(candidate.creationPartition, createdAtUtc)
  nonEmptyString(candidate.workspaceCwd, 'conversationIdentity.workspaceCwd')
  return value as ConversationIdentitySnapshot
}

function sameIdentity(
  left: ConversationIdentitySnapshot,
  right: ConversationIdentitySnapshot
): boolean {
  return (
    left.conversationId === right.conversationId &&
    left.createdAtUtc === right.createdAtUtc &&
    left.creationPartition.year === right.creationPartition.year &&
    left.creationPartition.month === right.creationPartition.month &&
    left.creationPartition.day === right.creationPartition.day &&
    left.creationPartition.path === right.creationPartition.path &&
    left.workspaceCwd === right.workspaceCwd
  )
}

function sameJson(left: unknown, right: unknown): boolean {
  return JSON.stringify(left) === JSON.stringify(right)
}

/** Validate one exact aggregate mutation outcome and its immutable identity invariants. */
export function parseConversationAggregateMutationOutcome(
  value: unknown
): ConversationAggregateMutationOutcome {
  const candidate = runtimeRecord(value, 'conversationAggregateMutationOutcome')
  exactKeys(candidate, [
    'status',
    'action',
    'conversationId',
    'previousRevision',
    'revision',
    'identityBefore',
    'identityAfter',
    'projectAttachment',
    'executionTarget',
    'conversation'
  ])
  if (candidate.status !== 'updated') throw new TypeError('aggregate status is invalid')
  if (
    !['attachProject', 'detachProject', 'updateExecutionTarget'].includes(String(candidate.action))
  ) {
    throw new TypeError('aggregate action is invalid')
  }
  if (typeof candidate.conversationId !== 'string') {
    throw new TypeError('aggregate conversationId is invalid')
  }
  const conversationId = parseConversationId(candidate.conversationId)
  const previousRevision = nonNegativeInteger(
    candidate.previousRevision,
    'aggregate.previousRevision'
  )
  const revision = nonNegativeInteger(candidate.revision, 'aggregate.revision')
  if (revision !== previousRevision + 1) {
    throw new TypeError('aggregate revision must advance exactly once')
  }
  const identityBefore = parseConversationIdentitySnapshot(candidate.identityBefore)
  const identityAfter = parseConversationIdentitySnapshot(candidate.identityAfter)
  if (
    !sameIdentity(identityBefore, identityAfter) ||
    identityAfter.conversationId !== conversationId
  ) {
    throw new TypeError('aggregate immutable identity changed')
  }
  const projectAttachment =
    candidate.projectAttachment === null
      ? null
      : parseProjectAttachment(candidate.projectAttachment)
  const executionTarget = parseExecutionTarget(candidate.executionTarget)
  const conversation = parseConversationRecordV2(candidate.conversation)
  if (
    conversation.conversationId !== conversationId ||
    conversation.lastSeq !== revision ||
    !sameIdentity(identityAfter, conversation) ||
    !sameJson(projectAttachment, conversation.projectAttachment) ||
    !sameJson(executionTarget, conversation.executionTarget)
  ) {
    throw new TypeError('aggregate outcome does not match its Conversation record')
  }
  if (candidate.action === 'attachProject' && projectAttachment === null) {
    throw new TypeError('attachProject requires a project attachment')
  }
  if (candidate.action === 'detachProject' && projectAttachment !== null) {
    throw new TypeError('detachProject must clear the project attachment')
  }
  return value as ConversationAggregateMutationOutcome
}
