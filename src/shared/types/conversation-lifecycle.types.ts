import {
  type AgentSessionBinding,
  CONVERSATION_LIFECYCLE_STATES,
  type ConversationId,
  type ConversationLifecycleState,
  type ExecutionTarget,
  type ProjectAttachment,
  parseAgentSessionBinding,
  parseConversationId,
  parseExecutionTarget,
  parseProjectAttachment
} from './conversation.types'

export const CONVERSATION_LIFECYCLE_ACTIONS = [
  'detachBinding',
  'rebindDetachedBinding',
  'suspendBinding',
  'replaceBinding',
  'deleteConversation'
] as const

export type ConversationLifecycleAction = (typeof CONVERSATION_LIFECYCLE_ACTIONS)[number]

export const CONVERSATION_LIFECYCLE_ERROR_CODES = [
  'CONVERSATION_CONFLICT',
  'CONVERSATION_NOT_FOUND',
  'CONVERSATION_BINDING_NOT_FOUND',
  'CONVERSATION_BINDING_NOT_ACTIVE',
  'CONVERSATION_BINDING_NOT_DETACHED',
  'CONVERSATION_BINDING_NOT_ADDRESSABLE',
  'CONVERSATION_LIVE_RESOURCES',
  'CONVERSATION_RECOVERY_REQUIRED',
  'CONVERSATION_DURABILITY_FAILED',
  'ACP_CLOSE_UNSUPPORTED',
  'ACP_CLOSE_FAILED',
  'ACP_REPLACE_FAILED',
  'ACP_COMPENSATION_FAILED',
  'VALIDATION_ERROR',
  'FORBIDDEN',
  'NETWORK_ERROR'
] as const

export type ConversationLifecycleErrorCode = (typeof CONVERSATION_LIFECYCLE_ERROR_CODES)[number]

/** Secret-safe compound detail returned with ACP_COMPENSATION_FAILED. */
export interface AcpCompensationFailure {
  conversationId: ConversationId
  primaryCode: string
  providerCloseCode?: string
  failureRecordCode?: string
  recoveryMarkerCode?: string
  recoveryRecordCode?: string
  recoveryId?: string
}

export interface ConversationLifecycleMutationRequest {
  expectedRevision: number
}

export interface ConversationReplacementRequest {
  schemaVersion: 1
  conversationId: ConversationId
  projectAttachment?: ProjectAttachment | null
  executionTarget: ExecutionTarget
}

export type ConversationDeleteBlocker =
  | {
      kind: 'liveBinding'
      count: number
      ids: string[]
    }
  | {
      kind: 'terminalResources'
      count: number
      ids: string[]
    }

export interface ConversationLifecycleUpdatedOutcome {
  status: 'updated'
  action: ConversationLifecycleAction
  conversationId: ConversationId
  previousRevision: number
  revision: number
  workspaceCwd: string
  lifecycleState: ConversationLifecycleState
  currentBinding: AgentSessionBinding | null
  previousAgentSessionId?: string | null
}

export interface ConversationLifecycleBlockedOutcome {
  status: 'blocked'
  action: 'deleteConversation'
  conversationId: ConversationId
  revision: number
  code: 'CONVERSATION_LIVE_RESOURCES'
  blockers: ConversationDeleteBlocker[]
}

export type ConversationLifecycleOutcome =
  | ConversationLifecycleUpdatedOutcome
  | ConversationLifecycleBlockedOutcome

export interface ConversationLifecycleApi {
  detachBinding(
    conversationId: ConversationId,
    expectedRevision: number
  ): Promise<ConversationLifecycleOutcome>
  rebindDetachedBinding(
    conversationId: ConversationId,
    expectedRevision: number
  ): Promise<ConversationLifecycleOutcome>
  suspendBinding(
    conversationId: ConversationId,
    expectedRevision: number
  ): Promise<ConversationLifecycleOutcome>
  replaceBinding(
    conversationId: ConversationId,
    request: ConversationReplacementRequest,
    expectedRevision: number
  ): Promise<ConversationLifecycleOutcome>
  deleteConversation(
    conversationId: ConversationId,
    expectedRevision: number,
    removeWorkspace?: boolean
  ): Promise<ConversationLifecycleOutcome>
  subscribe(listener: (outcome: ConversationLifecycleOutcome) => void): () => void
}

type RuntimeRecord = Record<string, unknown>

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
  if (
    Object.keys(value).some((key) => !allowed.has(key)) ||
    required.some((key) => !Object.prototype.hasOwnProperty.call(value, key))
  ) {
    throw new TypeError('Conversation lifecycle payload has missing or unknown fields')
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

function parseLifecycleAction(value: unknown): ConversationLifecycleAction {
  if (!CONVERSATION_LIFECYCLE_ACTIONS.includes(value as ConversationLifecycleAction)) {
    throw new TypeError('Conversation lifecycle action is invalid')
  }
  return value as ConversationLifecycleAction
}

/** Validate the exact replacement request before any provider or transport call. */
export function parseConversationReplacementRequest(
  value: unknown
): ConversationReplacementRequest {
  const candidate = runtimeRecord(value, 'Conversation replacement request')
  exactKeys(
    candidate,
    ['schemaVersion', 'conversationId', 'executionTarget'],
    ['projectAttachment']
  )
  if (candidate.schemaVersion !== 1) {
    throw new TypeError('Conversation replacement schemaVersion is unsupported')
  }
  if (typeof candidate.conversationId !== 'string') {
    throw new TypeError('Conversation replacement conversationId is invalid')
  }
  parseConversationId(candidate.conversationId)
  if (Object.prototype.hasOwnProperty.call(candidate, 'projectAttachment')) {
    if (candidate.projectAttachment !== null) parseProjectAttachment(candidate.projectAttachment)
  }
  parseExecutionTarget(candidate.executionTarget)
  return value as ConversationReplacementRequest
}

function parseDeleteBlocker(value: unknown): ConversationDeleteBlocker {
  const candidate = runtimeRecord(value, 'Conversation delete blocker')
  exactKeys(candidate, ['kind', 'count', 'ids'])
  if (!['liveBinding', 'terminalResources'].includes(String(candidate.kind))) {
    throw new TypeError('Conversation delete blocker kind is invalid')
  }
  const count = nonNegativeInteger(candidate.count, 'Conversation delete blocker count')
  if (count < 1) throw new TypeError('Conversation delete blocker count must be positive')
  if (!Array.isArray(candidate.ids) || candidate.ids.length !== count) {
    throw new TypeError('Conversation delete blocker ids must match count')
  }
  for (const id of candidate.ids) nonEmptyString(id, 'Conversation delete blocker id')
  if (new Set(candidate.ids).size !== candidate.ids.length) {
    throw new TypeError('Conversation delete blocker ids must be unique')
  }
  return value as ConversationDeleteBlocker
}

/** Validate one exact updated/blocked lifecycle outcome without cloning it. */
export function parseConversationLifecycleOutcome(value: unknown): ConversationLifecycleOutcome {
  const candidate = runtimeRecord(value, 'Conversation lifecycle outcome')
  if (candidate.status === 'updated') {
    exactKeys(
      candidate,
      [
        'status',
        'action',
        'conversationId',
        'previousRevision',
        'revision',
        'workspaceCwd',
        'lifecycleState',
        'currentBinding'
      ],
      ['previousAgentSessionId']
    )
    parseLifecycleAction(candidate.action)
    if (typeof candidate.conversationId !== 'string') {
      throw new TypeError('Conversation lifecycle conversationId is invalid')
    }
    parseConversationId(candidate.conversationId)
    const previousRevision = nonNegativeInteger(
      candidate.previousRevision,
      'Conversation lifecycle previousRevision'
    )
    const revision = nonNegativeInteger(candidate.revision, 'Conversation lifecycle revision')
    const revisionIsValid =
      candidate.action === 'deleteConversation'
        ? revision === previousRevision || revision === previousRevision + 1
        : revision === previousRevision + 1
    if (!revisionIsValid) {
      throw new TypeError('Conversation lifecycle revision is inconsistent with the action')
    }
    nonEmptyString(candidate.workspaceCwd, 'Conversation lifecycle workspaceCwd')
    if (
      !CONVERSATION_LIFECYCLE_STATES.includes(
        candidate.lifecycleState as ConversationLifecycleState
      )
    ) {
      throw new TypeError('Conversation lifecycle state is invalid')
    }
    if (candidate.currentBinding !== null) parseAgentSessionBinding(candidate.currentBinding)
    if (Object.prototype.hasOwnProperty.call(candidate, 'previousAgentSessionId')) {
      if (
        candidate.previousAgentSessionId !== null &&
        typeof candidate.previousAgentSessionId !== 'string'
      ) {
        throw new TypeError('previousAgentSessionId must be a string or null')
      }
      if (candidate.previousAgentSessionId === '') {
        throw new TypeError('previousAgentSessionId must not be empty')
      }
    }
    if (candidate.action === 'deleteConversation' && candidate.lifecycleState !== 'deleted') {
      throw new TypeError('deleteConversation must return the deleted lifecycle state')
    }
    return value as ConversationLifecycleUpdatedOutcome
  }
  if (candidate.status === 'blocked') {
    exactKeys(candidate, ['status', 'action', 'conversationId', 'revision', 'code', 'blockers'])
    if (candidate.action !== 'deleteConversation') {
      throw new TypeError('only deleteConversation can be blocked')
    }
    if (typeof candidate.conversationId !== 'string') {
      throw new TypeError('Conversation lifecycle conversationId is invalid')
    }
    parseConversationId(candidate.conversationId)
    nonNegativeInteger(candidate.revision, 'Conversation lifecycle revision')
    if (candidate.code !== 'CONVERSATION_LIVE_RESOURCES') {
      throw new TypeError('Conversation lifecycle blocked code is invalid')
    }
    if (!Array.isArray(candidate.blockers) || candidate.blockers.length === 0) {
      throw new TypeError('Conversation lifecycle blockers must be a non-empty array')
    }
    for (const blocker of candidate.blockers) parseDeleteBlocker(blocker)
    return value as ConversationLifecycleBlockedOutcome
  }
  throw new TypeError('Conversation lifecycle outcome status is invalid')
}
