import { type ConversationId, parseConversationId } from './conversation.types'
import {
  parseRecoveryActionResult,
  parseRecoveryItemV1,
  type RecoveryActionResult,
  type RecoveryItemV1,
  type ResolveRecoveryItemRequest
} from './conversation-recovery.types'
import type { IpcResult } from './ipc.types'

export const SESSION_WORKSPACE_SCHEMA_VERSION = 1 as const

export type SessionWorkspacePaneDirection = 'horizontal' | 'vertical'

export interface SessionWorkspaceSplitNode {
  type: 'split'
  id: string
  direction: SessionWorkspacePaneDirection
  children: SessionWorkspacePaneNode[]
  sizes: number[]
}

export interface SessionWorkspaceLeafNode {
  type: 'leaf'
  id: string
  terminalIds: string[]
  editorIds: string[]
  activeTabId?: string | null
}

export type SessionWorkspacePaneNode = SessionWorkspaceSplitNode | SessionWorkspaceLeafNode

/** Passive reference to a PtyManager-owned terminal. It conveys no ownership or credential. */
export interface TerminalResourceDescriptor {
  kind: 'terminal'
  /** PtyManager-owned live resource id. */
  terminalId: string
  /** Optional renderer record id used to rebuild visible topology. */
  terminalRecordId?: string
  conversationId: ConversationId
}

/** Renderer-only result of reconciling a passive terminal reference with its host. */
export type TerminalResourceHydrationStatus = 'running' | 'disconnected'

/** Passive editor reference. Unsaved contents, cursor state, and viewport state are excluded. */
export interface EditorResourceDescriptor {
  kind: 'editor'
  editorId: string
  filePath: string
}

export type SessionWorkspaceResourceDescriptor =
  | TerminalResourceDescriptor
  | EditorResourceDescriptor

export type SessionWorkspaceProjectionState =
  | { status: 'native' }
  | {
      status: 'projected'
      sourcePath: string
      sourceSha256: string
      projectedResourceCount: number
      unresolvedResourceCount: number
    }
  | {
      status: 'recoveryRequired'
      recoveryIds: string[]
    }

export interface SessionWorkspaceV1 {
  schemaVersion: typeof SESSION_WORKSPACE_SCHEMA_VERSION
  conversationId: ConversationId
  revision: number
  updatedAtUtc: string
  updateIdentity?: string | null
  topology?: SessionWorkspacePaneNode | null
  activePaneId?: string | null
  resources: SessionWorkspaceResourceDescriptor[]
  projectionState: SessionWorkspaceProjectionState
}

export type SessionWorkspaceLoadOutcome =
  | { status: 'missing'; conversationId: ConversationId }
  | { status: 'loaded'; workspace: SessionWorkspaceV1 }
  | {
      status: 'recoveryRequired'
      conversationId: ConversationId
      recoveryItems: RecoveryItemV1[]
    }

export type SessionWorkspaceWriteOutcome =
  | { status: 'updated'; revision: number; updatedAtUtc: string }
  | {
      status: 'conflict'
      currentRevision: number
      currentUpdatedAtUtc: string
      currentUpdateIdentity?: string | null
    }
  | {
      status: 'recoveryRequired'
      recoveryItems: RecoveryItemV1[]
    }

export interface SessionWorkspaceWriteRequestBody {
  basedRevision: number | null
  workspace: SessionWorkspaceV1
}

export interface SessionWorkspaceApi {
  getWorkspace(conversationId: ConversationId): Promise<IpcResult<SessionWorkspaceLoadOutcome>>
  writeWorkspace(
    conversationId: ConversationId,
    basedRevision: number | null,
    workspace: SessionWorkspaceV1
  ): Promise<IpcResult<SessionWorkspaceWriteOutcome>>
  resolveRecovery(request: ResolveRecoveryItemRequest): Promise<IpcResult<RecoveryActionResult>>
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
    throw new TypeError('SessionWorkspace payload has missing or unknown fields')
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

function positiveInteger(value: unknown, label: string): number {
  const result = nonNegativeInteger(value, label)
  if (result < 1) throw new TypeError(`${label} must be positive`)
  return result
}

function conversationId(value: unknown, label: string): ConversationId {
  if (typeof value !== 'string') throw new TypeError(`${label} is invalid`)
  return parseConversationId(value)
}

function stringList(value: unknown, label: string): string[] {
  if (!Array.isArray(value)) throw new TypeError(`${label} must be an array`)
  for (const item of value) nonEmptyString(item, `${label} item`)
  if (new Set(value).size !== value.length) throw new TypeError(`${label} contains duplicates`)
  return value as string[]
}

interface TopologyBudget {
  nodes: number
  ids: Set<string>
}

function parseTopologyNode(
  value: unknown,
  budget: TopologyBudget,
  depth = 0
): SessionWorkspacePaneNode {
  if (depth > 64 || budget.nodes >= 4096) {
    throw new TypeError('SessionWorkspace topology exceeds its structural bound')
  }
  budget.nodes += 1
  const candidate = runtimeRecord(value, 'SessionWorkspace topology node')
  const id = nonEmptyString(candidate.id, 'SessionWorkspace topology id')
  if (budget.ids.has(id)) throw new TypeError('SessionWorkspace topology node id is duplicated')
  budget.ids.add(id)
  if (candidate.type === 'leaf') {
    exactKeys(candidate, ['type', 'id', 'terminalIds', 'editorIds'], ['activeTabId'])
    stringList(candidate.terminalIds, 'SessionWorkspace terminalIds')
    stringList(candidate.editorIds, 'SessionWorkspace editorIds')
    if (
      Object.prototype.hasOwnProperty.call(candidate, 'activeTabId') &&
      candidate.activeTabId !== null &&
      typeof candidate.activeTabId !== 'string'
    ) {
      throw new TypeError('SessionWorkspace activeTabId must be a string or null')
    }
    return value as SessionWorkspaceLeafNode
  }
  if (candidate.type === 'split') {
    exactKeys(candidate, ['type', 'id', 'direction', 'children', 'sizes'])
    if (!['horizontal', 'vertical'].includes(String(candidate.direction))) {
      throw new TypeError('SessionWorkspace split direction is invalid')
    }
    if (!Array.isArray(candidate.children) || candidate.children.length < 2) {
      throw new TypeError('SessionWorkspace split must contain at least two children')
    }
    if (!Array.isArray(candidate.sizes) || candidate.sizes.length !== candidate.children.length) {
      throw new TypeError('SessionWorkspace split sizes must match children')
    }
    for (const size of candidate.sizes) {
      if (typeof size !== 'number' || !Number.isFinite(size) || size <= 0) {
        throw new TypeError('SessionWorkspace split size is invalid')
      }
    }
    for (const child of candidate.children) parseTopologyNode(child, budget, depth + 1)
    return value as SessionWorkspaceSplitNode
  }
  throw new TypeError('SessionWorkspace topology node type is invalid')
}

function parseProjectionState(value: unknown): SessionWorkspaceProjectionState {
  const candidate = runtimeRecord(value, 'SessionWorkspace projectionState')
  if (candidate.status === 'native') {
    exactKeys(candidate, ['status'])
  } else if (candidate.status === 'projected') {
    exactKeys(candidate, [
      'status',
      'sourcePath',
      'sourceSha256',
      'projectedResourceCount',
      'unresolvedResourceCount'
    ])
    nonEmptyString(candidate.sourcePath, 'SessionWorkspace projection sourcePath')
    if (
      typeof candidate.sourceSha256 !== 'string' ||
      !/^[0-9a-f]{64}$/.test(candidate.sourceSha256)
    ) {
      throw new TypeError('SessionWorkspace projection sourceSha256 is invalid')
    }
    nonNegativeInteger(candidate.projectedResourceCount, 'SessionWorkspace projectedResourceCount')
    nonNegativeInteger(
      candidate.unresolvedResourceCount,
      'SessionWorkspace unresolvedResourceCount'
    )
  } else if (candidate.status === 'recoveryRequired') {
    exactKeys(candidate, ['status', 'recoveryIds'])
    const recoveryIds = stringList(candidate.recoveryIds, 'SessionWorkspace recoveryIds')
    if (recoveryIds.some((id) => !/^[0-9a-f]{64}$/.test(id))) {
      throw new TypeError('SessionWorkspace recoveryIds contains an invalid id')
    }
  } else {
    throw new TypeError('SessionWorkspace projection status is invalid')
  }
  return value as SessionWorkspaceProjectionState
}

function parseResource(
  value: unknown,
  workspaceConversationId: ConversationId
): SessionWorkspaceResourceDescriptor {
  const candidate = runtimeRecord(value, 'SessionWorkspace resource')
  if (candidate.kind === 'terminal') {
    exactKeys(candidate, ['kind', 'terminalId', 'conversationId'], ['terminalRecordId'])
    nonEmptyString(candidate.terminalId, 'SessionWorkspace terminalId')
    const resourceConversationId = conversationId(
      candidate.conversationId,
      'SessionWorkspace terminal conversationId'
    )
    if (resourceConversationId !== workspaceConversationId) {
      throw new TypeError('SessionWorkspace terminal belongs to another Conversation')
    }
    if (
      Object.prototype.hasOwnProperty.call(candidate, 'terminalRecordId') &&
      typeof candidate.terminalRecordId !== 'string'
    ) {
      throw new TypeError('SessionWorkspace terminalRecordId must be a string')
    }
    return value as TerminalResourceDescriptor
  }
  if (candidate.kind === 'editor') {
    exactKeys(candidate, ['kind', 'editorId', 'filePath'])
    nonEmptyString(candidate.editorId, 'SessionWorkspace editorId')
    nonEmptyString(candidate.filePath, 'SessionWorkspace filePath')
    return value as EditorResourceDescriptor
  }
  throw new TypeError('SessionWorkspace resource kind is invalid')
}

/** Validate one exact SessionWorkspace response without cloning its topology or resources. */
export function parseSessionWorkspaceV1(value: unknown): SessionWorkspaceV1 {
  const candidate = runtimeRecord(value, 'SessionWorkspace')
  exactKeys(
    candidate,
    ['schemaVersion', 'conversationId', 'revision', 'updatedAtUtc', 'resources', 'projectionState'],
    ['updateIdentity', 'topology', 'activePaneId']
  )
  if (candidate.schemaVersion !== SESSION_WORKSPACE_SCHEMA_VERSION) {
    throw new TypeError('SessionWorkspace schemaVersion is unsupported')
  }
  const workspaceConversationId = conversationId(
    candidate.conversationId,
    'SessionWorkspace conversationId'
  )
  nonNegativeInteger(candidate.revision, 'SessionWorkspace revision')
  if (typeof candidate.updatedAtUtc !== 'string') {
    throw new TypeError('SessionWorkspace updatedAtUtc must be a string')
  }
  if (
    Object.prototype.hasOwnProperty.call(candidate, 'updateIdentity') &&
    candidate.updateIdentity !== null &&
    typeof candidate.updateIdentity !== 'string'
  ) {
    throw new TypeError('SessionWorkspace updateIdentity must be a string or null')
  }
  if (Object.prototype.hasOwnProperty.call(candidate, 'topology') && candidate.topology !== null) {
    parseTopologyNode(candidate.topology, { nodes: 0, ids: new Set() })
  }
  if (
    Object.prototype.hasOwnProperty.call(candidate, 'activePaneId') &&
    candidate.activePaneId !== null &&
    typeof candidate.activePaneId !== 'string'
  ) {
    throw new TypeError('SessionWorkspace activePaneId must be a string or null')
  }
  if (!Array.isArray(candidate.resources)) {
    throw new TypeError('SessionWorkspace resources must be an array')
  }
  for (const resource of candidate.resources) parseResource(resource, workspaceConversationId)
  parseProjectionState(candidate.projectionState)
  return value as SessionWorkspaceV1
}

/** Validate one exact workspace load outcome. */
export function parseSessionWorkspaceLoadOutcome(value: unknown): SessionWorkspaceLoadOutcome {
  const candidate = runtimeRecord(value, 'SessionWorkspace load outcome')
  if (candidate.status === 'missing') {
    exactKeys(candidate, ['status', 'conversationId'])
    conversationId(candidate.conversationId, 'SessionWorkspace load conversationId')
  } else if (candidate.status === 'loaded') {
    exactKeys(candidate, ['status', 'workspace'])
    parseSessionWorkspaceV1(candidate.workspace)
  } else if (candidate.status === 'recoveryRequired') {
    exactKeys(candidate, ['status', 'conversationId', 'recoveryItems'])
    conversationId(candidate.conversationId, 'SessionWorkspace recovery conversationId')
    if (!Array.isArray(candidate.recoveryItems)) {
      throw new TypeError('SessionWorkspace recoveryItems must be an array')
    }
    for (const item of candidate.recoveryItems) parseRecoveryItemV1(item)
  } else {
    throw new TypeError('SessionWorkspace load status is invalid')
  }
  return value as SessionWorkspaceLoadOutcome
}

/** Validate one exact workspace write outcome, including Conflict/RecoveryRequired success data. */
export function parseSessionWorkspaceWriteOutcome(value: unknown): SessionWorkspaceWriteOutcome {
  const candidate = runtimeRecord(value, 'SessionWorkspace write outcome')
  if (candidate.status === 'updated') {
    exactKeys(candidate, ['status', 'revision', 'updatedAtUtc'])
    positiveInteger(candidate.revision, 'SessionWorkspace updated revision')
    nonEmptyString(candidate.updatedAtUtc, 'SessionWorkspace updatedAtUtc')
  } else if (candidate.status === 'conflict') {
    exactKeys(
      candidate,
      ['status', 'currentRevision', 'currentUpdatedAtUtc'],
      ['currentUpdateIdentity']
    )
    positiveInteger(candidate.currentRevision, 'SessionWorkspace currentRevision')
    nonEmptyString(candidate.currentUpdatedAtUtc, 'SessionWorkspace currentUpdatedAtUtc')
    if (
      Object.prototype.hasOwnProperty.call(candidate, 'currentUpdateIdentity') &&
      candidate.currentUpdateIdentity !== null &&
      typeof candidate.currentUpdateIdentity !== 'string'
    ) {
      throw new TypeError('SessionWorkspace currentUpdateIdentity must be a string or null')
    }
  } else if (candidate.status === 'recoveryRequired') {
    exactKeys(candidate, ['status', 'recoveryItems'])
    if (!Array.isArray(candidate.recoveryItems)) {
      throw new TypeError('SessionWorkspace recoveryItems must be an array')
    }
    for (const item of candidate.recoveryItems) parseRecoveryItemV1(item)
  } else {
    throw new TypeError('SessionWorkspace write status is invalid')
  }
  return value as SessionWorkspaceWriteOutcome
}

/** Compatibility export for the recovery HTTP response parser. */
export const parseSessionWorkspaceRecoveryActionResult = parseRecoveryActionResult
