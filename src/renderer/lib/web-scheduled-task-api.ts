import type { IpcDataDecoder, IpcResult } from '@shared/types/ipc.types'
import type {
  ScheduledTaskApi,
  ScheduledTaskAuditEventV1,
  ScheduledTaskRecordV1,
  ScheduledTaskRunV1,
  ScheduledTaskSchedulePreview
} from '@shared/types/scheduled-task.types'
import {
  parseScheduledTask,
  parseScheduledTaskArray,
  parseScheduledTaskAuditArray,
  parseScheduledTaskPreview,
  parseScheduledTaskRun,
  parseScheduledTaskRunArray
} from '@shared/types/scheduled-task.types'
import { requestHttpIpcResult } from '@/lib/http-ipc-result'
import { remoteAccessHeaders } from './acp-transport'

function request<T>(
  path: string,
  decoder: IpcDataDecoder<T>,
  init: RequestInit = { method: 'GET' }
): Promise<IpcResult<T>> {
  return requestHttpIpcResult(
    `${typeof window === 'undefined' ? '' : window.location.origin}${path}`,
    { ...init, headers: remoteAccessHeaders(init.headers) },
    decoder
  )
}

function post<T>(path: string, body: unknown, decoder: IpcDataDecoder<T>): Promise<IpcResult<T>> {
  return request(path, decoder, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body)
  })
}

const decodeVoid = (): void => undefined
const taskPath = (taskId: string): string => `/scheduled-tasks/${encodeURIComponent(taskId)}`

export function createWebScheduledTaskApi(): ScheduledTaskApi {
  return {
    previewSchedule: (schedule) =>
      post<ScheduledTaskSchedulePreview>(
        '/scheduled-tasks/preview',
        { schedule, count: 5 },
        parseScheduledTaskPreview
      ),
    listTasks: (projectId) =>
      request<ScheduledTaskRecordV1[]>(
        `/scheduled-tasks${projectId ? `?projectId=${encodeURIComponent(projectId)}` : ''}`,
        parseScheduledTaskArray
      ),
    getTask: (taskId) => request(taskPath(taskId), parseScheduledTask),
    createDraft: (input) => post('/scheduled-tasks/drafts', input, parseScheduledTask),
    updateDraft: (taskId, expectedRevision, input) =>
      post(`${taskPath(taskId)}/draft`, { expectedRevision, input }, parseScheduledTask),
    activateTask: (taskId, expectedRevision, expectedDraftHash) =>
      post(
        `${taskPath(taskId)}/activate`,
        { expectedRevision, expectedDraftHash },
        parseScheduledTask
      ),
    pauseTask: (taskId, expectedRevision) =>
      post(`${taskPath(taskId)}/pause`, { expectedRevision }, parseScheduledTask),
    resumeTask: (taskId, expectedRevision) =>
      post(`${taskPath(taskId)}/resume`, { expectedRevision }, parseScheduledTask),
    deleteTask: (taskId, expectedRevision) =>
      post<void>(`${taskPath(taskId)}/delete`, { expectedRevision }, decodeVoid),
    runNow: (taskId) =>
      post<ScheduledTaskRunV1>(`${taskPath(taskId)}/run`, {}, parseScheduledTaskRun),
    retryRun: (taskId, runId) =>
      post<ScheduledTaskRunV1>(
        `${taskPath(taskId)}/runs/${encodeURIComponent(runId)}/retry`,
        {},
        parseScheduledTaskRun
      ),
    listRuns: (taskId) =>
      request<ScheduledTaskRunV1[]>(`${taskPath(taskId)}/runs`, parseScheduledTaskRunArray),
    listAudit: (taskId) =>
      request<ScheduledTaskAuditEventV1[]>(
        `${taskPath(taskId)}/audit`,
        parseScheduledTaskAuditArray
      )
  }
}

export const webScheduledTaskApi = createWebScheduledTaskApi()
