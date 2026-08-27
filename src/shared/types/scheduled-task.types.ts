import type { ConversationId, ExecutionTarget } from './conversation.types'
import type { IpcResult } from './ipc.types'

export type ScheduledTaskId = string & { readonly __brand: 'ScheduledTaskId' }
export type ScheduledTaskRunId = string & { readonly __brand: 'ScheduledTaskRunId' }

export type ScheduledTaskStatus = 'draft' | 'active' | 'paused'
export type ScheduledTaskRunStatus =
  | 'queued'
  | 'running'
  | 'succeeded'
  | 'failed'
  | 'cancelled'
  | 'skipped'
  | 'interrupted'
export type ScheduledTaskRunTrigger = 'scheduled' | 'manual' | 'catchUp' | 'retry'
export type ScheduledTaskOverlapPolicy = 'skip' | 'bufferOne'
export type ScheduledTaskCatchUpPolicy = 'skip' | 'latestOnce'

export type ScheduledTaskSchedule =
  | { kind: 'cron'; expression: string; timezone: string }
  | { kind: 'interval'; everySeconds: number; anchorAt: string }
  | { kind: 'at'; at: string }

export interface ScheduledTaskExecutionPolicy {
  overlap: ScheduledTaskOverlapPolicy
  catchUp: ScheduledTaskCatchUpPolicy
  catchUpWindowSeconds: number
}

export interface ScheduledTaskRecordV1 {
  schemaVersion: 1
  taskId: ScheduledTaskId
  projectId: string | null
  name: string
  description: string
  status: ScheduledTaskStatus
  schedule: ScheduledTaskSchedule
  executionPolicy: ScheduledTaskExecutionPolicy
  prompt: string
  agentConfigId: string
  executionTarget: ExecutionTarget
  executionCwd: string
  workspaceCwd: string
  sourceConversationId: ConversationId | null
  permissions: string[]
  skillTemplateVersion: number
  revision: number
  draftHash: string
  createdAt: string
  updatedAt: string
  nextRunAt: string | null
}

export interface ScheduledTaskDraftInput {
  projectId?: string | null
  name: string
  description: string
  schedule: ScheduledTaskSchedule
  executionPolicy?: Partial<ScheduledTaskExecutionPolicy>
  prompt: string
  agentConfigId: string
  executionTarget: ExecutionTarget
  executionCwd: string
  workspaceCwd: string
  sourceConversationId?: ConversationId | null
  permissions?: string[]
}

export interface ScheduledTaskSchedulePreview {
  normalized: ScheduledTaskSchedule
  nextRunTimes: string[]
}

export interface ScheduledTaskRunV1 {
  schemaVersion: 1
  runId: ScheduledTaskRunId
  taskId: ScheduledTaskId
  projectId: string | null
  trigger: ScheduledTaskRunTrigger
  status: ScheduledTaskRunStatus
  occurrenceKey: string
  scheduledFor: string
  queuedAt: string
  startedAt: string | null
  finishedAt: string | null
  conversationId: ConversationId | null
  taskSnapshotHash: string
  retryOfRunId: ScheduledTaskRunId | null
  summary: string | null
  errorCode: string | null
  errorDetail: string | null
  usage: unknown | null
}

export type ScheduledTaskAuditActor = 'human' | 'agent' | 'system'

export interface ScheduledTaskAuditEventV1 {
  schemaVersion: 1
  eventId: string
  taskId: ScheduledTaskId
  projectId: string | null
  action: string
  actor: ScheduledTaskAuditActor
  sourceConversationId: ConversationId | null
  sourceToolCallId: string | null
  beforeHash: string | null
  afterHash: string | null
  createdAt: string
}

export interface ScheduledTaskApi {
  previewSchedule(schedule: ScheduledTaskSchedule): Promise<IpcResult<ScheduledTaskSchedulePreview>>
  listTasks(projectId?: string): Promise<IpcResult<ScheduledTaskRecordV1[]>>
  getTask(taskId: ScheduledTaskId): Promise<IpcResult<ScheduledTaskRecordV1>>
  createDraft(input: ScheduledTaskDraftInput): Promise<IpcResult<ScheduledTaskRecordV1>>
  updateDraft(
    taskId: ScheduledTaskId,
    expectedRevision: number,
    input: ScheduledTaskDraftInput
  ): Promise<IpcResult<ScheduledTaskRecordV1>>
  activateTask(
    taskId: ScheduledTaskId,
    expectedRevision: number,
    expectedDraftHash: string
  ): Promise<IpcResult<ScheduledTaskRecordV1>>
  pauseTask(
    taskId: ScheduledTaskId,
    expectedRevision: number
  ): Promise<IpcResult<ScheduledTaskRecordV1>>
  resumeTask(
    taskId: ScheduledTaskId,
    expectedRevision: number
  ): Promise<IpcResult<ScheduledTaskRecordV1>>
  deleteTask(taskId: ScheduledTaskId, expectedRevision: number): Promise<IpcResult<void>>
  runNow(taskId: ScheduledTaskId): Promise<IpcResult<ScheduledTaskRunV1>>
  retryRun(
    taskId: ScheduledTaskId,
    runId: ScheduledTaskRunId
  ): Promise<IpcResult<ScheduledTaskRunV1>>
  listRuns(taskId: ScheduledTaskId): Promise<IpcResult<ScheduledTaskRunV1[]>>
  listAudit(taskId: ScheduledTaskId): Promise<IpcResult<ScheduledTaskAuditEventV1[]>>
}

export function parseScheduledTaskId(value: string): ScheduledTaskId {
  if (!/^[0-9a-f-]{36}$/i.test(value)) throw new TypeError('scheduled task id is invalid')
  return value as ScheduledTaskId
}

export function parseScheduledTaskRunId(value: string): ScheduledTaskRunId {
  if (!/^[0-9a-f-]{36}$/i.test(value)) throw new TypeError('scheduled task run id is invalid')
  return value as ScheduledTaskRunId
}

function record(value: unknown, label: string): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError(`${label} must be an object`)
  }
  return value as Record<string, unknown>
}

export function parseScheduledTask(value: unknown): ScheduledTaskRecordV1 {
  const candidate = record(value, 'scheduled task')
  if (candidate.schemaVersion !== 1) throw new TypeError('scheduled task schema is unsupported')
  parseScheduledTaskId(String(candidate.taskId))
  if (
    (candidate.projectId !== null && typeof candidate.projectId !== 'string') ||
    typeof candidate.name !== 'string'
  ) {
    throw new TypeError('scheduled task identity is invalid')
  }
  if (!['draft', 'active', 'paused'].includes(String(candidate.status))) {
    throw new TypeError('scheduled task status is invalid')
  }
  record(candidate.schedule, 'scheduled task schedule')
  if (!Number.isSafeInteger(candidate.revision) || Number(candidate.revision) < 1) {
    throw new TypeError('scheduled task revision is invalid')
  }
  if (typeof candidate.draftHash !== 'string' || candidate.draftHash.length !== 64) {
    throw new TypeError('scheduled task draft hash is invalid')
  }
  return value as ScheduledTaskRecordV1
}

export function parseScheduledTaskArray(value: unknown): ScheduledTaskRecordV1[] {
  if (!Array.isArray(value)) throw new TypeError('scheduled task list must be an array')
  value.forEach(parseScheduledTask)
  return value as ScheduledTaskRecordV1[]
}

export function parseScheduledTaskRun(value: unknown): ScheduledTaskRunV1 {
  const candidate = record(value, 'scheduled task run')
  if (candidate.schemaVersion !== 1) throw new TypeError('scheduled task run schema is unsupported')
  parseScheduledTaskRunId(String(candidate.runId))
  parseScheduledTaskId(String(candidate.taskId))
  if (
    !['queued', 'running', 'succeeded', 'failed', 'cancelled', 'skipped', 'interrupted'].includes(
      String(candidate.status)
    )
  ) {
    throw new TypeError('scheduled task run status is invalid')
  }
  return value as ScheduledTaskRunV1
}

export function parseScheduledTaskRunArray(value: unknown): ScheduledTaskRunV1[] {
  if (!Array.isArray(value)) throw new TypeError('scheduled task runs must be an array')
  value.forEach(parseScheduledTaskRun)
  return value as ScheduledTaskRunV1[]
}

export function parseScheduledTaskAuditArray(value: unknown): ScheduledTaskAuditEventV1[] {
  if (!Array.isArray(value)) throw new TypeError('scheduled task audit must be an array')
  for (const entry of value) {
    const candidate = record(entry, 'scheduled task audit event')
    if (candidate.schemaVersion !== 1 || typeof candidate.action !== 'string') {
      throw new TypeError('scheduled task audit event is invalid')
    }
  }
  return value as ScheduledTaskAuditEventV1[]
}

export function parseScheduledTaskPreview(value: unknown): ScheduledTaskSchedulePreview {
  const candidate = record(value, 'scheduled task preview')
  record(candidate.normalized, 'scheduled task normalized schedule')
  if (!Array.isArray(candidate.nextRunTimes)) {
    throw new TypeError('scheduled task preview times are invalid')
  }
  return value as ScheduledTaskSchedulePreview
}
