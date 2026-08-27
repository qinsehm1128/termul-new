import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { createWebScheduledTaskApi } from './web-scheduled-task-api'

const taskId = '11111111-1111-4111-8111-111111111111'
const task = {
  schemaVersion: 1,
  taskId,
  projectId: null,
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
  skillTemplateVersion: 2,
  revision: 1,
  draftHash: 'b'.repeat(64),
  createdAt: '2026-08-20T00:00:00Z',
  updatedAt: '2026-08-20T00:00:00Z',
  nextRunAt: '2026-08-21T01:00:00Z'
}

function response(body: unknown): Response {
  return {
    ok: true,
    status: 200,
    statusText: 'OK',
    json: vi.fn(async () => body)
  } as unknown as Response
}

beforeEach(() => vi.stubGlobal('fetch', vi.fn()))
afterEach(() => vi.unstubAllGlobals())

it('uses the authenticated web route with the same activation contract', async () => {
  vi.mocked(fetch).mockResolvedValue(response({ success: true, data: task }))
  const result = await createWebScheduledTaskApi().activateTask(taskId, 1, task.draftHash)

  expect(result).toEqual({ success: true, data: task })
  expect(fetch).toHaveBeenCalledWith(
    expect.stringContaining(`/scheduled-tasks/${taskId}/activate`),
    expect.objectContaining({
      method: 'POST',
      body: JSON.stringify({ expectedRevision: 1, expectedDraftHash: task.draftHash })
    })
  )
})

it('fails closed when the web success payload is malformed', async () => {
  vi.mocked(fetch).mockResolvedValue(response({ success: true, data: { taskId } }))
  const result = await createWebScheduledTaskApi().getTask(taskId)
  expect(result).toMatchObject({ success: false, code: 'NETWORK_ERROR' })
})
