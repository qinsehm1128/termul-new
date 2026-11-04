/** ABI-v1 host capabilities. Secrets are deliberately unavailable; no secret DTO exists. */
import viewIcons from '../plugin-fixtures/view-icons.json'
import type { PluginError } from './plugin.types'

export const PLUGIN_HOST_CAPABILITY = 'plugins.host-capabilities.v1' as const
export const PLUGIN_STORAGE_CONFLICT = 'PLUGIN_STORAGE_CONFLICT' as const
export const PLUGIN_QUOTA_EXCEEDED = 'PLUGIN_QUOTA_EXCEEDED' as const
export const HOST_SERVICE_NAMES = [
  'se.host.context',
  'se.host.events',
  'se.host.storage',
  'se.host.feedback',
  'se.host.status',
  'se.host.ui',
  'se.host.wake',
  'se.host.process'
] as const
export const KNOWN_HOST_SERVICE_NAMES = [...HOST_SERVICE_NAMES, 'se.host.secrets'] as const
export type HostServiceName = (typeof HOST_SERVICE_NAMES)[number]
export function isReservedHostService(name: string): boolean {
  return name.startsWith('se.host.')
}
export function isAvailableHostService(name: string, version: number): boolean {
  return version === 1 && HOST_SERVICE_NAMES.some((service) => service === name)
}
export const HOST_EVENT_TOPICS = [
  'project.activeChanged',
  'terminal.created',
  'terminal.closed',
  'terminal.activeChanged',
  'agent.session.stateChanged'
] as const
export type HostEventTopic = (typeof HOST_EVENT_TOPICS)[number]
export type HostContextScope = 'window' | 'activeProject'
export type HostWhen = Partial<
  Record<
    | 'project.exists'
    | 'terminal.exists'
    | 'agentSession.exists'
    | 'textInput.focused'
    | 'terminal.focused',
    boolean
  >
>
export interface PluginEventSubscription {
  id: string
  topic: HostEventTopic
  scope: HostContextScope
}
export interface PluginMenu {
  id: string
  location: 'terminal.tab' | 'terminal.context' | 'project.context' | 'agentSession.context'
  commandId: string
  order?: number
  when?: HostWhen
}
export interface PluginKeybinding {
  id: string
  commandId: string
  key: string
  mac?: string
  when?: HostWhen
}
export interface PluginStatusItem {
  id: string
  label: string
  location: 'statusBar'
  scope: HostContextScope
  commandId?: string
  order?: number
}
export interface ProjectContext {
  id: string
  name: string
  path: string
}
export interface TerminalContext {
  id: string
  ptyId: string | null
  projectId: string | null
}
export interface AgentSessionContext {
  id: string
  projectId: string
  status: 'initializing' | 'active' | 'error' | 'closed'
  activeTurn: boolean
  openTurnId: string | null
  replaying: boolean
}
export interface HostContextState {
  activeProject: ProjectContext | null
  projectIds: string[]
  activeTerminalId: string | null
  activeSessionId: string | null
  terminals: TerminalContext[]
  agentSessions: AgentSessionContext[]
  focus: { textInput: boolean; terminal: boolean }
  truncated: boolean
}
export interface HostEventDataByTopic {
  'project.activeChanged': { previousProjectId: string | null; projectId: string | null }
  'terminal.created': { terminal: TerminalContext }
  'terminal.closed': { terminal: TerminalContext }
  'terminal.activeChanged': { terminalId: string | null; projectId: string | null }
  'agent.session.stateChanged': { session: AgentSessionContext; removed: boolean }
}
export type HostEventData = HostEventDataByTopic[HostEventTopic]
export type HostSourceEvent = {
  [Topic in HostEventTopic]: {
    topic: Topic
    projectId: string | null
    data: HostEventDataByTopic[Topic]
  }
}[HostEventTopic]
export interface HostContextPublishRequest {
  sourceId: string
  sourceSequence: number
  reset: boolean
  snapshot: HostContextState
  events: HostSourceEvent[]
}
export interface HostContextHeartbeatRequest {
  sourceId: string
}
export interface HostContextAck {
  contextRevision: number
  sourceSequence: number
}
export interface HostContextSnapshotRequest {
  scope: HostContextScope
}
export interface HostContextSnapshot {
  sourceId: string
  contextRevision: number
  sequences: Record<string, number>
  scope: HostContextScope
  context: HostContextState
}
export interface HostEventDeliveryMetadata {
  subscriptionId: string
  generation: string
  sourceId: string
  sequence: number
  contextRevision: number
}
export type HostEventDelivery = HostEventDeliveryMetadata &
  (
    | (HostSourceEvent & { resyncRequired: false })
    | { topic: HostEventTopic; projectId: string | null; data: null; resyncRequired: true }
  )
export interface HostDeliveryReply {
  schemaVersion: 1
  result: { invalidateViews: string[] }
}
export type StorageScope = 'profile' | 'project'
export interface StorageGetRequest {
  requestId: string
  scope: StorageScope
  key: string
  projectId?: string
}
export interface StorageSetRequest extends StorageGetRequest {
  value: unknown
  expectedRevision: number
}
export interface StorageDeleteRequest extends StorageGetRequest {
  expectedRevision: number
}
export interface StorageTicket {
  accepted: true
  ticketId: string
  requestId: string
}
export type StorageCompletionValue =
  | { revision: number }
  | { found: false; revision: number }
  | { found: true; value: unknown; revision: number }
export type HostResult<T> = { ok: true; value: T } | { ok: false; error: PluginError }
export interface StorageCompletion {
  requestId: string
  ticketId: string
  result: HostResult<StorageCompletionValue>
}
export interface NotificationAction {
  label: string
  commandId: string
  args: unknown
}
export interface NotificationUpsertRequest {
  id: string
  severity: 'info' | 'success' | 'warning' | 'error'
  title: string
  message?: string
  actions?: NotificationAction[]
  ttlMs?: number
}
export interface NotificationCloseRequest {
  id: string
}
export interface ProgressUpsertRequest {
  id: string
  title: string
  state: 'running' | 'completed' | 'failed' | 'cancelled'
  fraction?: number
  cancelCommandId?: string
}
export interface StatusSetRequest {
  id: string
  text: string
  icon?: string
  tone?: 'default' | 'muted' | 'success' | 'warning' | 'danger'
  tooltip?: string
  visible?: boolean
  projectId?: string
}
export interface StatusClearRequest {
  id: string
}
export interface HostUiRecordMetadata {
  pluginId: string
  pluginName: string
  generation: string
  revision: number
  createdAt: number
  updatedAt: number
  expiresAt: number | null
}
export interface HostNotificationRecord extends HostUiRecordMetadata, NotificationUpsertRequest {}
export interface HostProgressRecord extends HostUiRecordMetadata, ProgressUpsertRequest {}
export interface HostStatusRecord extends HostUiRecordMetadata, StatusSetRequest {
  label: string
  location: 'statusBar'
  scope: HostContextScope
  order: number
  commandId?: string
}
export interface HostUiSnapshot {
  schemaVersion: 1
  bootId: string
  revision: number
  notifications: HostNotificationRecord[]
  progress: HostProgressRecord[]
  statusItems: HostStatusRecord[]
}
export type HostServiceRequest = { schemaVersion: 1; version: 1 } & (
  | {
      service: 'se.host.context'
      method: 'snapshot'
      input: HostContextSnapshotRequest
    }
  | { service: 'se.host.storage'; method: 'get'; input: StorageGetRequest }
  | { service: 'se.host.storage'; method: 'set'; input: StorageSetRequest }
  | {
      service: 'se.host.storage'
      method: 'delete'
      input: StorageDeleteRequest
    }
  | {
      service: 'se.host.feedback'
      method: 'notification.upsert'
      input: NotificationUpsertRequest
    }
  | {
      service: 'se.host.feedback'
      method: 'notification.close'
      input: NotificationCloseRequest
    }
  | {
      service: 'se.host.feedback'
      method: 'progress.upsert'
      input: ProgressUpsertRequest
    }
  | { service: 'se.host.status'; method: 'set'; input: StatusSetRequest }
  | { service: 'se.host.status'; method: 'clear'; input: StatusClearRequest }
  | { service: 'se.host.ui'; method: 'postMessage'; input: HostUiPostMessageRequest }
  | { service: 'se.host.wake'; method: 'request'; input: HostWakeRequest }
  | {
      service: 'se.host.process'
      method: 'status' | 'start' | 'stop' | 'restart'
      input: HostProcessRequest
    }
  | { service: 'se.host.process'; method: 'logs'; input: HostProcessLogsRequest }
)
export type HostServiceReplyValue = unknown
export interface HostServiceReply {
  schemaVersion: 1
  result: HostResult<HostServiceReplyValue>
}
export type HostDeliveryRequest = { schemaVersion: 1; version: 1 } & (
  | { service: 'se.host.events'; method: 'deliver'; input: HostEventDeliveryInput }
  | { service: 'se.host.storage'; method: 'completed'; input: StorageCompletionInput }
  | { service: 'se.host.wake'; method: 'wake'; input: HostWakeDelivery }
)
export type HostRequest = HostServiceRequest
export type HostReply = HostServiceReply
export type HostEventDeliveryInput = HostEventDelivery
export type StorageCompletionInput = StorageCompletion
export type HostScope = HostContextScope
export type EventSubscriptionContribution = PluginEventSubscription
export type MenuContribution = PluginMenu
export type KeybindingContribution = PluginKeybinding
export type StatusItemContribution = PluginStatusItem
export type ContributionWhen = HostWhen
export interface HostUiDismissRequest {
  pluginId: string
  generation: string
  kind: 'notification' | 'progress'
  id: string
}
export interface HostUiDismissResult {
  dismissed: true
}
type ExecuteContributionBase = {
  pluginId: string
  generation: string
  contributionId: string
  actionIndex?: number
  contextRevision: number
  target: { projectId?: string; terminalId?: string; sessionId?: string }
}

export type ExecuteContributionRequest = ExecuteContributionBase &
  (
    | { kind: 'notification' | 'progress'; recordRevision: number }
    | { kind: 'menu' | 'keybinding' | 'status'; recordRevision?: never }
  )

const own = (o: object, k: string): boolean => Object.prototype.hasOwnProperty.call(o, k)
function bad(): never {
  throw new TypeError('Invalid plugin host contract')
}
function record(v: unknown): Record<string, unknown> {
  if (!v || typeof v !== 'object' || Array.isArray(v)) bad()
  return v as Record<string, unknown>
}
function object(v: unknown, required: string[], optional: string[] = []): Record<string, unknown> {
  const o = record(v)
  if (
    required.some((k) => !own(o, k)) ||
    Object.keys(o).some((k) => !required.includes(k) && !optional.includes(k))
  )
    bad()
  return o
}
function text(v: unknown, min = 0, max = 128): string {
  if (
    typeof v !== 'string' ||
    [...v].length < min ||
    [...v].length > max ||
    /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/u.test(v)
  )
    bad()
  return v
}
function integer(v: unknown, min = 0, max = Number.MAX_SAFE_INTEGER): number {
  if (typeof v !== 'number' || !Number.isSafeInteger(v) || v < min || v > max) bad()
  return v
}
function boolean(v: unknown): boolean {
  if (typeof v !== 'boolean') bad()
  return v
}
function choice<T extends string>(v: unknown, choices: readonly T[]): T {
  if (typeof v !== 'string' || !choices.includes(v as T)) bad()
  return v as T
}
function array(v: unknown, max: number): unknown[] {
  if (!Array.isArray(v) || v.length > max) bad()
  return v
}
function unique(ids: string[]): void {
  if (new Set(ids).size !== ids.length) bad()
}
function id(v: unknown): string {
  return text(v, 1)
}
function local(v: unknown): string {
  const s = id(v)
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(s)) bad()
  return s
}
function pluginId(v: unknown): string {
  const s = id(v)
  if (!/^[a-z][a-z0-9]*(?:[.-][a-z0-9]+)+$/.test(s)) bad()
  return s
}
function uuid(v: unknown): string {
  const s = text(v, 36, 36)
  if (!/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(s)) bad()
  return s
}
function nullableId(v: unknown): string | null {
  return v === null ? null : id(v)
}
/** Reject values JSON.stringify would silently coerce or omit, before measuring UTF-8. */
function bounded(v: unknown, max = 256 * 1024): void {
  const seen = new Set<object>()
  function json(x: unknown): void {
    if (x === null || typeof x === 'boolean') return
    if (typeof x === 'string') {
      text(x, 0, Number.MAX_SAFE_INTEGER)
      return
    }
    if (typeof x === 'number') {
      if (!Number.isFinite(x)) bad()
      return
    }
    if (!x || typeof x !== 'object' || seen.has(x)) bad()
    const proto = Object.getPrototypeOf(x)
    if (
      (!Array.isArray(x) && proto !== Object.prototype && proto !== null) ||
      Object.getOwnPropertySymbols(x).length
    )
      bad()
    seen.add(x)
    if (Array.isArray(x)) {
      for (let i = 0; i < x.length; i++) {
        if (!own(x, String(i))) bad()
        json(x[i])
      }
      if (Object.keys(x).length !== x.length) bad()
    } else
      for (const [key, value] of Object.entries(x)) {
        text(key, 0, Number.MAX_SAFE_INTEGER)
        json(value)
      }
    seen.delete(x)
  }
  try {
    json(v)
    if (new TextEncoder().encode(JSON.stringify(v)).length > max) bad()
  } catch {
    bad()
  }
}
function schema(v: unknown): 1 {
  if (v !== 1) bad()
  return 1
}
function scope(v: unknown): HostContextScope {
  return choice(v, ['window', 'activeProject'])
}
export function parseHostWhen(v: unknown, keybinding = false): HostWhen {
  bounded(v)
  const o = object(
    v,
    [],
    [
      'project.exists',
      'terminal.exists',
      'agentSession.exists',
      'textInput.focused',
      'terminal.focused'
    ]
  )
  const result = Object.fromEntries(
    Object.entries(o).map(([k, value]) => [k, boolean(value)])
  ) as HostWhen
  if (keybinding && (result['textInput.focused'] === true || result['terminal.focused'] === true))
    bad()
  return result
}
export function parsePluginKey(v: unknown): string {
  const s = text(v, 0, 128)
  if (s === '') return s
  const parts = s.split('+'),
    key = parts.pop()!,
    modifiers = ['ctrl', 'cmd', 'shift', 'alt']
  if (
    !parts.length ||
    !parts.some((p) => p === 'ctrl' || p === 'cmd') ||
    parts.some(
      (p, i) =>
        !modifiers.includes(p) || (i > 0 && modifiers.indexOf(parts[i - 1]) >= modifiers.indexOf(p))
    )
  )
    bad()
  if (
    !/^(?:[a-z0-9]|f[1-9]|f1[0-2]|enter|space|tab|escape|backspace|delete|home|end|pageup|pagedown|arrowup|arrowdown|arrowleft|arrowright)$/.test(
      key
    )
  )
    bad()
  return s
}
export function parsePluginEventSubscription(v: unknown): PluginEventSubscription {
  bounded(v)
  const o = object(v, ['id', 'topic', 'scope'])
  const topic = choice(o.topic, HOST_EVENT_TOPICS),
    itemScope = scope(o.scope)
  if (topic === 'project.activeChanged' && itemScope !== 'window') bad()
  return { id: local(o.id), topic, scope: itemScope }
}
export function parsePluginMenu(v: unknown): PluginMenu {
  bounded(v)
  const o = object(v, ['id', 'location', 'commandId'], ['order', 'when'])
  return {
    id: local(o.id),
    location: choice(o.location, [
      'terminal.tab',
      'terminal.context',
      'project.context',
      'agentSession.context'
    ]),
    commandId: local(o.commandId),
    order: own(o, 'order') ? integer(o.order, -100, 100) : 0,
    ...(own(o, 'when') ? { when: parseHostWhen(o.when) } : {})
  }
}
export function parsePluginKeybinding(v: unknown): PluginKeybinding {
  bounded(v)
  const o = object(v, ['id', 'commandId', 'key'], ['mac', 'when'])
  return {
    id: local(o.id),
    commandId: local(o.commandId),
    key: parsePluginKey(o.key),
    ...(own(o, 'mac') ? { mac: parsePluginKey(o.mac) } : {}),
    ...(own(o, 'when') ? { when: parseHostWhen(o.when, true) } : {})
  }
}
export function parsePluginStatusItem(v: unknown): PluginStatusItem {
  bounded(v)
  const o = object(v, ['id', 'label', 'location', 'scope'], ['commandId', 'order'])
  return {
    id: local(o.id),
    label: text(o.label, 1),
    location: choice(o.location, ['statusBar']),
    scope: scope(o.scope),
    order: own(o, 'order') ? integer(o.order, -100, 100) : 0,
    ...(own(o, 'commandId') ? { commandId: local(o.commandId) } : {})
  }
}
export function parseProjectContext(v: unknown): ProjectContext {
  bounded(v)
  const o = object(v, ['id', 'name', 'path'])
  return { id: id(o.id), name: text(o.name, 1), path: text(o.path, 1, 4096) }
}
export function parseTerminalContext(v: unknown): TerminalContext {
  bounded(v)
  const o = object(v, ['id', 'ptyId', 'projectId'])
  return {
    id: id(o.id),
    ptyId: nullableId(o.ptyId),
    projectId: nullableId(o.projectId)
  }
}
export function parseAgentSessionContext(v: unknown): AgentSessionContext {
  bounded(v)
  const o = object(v, ['id', 'projectId', 'status', 'activeTurn', 'openTurnId', 'replaying'])
  return {
    id: id(o.id),
    projectId: id(o.projectId),
    status: choice(o.status, ['initializing', 'active', 'error', 'closed']),
    activeTurn: boolean(o.activeTurn),
    openTurnId: nullableId(o.openTurnId),
    replaying: boolean(o.replaying)
  }
}
export function parseHostContextState(v: unknown): HostContextState {
  bounded(v, 128 * 1024)
  const o = object(v, [
    'activeProject',
    'projectIds',
    'activeTerminalId',
    'activeSessionId',
    'terminals',
    'agentSessions',
    'focus',
    'truncated'
  ])
  const focus = object(o.focus, ['textInput', 'terminal']),
    projectIds = array(o.projectIds, 256).map(id),
    terminals = array(o.terminals, 256).map(parseTerminalContext),
    agentSessions = array(o.agentSessions, 256).map(parseAgentSessionContext)
  unique(projectIds)
  unique(terminals.map((x) => x.id))
  unique(agentSessions.map((x) => x.id))
  return {
    activeProject: o.activeProject === null ? null : parseProjectContext(o.activeProject),
    projectIds,
    activeTerminalId: nullableId(o.activeTerminalId),
    activeSessionId: nullableId(o.activeSessionId),
    terminals,
    agentSessions,
    focus: {
      textInput: boolean(focus.textInput),
      terminal: boolean(focus.terminal)
    },
    truncated: boolean(o.truncated)
  }
}
export function parseHostContextSnapshotRequest(v: unknown): HostContextSnapshotRequest {
  bounded(v)
  const o = object(v, ['scope'])
  return { scope: scope(o.scope) }
}
export function parseHostContextSnapshot(v: unknown): HostContextSnapshot {
  bounded(v)
  const o = object(v, ['sourceId', 'contextRevision', 'sequences', 'scope', 'context'])
  const sequences = record(o.sequences),
    entries = Object.entries(sequences)
  if (entries.length > 16) bad()
  return {
    sourceId: uuid(o.sourceId),
    contextRevision: integer(o.contextRevision),
    sequences: Object.fromEntries(entries.map(([key, value]) => [local(key), integer(value)])),
    scope: scope(o.scope),
    context: parseHostContextState(o.context)
  }
}
function eventData(topic: HostEventTopic, v: unknown): HostEventData {
  switch (topic) {
    case 'project.activeChanged': {
      const o = object(v, ['previousProjectId', 'projectId'])
      return {
        previousProjectId: nullableId(o.previousProjectId),
        projectId: nullableId(o.projectId)
      }
    }
    case 'terminal.created':
    case 'terminal.closed': {
      const o = object(v, ['terminal'])
      return { terminal: parseTerminalContext(o.terminal) }
    }
    case 'terminal.activeChanged': {
      const o = object(v, ['terminalId', 'projectId'])
      return {
        terminalId: nullableId(o.terminalId),
        projectId: nullableId(o.projectId)
      }
    }
    case 'agent.session.stateChanged': {
      const o = object(v, ['session', 'removed'])
      return {
        session: parseAgentSessionContext(o.session),
        removed: boolean(o.removed)
      }
    }
  }
}
export function parseHostSourceEvent(v: unknown): HostSourceEvent {
  bounded(v)
  const o = object(v, ['topic', 'projectId', 'data']),
    topic = choice(o.topic, HOST_EVENT_TOPICS)
  return {
    topic,
    projectId: nullableId(o.projectId),
    data: eventData(topic, o.data)
  } as HostSourceEvent
}
export function parseHostContextPublishRequest(v: unknown): HostContextPublishRequest {
  bounded(v)
  const o = object(v, ['sourceId', 'sourceSequence', 'reset', 'snapshot', 'events'])
  return {
    sourceId: uuid(o.sourceId),
    sourceSequence: integer(o.sourceSequence, 1),
    reset: boolean(o.reset),
    snapshot: parseHostContextState(o.snapshot),
    events: array(o.events, 128).map(parseHostSourceEvent)
  }
}
export function parseHostContextHeartbeatRequest(v: unknown): HostContextHeartbeatRequest {
  bounded(v)
  const o = object(v, ['sourceId'])
  return { sourceId: uuid(o.sourceId) }
}
export function parseHostContextAck(v: unknown): HostContextAck {
  bounded(v)
  const o = object(v, ['contextRevision', 'sourceSequence'])
  return {
    contextRevision: integer(o.contextRevision),
    sourceSequence: integer(o.sourceSequence, 1)
  }
}
export function parseHostEventDelivery(v: unknown): HostEventDelivery {
  bounded(v)
  const o = object(v, [
    'subscriptionId',
    'generation',
    'sourceId',
    'sequence',
    'contextRevision',
    'topic',
    'projectId',
    'data',
    'resyncRequired'
  ])
  const topic = choice(o.topic, HOST_EVENT_TOPICS),
    resyncRequired = boolean(o.resyncRequired)
  if (resyncRequired ? o.data !== null : o.data === null) bad()
  return {
    subscriptionId: local(o.subscriptionId),
    generation: uuid(o.generation),
    sourceId: uuid(o.sourceId),
    sequence: integer(o.sequence, 1),
    contextRevision: integer(o.contextRevision),
    topic,
    projectId: nullableId(o.projectId),
    data: resyncRequired ? null : eventData(topic, o.data),
    resyncRequired
  } as HostEventDelivery
}
export function parseHostDeliveryReply(v: unknown): HostDeliveryReply {
  bounded(v)
  const o = object(v, ['schemaVersion', 'result']),
    result = object(o.result, ['invalidateViews']),
    invalidateViews = array(result.invalidateViews, 8).map(local)
  unique(invalidateViews)
  return {
    schemaVersion: schema(o.schemaVersion),
    result: { invalidateViews }
  }
}
function storageInput(
  v: unknown,
  method: 'get' | 'set' | 'delete'
): StorageGetRequest | StorageSetRequest | StorageDeleteRequest {
  bounded(v)
  const o = object(
    v,
    [
      'requestId',
      'scope',
      'key',
      ...(method === 'get' ? [] : ['expectedRevision']),
      ...(method === 'set' ? ['value'] : [])
    ],
    ['projectId']
  )
  const storageScope = choice(o.scope, ['profile', 'project'])
  if (
    (storageScope === 'profile' && own(o, 'projectId')) ||
    (storageScope === 'project' && !own(o, 'projectId'))
  )
    bad()
  if (method === 'set') bounded(o.value, 64 * 1024)
  return {
    requestId: local(o.requestId),
    scope: storageScope,
    key: local(o.key),
    ...(storageScope === 'project' ? { projectId: id(o.projectId) } : {}),
    ...(method !== 'get' ? { expectedRevision: integer(o.expectedRevision) } : {}),
    ...(method === 'set' ? { value: o.value } : {})
  }
}
export function parseStorageGetRequest(v: unknown): StorageGetRequest {
  return storageInput(v, 'get')
}
export function parseStorageSetRequest(v: unknown): StorageSetRequest {
  return storageInput(v, 'set') as StorageSetRequest
}
export function parseStorageDeleteRequest(v: unknown): StorageDeleteRequest {
  return storageInput(v, 'delete') as StorageDeleteRequest
}
export function parseStorageTicket(v: unknown): StorageTicket {
  bounded(v)
  const o = object(v, ['accepted', 'ticketId', 'requestId'])
  if (o.accepted !== true) bad()
  return {
    accepted: true,
    ticketId: uuid(o.ticketId),
    requestId: local(o.requestId)
  }
}
function hostError(v: unknown): PluginError {
  const o = object(v, ['code', 'message']),
    code = text(o.code, 1, 128)
  if (!/^PLUGIN_[A-Z0-9_]+$/.test(code)) bad()
  return { code, message: text(o.message, 1, 2048) }
}
function result<T>(v: unknown, parse: (value: unknown) => T): HostResult<T> {
  const o = record(v)
  if (o.ok === true) {
    object(v, ['ok', 'value'])
    return { ok: true, value: parse(o.value) }
  }
  if (o.ok === false) {
    object(v, ['ok', 'error'])
    return { ok: false, error: hostError(o.error) }
  }
  return bad()
}
function completionValue(v: unknown): StorageCompletionValue {
  const raw = record(v),
    hasFound = own(raw, 'found')
  const o = object(v, [
    'revision',
    ...(hasFound ? ['found'] : []),
    ...(raw.found === true ? ['value'] : [])
  ])
  const revision = integer(o.revision)
  if (!hasFound) return { revision }
  if (!boolean(o.found)) return { found: false, revision }
  bounded(o.value, 64 * 1024)
  return { found: true, value: o.value, revision }
}
export function parseStorageCompletion(v: unknown): StorageCompletion {
  bounded(v)
  const o = object(v, ['requestId', 'ticketId', 'result'])
  return {
    requestId: local(o.requestId),
    ticketId: uuid(o.ticketId),
    result: result(o.result, completionValue)
  }
}
export function parseNotificationUpsertRequest(v: unknown): NotificationUpsertRequest {
  bounded(v)
  const o = object(v, ['id', 'severity', 'title'], ['message', 'actions', 'ttlMs'])
  return {
    id: local(o.id),
    severity: choice(o.severity, ['info', 'success', 'warning', 'error']),
    title: text(o.title, 1),
    ...(own(o, 'message') ? { message: text(o.message, 0, 1024) } : {}),
    ...(own(o, 'actions')
      ? {
          actions: array(o.actions, 2).map((v) => {
            const a = object(v, ['label', 'commandId', 'args'])
            bounded(a.args, 8 * 1024)
            return {
              label: text(a.label, 1, 64),
              commandId: local(a.commandId),
              args: a.args
            }
          })
        }
      : { actions: [] }),
    ttlMs: own(o, 'ttlMs') ? integer(o.ttlMs, 2000, 30000) : 8000
  }
}
export function parseNotificationCloseRequest(v: unknown): NotificationCloseRequest {
  bounded(v)
  const o = object(v, ['id'])
  return { id: local(o.id) }
}
export const parseStatusClearRequest = parseNotificationCloseRequest
export function parseProgressUpsertRequest(v: unknown): ProgressUpsertRequest {
  bounded(v)
  const o = object(v, ['id', 'title', 'state'], ['fraction', 'cancelCommandId'])
  if (
    own(o, 'fraction') &&
    (typeof o.fraction !== 'number' ||
      !Number.isFinite(o.fraction) ||
      o.fraction < 0 ||
      o.fraction > 1)
  )
    bad()
  return {
    id: local(o.id),
    title: text(o.title, 1),
    state: choice(o.state, ['running', 'completed', 'failed', 'cancelled']),
    ...(own(o, 'fraction') ? { fraction: o.fraction as number } : {}),
    ...(own(o, 'cancelCommandId') ? { cancelCommandId: local(o.cancelCommandId) } : {})
  }
}
export function parseStatusSetRequest(v: unknown): StatusSetRequest {
  bounded(v)
  const o = object(v, ['id', 'text'], ['icon', 'tone', 'tooltip', 'visible', 'projectId'])
  return {
    id: local(o.id),
    text: text(o.text, 1, 32),
    ...(own(o, 'icon') ? { icon: choice(o.icon, viewIcons) } : {}),
    tone: own(o, 'tone')
      ? choice(o.tone, ['default', 'muted', 'success', 'warning', 'danger'] as const)
      : 'default',
    ...(own(o, 'tooltip') ? { tooltip: text(o.tooltip, 0, 256) } : {}),
    visible: own(o, 'visible') ? boolean(o.visible) : true,
    ...(own(o, 'projectId') ? { projectId: id(o.projectId) } : {})
  }
}
export function parseHostUiDismissRequest(v: unknown): HostUiDismissRequest {
  bounded(v)
  const o = object(v, ['pluginId', 'generation', 'kind', 'id'])
  return {
    pluginId: pluginId(o.pluginId),
    generation: uuid(o.generation),
    kind: choice(o.kind, ['notification', 'progress']),
    id: local(o.id)
  }
}
export function parseHostUiDismissResult(v: unknown): HostUiDismissResult {
  bounded(v)
  const o = object(v, ['dismissed'])
  if (o.dismissed !== true) bad()
  return { dismissed: true }
}
export function parseExecuteContributionRequest(v: unknown): ExecuteContributionRequest {
  bounded(v)
  const o = object(
      v,
      ['pluginId', 'generation', 'kind', 'contributionId', 'contextRevision', 'target'],
      ['actionIndex', 'recordRevision']
    ),
    kind = choice(o.kind, ['menu', 'keybinding', 'status', 'notification', 'progress']),
    target = object(o.target, [], ['projectId', 'terminalId', 'sessionId'])
  if (kind === 'notification' ? !own(o, 'actionIndex') : own(o, 'actionIndex')) bad()
  const feedback = kind === 'notification' || kind === 'progress'
  if (feedback ? !own(o, 'recordRevision') : own(o, 'recordRevision')) bad()
  return {
    pluginId: pluginId(o.pluginId),
    generation: uuid(o.generation),
    ...(feedback ? { kind, recordRevision: integer(o.recordRevision) } : { kind }),
    contributionId: local(o.contributionId),
    ...(kind === 'notification' ? { actionIndex: integer(o.actionIndex, 0, 1) } : {}),
    contextRevision: integer(o.contextRevision),
    target: Object.fromEntries(Object.entries(target).map(([k, v]) => [k, id(v)]))
  }
}

const META_KEYS = [
  'pluginId',
  'pluginName',
  'generation',
  'revision',
  'createdAt',
  'updatedAt',
  'expiresAt'
]
function metadata(o: Record<string, unknown>): HostUiRecordMetadata {
  return {
    pluginId: pluginId(o.pluginId),
    pluginName: text(o.pluginName, 1),
    generation: uuid(o.generation),
    revision: integer(o.revision),
    createdAt: integer(o.createdAt),
    updatedAt: integer(o.updatedAt),
    expiresAt: o.expiresAt === null ? null : integer(o.expiresAt)
  }
}
function without(o: Record<string, unknown>, keys: string[]): Record<string, unknown> {
  return Object.fromEntries(Object.entries(o).filter(([k]) => !keys.includes(k)))
}
export function parseHostNotificationRecord(v: unknown): HostNotificationRecord {
  bounded(v)
  const o = object(v, [...META_KEYS, 'id', 'severity', 'title'], ['message', 'actions', 'ttlMs'])
  return {
    ...metadata(o),
    ...parseNotificationUpsertRequest(without(o, META_KEYS))
  }
}
export function parseHostProgressRecord(v: unknown): HostProgressRecord {
  bounded(v)
  const o = object(v, [...META_KEYS, 'id', 'title', 'state'], ['fraction', 'cancelCommandId'])
  return {
    ...metadata(o),
    ...parseProgressUpsertRequest(without(o, META_KEYS))
  }
}
export function parseHostStatusRecord(v: unknown): HostStatusRecord {
  bounded(v)
  const declaration = ['label', 'location', 'scope', 'order', 'commandId']
  const o = object(
    v,
    [...META_KEYS, 'id', 'text', 'label', 'location', 'scope'],
    ['order', 'commandId', 'icon', 'tone', 'tooltip', 'visible', 'projectId']
  )
  const itemScope = scope(o.scope)
  if (itemScope === 'window' ? own(o, 'projectId') : !own(o, 'projectId')) bad()
  return {
    ...metadata(o),
    ...parseStatusSetRequest(without(o, [...META_KEYS, ...declaration])),
    label: text(o.label, 1),
    location: choice(o.location, ['statusBar']),
    scope: itemScope,
    order: own(o, 'order') ? integer(o.order, -100, 100) : 0,
    ...(own(o, 'commandId') ? { commandId: local(o.commandId) } : {})
  }
}
export function parseHostUiSnapshot(v: unknown): HostUiSnapshot {
  bounded(v, 128 * 1024)
  const o = object(v, [
    'schemaVersion',
    'bootId',
    'revision',
    'notifications',
    'progress',
    'statusItems'
  ])
  const notifications = array(o.notifications, 128).map(parseHostNotificationRecord),
    progress = array(o.progress, 128).map(parseHostProgressRecord),
    statusItems = array(o.statusItems, 128).map(parseHostStatusRecord)
  if (notifications.length + progress.length + statusItems.length > 128) bad()
  for (const [entries, cap] of [
    [notifications, 8],
    [progress, 2],
    [statusItems, 2]
  ] as const) {
    unique(entries.map((x) => JSON.stringify([x.pluginId, x.generation, x.id])))
    const counts = new Map<string, number>()
    for (const entry of entries) {
      const key = JSON.stringify([entry.pluginId, entry.generation]),
        count = (counts.get(key) ?? 0) + 1
      if (count > cap) bad()
      counts.set(key, count)
    }
  }
  const snapshot: HostUiSnapshot = {
    schemaVersion: schema(o.schemaVersion),
    bootId: uuid(o.bootId),
    revision: integer(o.revision),
    notifications,
    progress,
    statusItems
  }
  bounded(snapshot, 128 * 1024)
  return snapshot
}
export function parseHostServiceRequest(v: unknown): HostServiceRequest {
  bounded(v)
  const o = object(v, ['schemaVersion', 'service', 'version', 'method', 'input'])
  schema(o.schemaVersion)
  schema(o.version)
  const service = choice(o.service, HOST_SERVICE_NAMES),
    method = text(o.method, 1, 128)
  const parsers: Record<string, (input: unknown) => unknown> = {
    'se.host.context:snapshot': parseHostContextSnapshotRequest,
    'se.host.storage:get': parseStorageGetRequest,
    'se.host.storage:set': parseStorageSetRequest,
    'se.host.storage:delete': parseStorageDeleteRequest,
    'se.host.feedback:notification.upsert': parseNotificationUpsertRequest,
    'se.host.feedback:notification.close': parseNotificationCloseRequest,
    'se.host.feedback:progress.upsert': parseProgressUpsertRequest,
    'se.host.status:set': parseStatusSetRequest,
    'se.host.status:clear': parseStatusClearRequest,
    'se.host.ui:postMessage': parseHostUiPostMessageRequest,
    'se.host.wake:request': parseHostWakeRequest,
    'se.host.process:status': parseHostProcessRequest,
    'se.host.process:start': parseHostProcessRequest,
    'se.host.process:stop': parseHostProcessRequest,
    'se.host.process:restart': parseHostProcessRequest,
    'se.host.process:logs': parseHostProcessLogsRequest
  }
  const parse = parsers[service + ':' + method]
  if (!parse) bad()
  return {
    schemaVersion: 1,
    version: 1,
    service,
    method,
    input: parse(o.input)
  } as HostServiceRequest
}
export function parseHostServiceReply(v: unknown): HostServiceReply {
  bounded(v)
  const o = object(v, ['schemaVersion', 'result'])
  return {
    schemaVersion: schema(o.schemaVersion),
    result: result(o.result, (value) => {
      bounded(value)
      return value
    })
  }
}
export const parseHostRequest = parseHostServiceRequest
export const parseHostReply = parseHostServiceReply
export const parseHostEventDeliveryInput = parseHostEventDelivery
export const parseStorageCompletionInput = parseStorageCompletion

export function parseHostDeliveryRequest(v: unknown): HostDeliveryRequest {
  bounded(v)
  const o = object(v, ['schemaVersion', 'service', 'version', 'method', 'input'])
  schema(o.schemaVersion)
  schema(o.version)
  if (o.service === 'se.host.events' && o.method === 'deliver')
    return {
      schemaVersion: 1,
      version: 1,
      service: o.service,
      method: o.method,
      input: parseHostEventDelivery(o.input)
    }
  if (o.service === 'se.host.wake' && o.method === 'wake')
    return {
      schemaVersion: 1,
      version: 1,
      service: o.service,
      method: o.method,
      input: parseHostWakeDelivery(o.input)
    }
  if (o.service === 'se.host.storage' && o.method === 'completed')
    return {
      schemaVersion: 1,
      version: 1,
      service: o.service,
      method: o.method,
      input: parseStorageCompletion(o.input)
    }
  return bad()
}

export type ContextSnapshotRequest = HostContextSnapshotRequest
export type HostUiGetRequest = Record<string, never>
export interface HostAcceptedResult {
  accepted: true
  id: string
}
export interface NotificationCloseResult {
  closed: true
}
export interface StatusClearResult {
  cleared: true
}
export const PLUGIN_HOST_WAKE_LIMITS = {
  coalesceMs: 50,
  operationsPerSecond: 20,
  burst: 40,
  reasons: 8
} as const
export const PLUGIN_HOST_PROCESS_LIMITS = {
  processes: 4,
  args: 64,
  env: 64,
  logEntries: 500,
  logText: 4096
} as const
export interface PluginManagedProcess {
  id: string
  command: string
  args: string[]
  env: Record<string, string>
  cwd?: string
  autostart: boolean
  restart: 'never' | 'on-failure' | 'always'
}
export interface HostWakeRequest {
  reason?: string
}
export interface HostWakeDelivery {
  generation: string
  count: number
  reasons: string[]
}
export interface HostUiPostMessageRequest {
  viewId: string
  payload: unknown
}
export interface HostUiPostMessageReply {
  delivered: number
}
export interface HostProcessRequest {
  id: string
}
function noNul(v: unknown, min = 0, max = 4096): string {
  const value = text(v, min, max)
  if (value.includes('\u0000')) bad()
  return value
}
function absolutePath(s: string): boolean {
  return (
    s.startsWith('/') ||
    (s.length > 2 && s.charCodeAt(0) === 92 && s.charCodeAt(1) === 92) ||
    /^[A-Za-z]:[/\\\\]/.test(s)
  )
}
function relativeCommand(s: string): boolean {
  return (
    s.length <= 512 &&
    s
      .split('/')
      .every(
        (p) =>
          /^[A-Za-z0-9._-]+$/.test(p) &&
          p !== '.' &&
          p !== '..' &&
          !p.endsWith('.') &&
          p.toLowerCase() !== '__se__' &&
          !/^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i.test(p)
      )
  )
}
export function parsePluginManagedProcess(v: unknown): PluginManagedProcess {
  bounded(v)
  const o = object(v, ['id', 'command'], ['args', 'env', 'cwd', 'autostart', 'restart'])
  const command = noNul(o.command, 1)
  if (!absolutePath(command) && !relativeCommand(command)) bad()
  const args = own(o, 'args')
    ? array(o.args, PLUGIN_HOST_PROCESS_LIMITS.args).map((v) => noNul(v))
    : []
  const rawEnv = own(o, 'env') ? record(o.env) : {}
  if (Object.keys(rawEnv).length > PLUGIN_HOST_PROCESS_LIMITS.env) bad()
  const env = Object.fromEntries(
    Object.entries(rawEnv).map(([key, value]) => {
      if (key.length > 128 || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) bad()
      return [key, noNul(value)]
    })
  )
  const cwd = own(o, 'cwd') ? noNul(o.cwd, 1) : undefined
  if (cwd !== undefined && cwd !== 'package' && cwd !== 'profileData' && !absolutePath(cwd)) bad()
  return {
    id: local(o.id),
    command,
    args,
    env,
    ...(cwd !== undefined ? { cwd } : {}),
    autostart: own(o, 'autostart') ? boolean(o.autostart) : true,
    restart: own(o, 'restart') ? choice(o.restart, ['never', 'on-failure', 'always']) : 'on-failure'
  }
}
export function parseHostWakeRequest(v: unknown): HostWakeRequest {
  bounded(v)
  const o = object(v, [], ['reason'])
  return own(o, 'reason') ? { reason: text(o.reason, 0, 64) } : {}
}
export function parseHostWakeDelivery(v: unknown): HostWakeDelivery {
  bounded(v)
  const o = object(v, ['generation', 'count', 'reasons'])
  const reasons = array(o.reasons, PLUGIN_HOST_WAKE_LIMITS.reasons).map((v) => text(v, 0, 64))
  unique(reasons)
  return { generation: uuid(o.generation), count: integer(o.count, 1), reasons }
}
export function parseHostUiPostMessageRequest(v: unknown): HostUiPostMessageRequest {
  bounded(v)
  const o = object(v, ['viewId', 'payload'])
  bounded(o.payload)
  return { viewId: local(o.viewId), payload: o.payload }
}
export function parseHostUiPostMessageReply(v: unknown): HostUiPostMessageReply {
  bounded(v)
  const o = object(v, ['delivered'])
  return { delivered: integer(o.delivered, 0, 16) }
}
export function parseHostProcessRequest(v: unknown): HostProcessRequest {
  bounded(v)
  const o = object(v, ['id'])
  return { id: local(o.id) }
}
export type HostProcessState = 'stopped' | 'starting' | 'running' | 'exited' | 'crashed' | 'backoff'
export interface HostProcessStatus {
  state: HostProcessState
  pid?: number
  exitCode?: number
  startedAt?: number
  restarts: number
}
export interface HostProcessLogsRequest {
  id: string
  tail: number
}
export interface HostProcessLogLine {
  stream: 'stdout' | 'stderr'
  at: number
  text: string
}
export interface HostProcessLogsReply {
  lines: HostProcessLogLine[]
  truncated: boolean
}
export function parseHostProcessStatus(v: unknown): HostProcessStatus {
  bounded(v)
  const o = object(v, ['state', 'restarts'], ['pid', 'exitCode', 'startedAt'])
  return {
    state: choice(o.state, ['stopped', 'starting', 'running', 'exited', 'crashed', 'backoff']),
    restarts: integer(o.restarts),
    ...(own(o, 'pid') ? { pid: integer(o.pid, 1) } : {}),
    ...(own(o, 'exitCode') ? { exitCode: integer(o.exitCode, -2147483648, 2147483647) } : {}),
    ...(own(o, 'startedAt') ? { startedAt: integer(o.startedAt) } : {})
  }
}
export function parseHostProcessLogsRequest(v: unknown): HostProcessLogsRequest {
  bounded(v)
  const o = object(v, ['id', 'tail'])
  return { id: local(o.id), tail: integer(o.tail, 0, 500) }
}
export function parseHostProcessLogsReply(v: unknown): HostProcessLogsReply {
  bounded(v)
  const o = object(v, ['lines', 'truncated'])
  const lines = array(o.lines, 500).map((v) => {
    const line = object(v, ['stream', 'at', 'text'])
    return {
      stream: choice(line.stream, ['stdout', 'stderr']),
      at: integer(line.at),
      text: text(line.text, 0, 4096)
    }
  })
  return { lines, truncated: boolean(o.truncated) }
}
export const parseContextSnapshotRequest = parseHostContextSnapshotRequest
export function parseHostUiGetRequest(v: unknown): HostUiGetRequest {
  bounded(v)
  object(v, [])
  return {}
}
export function parseHostAcceptedResult(v: unknown): HostAcceptedResult {
  bounded(v)
  const o = object(v, ['accepted', 'id'])
  if (o.accepted !== true) bad()
  return { accepted: true, id: local(o.id) }
}
export function parseNotificationCloseResult(v: unknown): NotificationCloseResult {
  bounded(v)
  const o = object(v, ['closed'])
  if (o.closed !== true) bad()
  return { closed: true }
}
export function parseStatusClearResult(v: unknown): StatusClearResult {
  bounded(v)
  const o = object(v, ['cleared'])
  if (o.cleared !== true) bad()
  return { cleared: true }
}
