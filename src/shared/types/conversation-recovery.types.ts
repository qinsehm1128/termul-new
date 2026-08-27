import { type ConversationId, parseConversationId } from './conversation.types'

export const RECOVERY_ACTIONS = [
  'inspect',
  'associateConversation',
  'startEmptyWorkspace',
  'dismissPreservedSource'
] as const

export type RecoveryActionName = (typeof RECOVERY_ACTIONS)[number]
export type RecoveryAuthorizationClass = 'read' | 'mutation'
export type RecoveryStatus =
  | 'unresolved'
  | 'resolvedAssociated'
  | 'resolvedStartedEmpty'
  | 'dismissedPreserved'

export interface RecoveryProvenanceV1 {
  readonly sourceKind: string
  readonly relativePath: string
  readonly sha256: string
  readonly preservedReadOnly: true
}

export interface RecoveryItemV1 {
  readonly recoveryId: string
  readonly kind:
    | 'ambiguous_workspace_manifest'
    | 'identifier_collision'
    | 'invalid_created_at'
    | 'corrupt_source'
    | 'conflicting_worktree_provenance'
    | 'conflicting_session_metadata'
  readonly severity: 'warning' | 'blocking'
  readonly sourcePaths: readonly string[]
  readonly conversationIds: readonly ConversationId[]
  readonly sourceSha256: readonly string[]
  readonly candidateFacts: readonly Readonly<Record<string, unknown>>[]
  readonly provenance: readonly RecoveryProvenanceV1[]
  readonly status: RecoveryStatus
  readonly suggestedActions: readonly RecoveryActionName[]
  readonly revision: number
  readonly associationDecisions: readonly ConversationId[]
}

interface RecoveryRequestCommon {
  recoveryId: string
  /** The RecoveryItem revision, never Conversation.lastSeq or workspace revision. */
  expectedRevision: number
}

export interface InspectRecoveryRequest extends RecoveryRequestCommon {
  action: 'inspect'
  payload: Record<string, never>
  idempotencyKey?: string
}

export interface AssociateConversationRecoveryRequest extends RecoveryRequestCommon {
  action: 'associateConversation'
  idempotencyKey: string
  payload: { conversationId: ConversationId }
}

export interface StartEmptyWorkspaceRecoveryRequest extends RecoveryRequestCommon {
  action: 'startEmptyWorkspace'
  idempotencyKey: string
  payload: {
    conversationId: ConversationId
    expectedWorkspaceRevision: number | null
  }
}

export interface DismissPreservedSourceRecoveryRequest extends RecoveryRequestCommon {
  action: 'dismissPreservedSource'
  idempotencyKey: string
  payload: { reasonCode: 'notApplicable' | 'deferLegacyProjection' }
}

export type RecoveryAction =
  | InspectRecoveryRequest
  | AssociateConversationRecoveryRequest
  | StartEmptyWorkspaceRecoveryRequest
  | DismissPreservedSourceRecoveryRequest

export type ResolveRecoveryItemRequest = RecoveryAction

const idempotencyKeyUuidPattern =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/

/** Strict boundary decoder: rejects aliases, snake_case fields, mismatched payloads, and extras. */
export function parseResolveRecoveryItemRequest(value: unknown): ResolveRecoveryItemRequest {
  if (!isRecord(value)) throw new TypeError('recovery request must be an object')
  const common = new Set(['recoveryId', 'expectedRevision', 'idempotencyKey', 'action', 'payload'])
  rejectExtraKeys(value, common)
  if (typeof value.recoveryId !== 'string' || value.recoveryId.length === 0) {
    throw new TypeError('recoveryId is required')
  }
  if (!Number.isSafeInteger(value.expectedRevision) || Number(value.expectedRevision) < 1) {
    throw new TypeError('expectedRevision must be a positive RecoveryItem revision')
  }
  if (!isRecord(value.payload)) throw new TypeError('payload must be an object')
  const idempotencyKey = value.idempotencyKey
  if (idempotencyKey !== undefined && !isIdempotencyKeyUuid(idempotencyKey)) {
    throw new TypeError('idempotencyKey must be a canonical UUID')
  }

  switch (value.action) {
    case 'inspect':
      rejectExtraKeys(value.payload, new Set())
      return value as unknown as InspectRecoveryRequest
    case 'associateConversation':
      requireMutationKey(idempotencyKey)
      rejectExtraKeys(value.payload, new Set(['conversationId']))
      requireConversationId(value.payload.conversationId)
      return value as unknown as AssociateConversationRecoveryRequest
    case 'startEmptyWorkspace':
      requireMutationKey(idempotencyKey)
      rejectExtraKeys(value.payload, new Set(['conversationId', 'expectedWorkspaceRevision']))
      requireConversationId(value.payload.conversationId)
      if (
        value.payload.expectedWorkspaceRevision !== null &&
        (!Number.isSafeInteger(value.payload.expectedWorkspaceRevision) ||
          Number(value.payload.expectedWorkspaceRevision) < 1)
      ) {
        throw new TypeError('expectedWorkspaceRevision must be null or a positive integer')
      }
      return value as unknown as StartEmptyWorkspaceRecoveryRequest
    case 'dismissPreservedSource':
      requireMutationKey(idempotencyKey)
      rejectExtraKeys(value.payload, new Set(['reasonCode']))
      if (!['notApplicable', 'deferLegacyProjection'].includes(String(value.payload.reasonCode))) {
        throw new TypeError('invalid dismiss reasonCode')
      }
      return value as unknown as DismissPreservedSourceRecoveryRequest
    default:
      throw new TypeError('unknown recovery action')
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function rejectExtraKeys(value: Record<string, unknown>, allowed: ReadonlySet<string>): void {
  if (Object.keys(value).some((key) => !allowed.has(key))) {
    throw new TypeError('unknown recovery request field')
  }
}

function isIdempotencyKeyUuid(value: unknown): value is string {
  return typeof value === 'string' && idempotencyKeyUuidPattern.test(value)
}

function requireMutationKey(value: unknown): asserts value is string {
  if (!isIdempotencyKeyUuid(value)) {
    throw new TypeError('mutation action requires a UUID idempotencyKey')
  }
}

function requireConversationId(value: unknown): asserts value is ConversationId {
  if (typeof value !== 'string') {
    throw new TypeError('conversationId must be a canonical lowercase-hyphenated UUID')
  }
  try {
    parseConversationId(value)
  } catch {
    throw new TypeError('conversationId must be a canonical lowercase-hyphenated UUID')
  }
}

export interface RecoveryActionResult {
  recoveryId: string
  action: RecoveryActionName
  authorization: RecoveryAuthorizationClass
  status: RecoveryStatus
  recoveryRevision: number
  workspaceRevision: number | null
  workspaceChanged: boolean
  readonly sourcePaths: readonly string[]
  readonly sourceSha256: readonly string[]
  readonly candidateFacts: readonly Readonly<Record<string, unknown>>[]
  readonly provenance: readonly RecoveryProvenanceV1[]
}

const recoveryIdPattern = /^[0-9a-f]{64}$/
const sha256Pattern = /^[0-9a-f]{64}$/
const forbiddenEvidenceKey =
  /^(?:claim|claims|credential|credentials|token|tokens|env|envVars|environment|terminalIo|terminalOutput)$/i

function exactResponseKeys(
  value: Record<string, unknown>,
  required: readonly string[],
  label: string
): void {
  const allowed = new Set(required)
  if (
    Object.keys(value).some((key) => !allowed.has(key)) ||
    required.some((key) => !Object.prototype.hasOwnProperty.call(value, key))
  ) {
    throw new TypeError(`${label} has missing or unknown fields`)
  }
}

function responseString(value: unknown, label: string): string {
  if (typeof value !== 'string' || value.length === 0) {
    throw new TypeError(`${label} must be a non-empty string`)
  }
  return value
}

function positiveInteger(value: unknown, label: string): number {
  if (!Number.isSafeInteger(value) || Number(value) < 1) {
    throw new TypeError(`${label} must be a positive safe integer`)
  }
  return Number(value)
}

function parseRecoveryId(value: unknown): string {
  const recoveryId = responseString(value, 'recoveryId')
  if (!recoveryIdPattern.test(recoveryId))
    throw new TypeError('recoveryId must be lowercase SHA-256')
  return recoveryId
}

function parseStringList(value: unknown, label: string): readonly string[] {
  if (!Array.isArray(value)) throw new TypeError(`${label} must be an array`)
  for (const item of value) responseString(item, `${label} item`)
  return value as string[]
}

function parseSha256List(value: unknown): readonly string[] {
  const values = parseStringList(value, 'sourceSha256')
  if (values.some((item) => !sha256Pattern.test(item))) {
    throw new TypeError('sourceSha256 contains an invalid digest')
  }
  return values
}

function validateEvidenceValue(value: unknown, depth = 0): void {
  if (depth > 32) throw new TypeError('recovery evidence nesting is too deep')
  if (value === null || typeof value === 'string' || typeof value === 'boolean') return
  if (typeof value === 'number' && Number.isFinite(value)) return
  if (Array.isArray(value)) {
    if (value.length > 4096) throw new TypeError('recovery evidence array is too large')
    for (const item of value) validateEvidenceValue(item, depth + 1)
    return
  }
  if (!isRecord(value)) throw new TypeError('recovery evidence must be JSON data')
  const keys = Object.keys(value)
  if (keys.length > 256) throw new TypeError('recovery evidence object is too large')
  for (const [key, item] of Object.entries(value)) {
    if (forbiddenEvidenceKey.test(key)) {
      throw new TypeError('recovery evidence contains a forbidden sensitive field')
    }
    validateEvidenceValue(item, depth + 1)
  }
}

function parseCandidateFacts(value: unknown): readonly Readonly<Record<string, unknown>>[] {
  if (!Array.isArray(value)) throw new TypeError('candidateFacts must be an array')
  for (const fact of value) {
    if (!isRecord(fact)) throw new TypeError('candidateFacts entries must be objects')
    validateEvidenceValue(fact)
  }
  return value as Readonly<Record<string, unknown>>[]
}

function parseRecoveryProvenance(value: unknown): RecoveryProvenanceV1 {
  if (!isRecord(value)) throw new TypeError('recovery provenance must be an object')
  exactResponseKeys(
    value,
    ['sourceKind', 'relativePath', 'sha256', 'preservedReadOnly'],
    'recovery provenance'
  )
  responseString(value.sourceKind, 'recovery provenance sourceKind')
  responseString(value.relativePath, 'recovery provenance relativePath')
  if (typeof value.sha256 !== 'string' || !sha256Pattern.test(value.sha256)) {
    throw new TypeError('recovery provenance sha256 is invalid')
  }
  if (value.preservedReadOnly !== true) {
    throw new TypeError('recovery provenance must remain read-only')
  }
  return value as unknown as RecoveryProvenanceV1
}

function parseRecoveryProvenanceList(value: unknown): readonly RecoveryProvenanceV1[] {
  if (!Array.isArray(value)) throw new TypeError('provenance must be an array')
  for (const item of value) parseRecoveryProvenance(item)
  return value as RecoveryProvenanceV1[]
}

/** Validate one exact RecoveryItem response, including redacted empty evidence arrays. */
export function parseRecoveryItemV1(value: unknown): RecoveryItemV1 {
  if (!isRecord(value)) throw new TypeError('recovery item must be an object')
  exactResponseKeys(
    value,
    [
      'recoveryId',
      'kind',
      'severity',
      'sourcePaths',
      'conversationIds',
      'sourceSha256',
      'candidateFacts',
      'provenance',
      'status',
      'suggestedActions',
      'revision',
      'associationDecisions'
    ],
    'recovery item'
  )
  parseRecoveryId(value.recoveryId)
  if (
    ![
      'ambiguous_workspace_manifest',
      'identifier_collision',
      'invalid_created_at',
      'corrupt_source',
      'conflicting_worktree_provenance',
      'conflicting_session_metadata'
    ].includes(String(value.kind))
  ) {
    throw new TypeError('recovery item kind is invalid')
  }
  if (!['warning', 'blocking'].includes(String(value.severity))) {
    throw new TypeError('recovery item severity is invalid')
  }
  parseStringList(value.sourcePaths, 'sourcePaths')
  if (!Array.isArray(value.conversationIds)) {
    throw new TypeError('conversationIds must be an array')
  }
  for (const conversationId of value.conversationIds) requireConversationId(conversationId)
  parseSha256List(value.sourceSha256)
  parseCandidateFacts(value.candidateFacts)
  parseRecoveryProvenanceList(value.provenance)
  if (
    !['unresolved', 'resolvedAssociated', 'resolvedStartedEmpty', 'dismissedPreserved'].includes(
      String(value.status)
    )
  ) {
    throw new TypeError('recovery item status is invalid')
  }
  if (!Array.isArray(value.suggestedActions)) {
    throw new TypeError('suggestedActions must be an array')
  }
  for (const action of value.suggestedActions) {
    if (!RECOVERY_ACTIONS.includes(action as RecoveryActionName)) {
      throw new TypeError('suggestedActions contains an invalid action')
    }
  }
  positiveInteger(value.revision, 'recovery item revision')
  if (!Array.isArray(value.associationDecisions)) {
    throw new TypeError('associationDecisions must be an array')
  }
  for (const conversationId of value.associationDecisions) requireConversationId(conversationId)
  return value as unknown as RecoveryItemV1
}

/** Validate one exact recovery action result without cloning immutable evidence. */
export function parseRecoveryActionResult(value: unknown): RecoveryActionResult {
  if (!isRecord(value)) throw new TypeError('recovery result must be an object')
  exactResponseKeys(
    value,
    [
      'recoveryId',
      'action',
      'authorization',
      'status',
      'recoveryRevision',
      'workspaceRevision',
      'workspaceChanged',
      'sourcePaths',
      'sourceSha256',
      'candidateFacts',
      'provenance'
    ],
    'recovery result'
  )
  parseRecoveryId(value.recoveryId)
  if (!RECOVERY_ACTIONS.includes(value.action as RecoveryActionName)) {
    throw new TypeError('recovery result action is invalid')
  }
  const action = value.action as RecoveryActionName
  const expectedAuthorization: RecoveryAuthorizationClass =
    action === 'inspect' ? 'read' : 'mutation'
  if (value.authorization !== expectedAuthorization) {
    throw new TypeError('recovery result authorization does not match action')
  }
  const expectedStatus: Record<RecoveryActionName, RecoveryStatus> = {
    inspect: 'unresolved',
    associateConversation: 'resolvedAssociated',
    startEmptyWorkspace: 'resolvedStartedEmpty',
    dismissPreservedSource: 'dismissedPreserved'
  }
  if (value.status !== expectedStatus[action]) {
    throw new TypeError('recovery result status does not match action')
  }
  positiveInteger(value.recoveryRevision, 'recoveryRevision')
  if (value.workspaceRevision !== null) {
    positiveInteger(value.workspaceRevision, 'workspaceRevision')
  }
  if (typeof value.workspaceChanged !== 'boolean') {
    throw new TypeError('workspaceChanged must be a boolean')
  }
  if (action === 'startEmptyWorkspace') {
    if (!value.workspaceChanged || value.workspaceRevision === null) {
      throw new TypeError('startEmptyWorkspace must report the new workspace revision')
    }
  } else if (value.workspaceChanged || value.workspaceRevision !== null) {
    throw new TypeError('non-workspace recovery actions cannot change the workspace')
  }
  parseStringList(value.sourcePaths, 'sourcePaths')
  parseSha256List(value.sourceSha256)
  parseCandidateFacts(value.candidateFacts)
  parseRecoveryProvenanceList(value.provenance)
  return value as unknown as RecoveryActionResult
}

/** One canonical JSON fixture set consumed by both TypeScript and Rust tests. */
export const RECOVERY_ACTION_FIXTURES_JSON = `[
  {
    "request": {
      "recoveryId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "expectedRevision": 1,
      "action": "inspect",
      "payload": {}
    },
    "authorization": "read",
    "result": {
      "recoveryId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "action": "inspect",
      "authorization": "read",
      "status": "unresolved",
      "recoveryRevision": 1,
      "workspaceRevision": null,
      "workspaceChanged": false,
      "sourcePaths": ["legacy_workspace_manifests/0/project.json"],
      "sourceSha256": ["eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"],
      "candidateFacts": [{ "candidate": "preserved" }],
      "provenance": [{
        "sourceKind": "legacy_workspace_manifests",
        "relativePath": "legacy_workspace_manifests/0/project.json",
        "sha256": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        "preservedReadOnly": true
      }]
    }
  },
  {
    "request": {
      "recoveryId": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      "expectedRevision": 2,
      "idempotencyKey": "21aee10a-56b8-4624-a5e7-586c25dc8d1f",
      "action": "associateConversation",
      "payload": { "conversationId": "018f7a1c-1b4d-7c8a-9f01-0123456789ab" }
    },
    "authorization": "mutation",
    "result": {
      "recoveryId": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      "action": "associateConversation",
      "authorization": "mutation",
      "status": "resolvedAssociated",
      "recoveryRevision": 3,
      "workspaceRevision": null,
      "workspaceChanged": false,
      "sourcePaths": ["legacy_workspace_manifests/0/project.json"],
      "sourceSha256": ["eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"],
      "candidateFacts": [{ "candidate": "preserved" }],
      "provenance": [{
        "sourceKind": "legacy_workspace_manifests",
        "relativePath": "legacy_workspace_manifests/0/project.json",
        "sha256": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        "preservedReadOnly": true
      }]
    }
  },
  {
    "request": {
      "recoveryId": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
      "expectedRevision": 3,
      "idempotencyKey": "d70c2b93-71bc-4df0-85a5-15bd1b7cf452",
      "action": "startEmptyWorkspace",
      "payload": {
        "conversationId": "018f7a1c-1b4d-7c8a-9f01-0123456789ab",
        "expectedWorkspaceRevision": null
      }
    },
    "authorization": "mutation",
    "result": {
      "recoveryId": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
      "action": "startEmptyWorkspace",
      "authorization": "mutation",
      "status": "resolvedStartedEmpty",
      "recoveryRevision": 4,
      "workspaceRevision": 1,
      "workspaceChanged": true,
      "sourcePaths": ["legacy_workspace_manifests/0/project.json"],
      "sourceSha256": ["eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"],
      "candidateFacts": [{ "candidate": "preserved" }],
      "provenance": [{
        "sourceKind": "legacy_workspace_manifests",
        "relativePath": "legacy_workspace_manifests/0/project.json",
        "sha256": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        "preservedReadOnly": true
      }]
    }
  },
  {
    "request": {
      "recoveryId": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
      "expectedRevision": 4,
      "idempotencyKey": "b025313d-df5d-4254-af4f-535b47ea570f",
      "action": "dismissPreservedSource",
      "payload": { "reasonCode": "deferLegacyProjection" }
    },
    "authorization": "mutation",
    "result": {
      "recoveryId": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
      "action": "dismissPreservedSource",
      "authorization": "mutation",
      "status": "dismissedPreserved",
      "recoveryRevision": 5,
      "workspaceRevision": null,
      "workspaceChanged": false,
      "sourcePaths": ["legacy_workspace_manifests/0/project.json"],
      "sourceSha256": ["eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"],
      "candidateFacts": [{ "candidate": "preserved" }],
      "provenance": [{
        "sourceKind": "legacy_workspace_manifests",
        "relativePath": "legacy_workspace_manifests/0/project.json",
        "sha256": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        "preservedReadOnly": true
      }]
    }
  }
]`

export const RECOVERY_ACTION_FIXTURES = JSON.parse(RECOVERY_ACTION_FIXTURES_JSON) as readonly {
  request: RecoveryAction
  authorization: RecoveryAuthorizationClass
  result: RecoveryActionResult
}[]
