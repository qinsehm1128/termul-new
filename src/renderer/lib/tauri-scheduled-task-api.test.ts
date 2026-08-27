import { beforeEach, expect, it, vi } from 'vitest'

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

import { createTauriScheduledTaskApi } from './tauri-scheduled-task-api'

const taskId = '11111111-1111-4111-8111-111111111111'
const task = {
  schemaVersion: 1,
  taskId,
  projectId: 'project-1',
  name: 'Daily review',
  description: '',
  status: 'draft',
  schedule: { kind: 'cron', expression: '0 9 * * *', timezone: 'Asia/Shanghai' },
  executionPolicy: {
    overlap: 'bufferOne',
    catchUp: 'latestOnce',
    catchUpWindowSeconds: 86400
  },
  prompt: 'Review the project',
  agentConfigId: 'codex',
  executionTarget: { kind: 'workspace' },
  executionCwd: '/project',
  workspaceCwd: '/conversation',
  sourceConversationId: null,
  permissions: [],
  skillTemplateVersion: 1,
  revision: 1,
  draftHash: 'a'.repeat(64),
  createdAt: '2026-08-20T00:00:00Z',
  updatedAt: '2026-08-20T00:00:00Z',
  nextRunAt: '2026-08-21T01:00:00Z'
}

beforeEach(() => invokeMock.mockReset())

it('uses the desktop command surface and decodes task records', async () => {
  invokeMock.mockResolvedValue({ success: true, data: task })
  const result = await createTauriScheduledTaskApi().activateTask(taskId, 1, task.draftHash)

  expect(result).toEqual({ success: true, data: task })
  expect(invokeMock).toHaveBeenCalledWith('scheduled_task_activate', {
    taskId,
    request: { expectedRevision: 1, expectedDraftHash: task.draftHash }
  })
})

it('fails closed when the desktop success payload is malformed', async () => {
  invokeMock.mockResolvedValue({ success: true, data: { taskId } })
  const result = await createTauriScheduledTaskApi().getTask(taskId)
  expect(result).toMatchObject({ success: false, code: 'NETWORK_ERROR' })
})
