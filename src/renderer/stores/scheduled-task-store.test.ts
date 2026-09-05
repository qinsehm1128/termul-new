import { beforeEach, expect, it, vi } from 'vitest'

const { listTasks } = vi.hoisted(() => ({
  listTasks: vi.fn()
}))

vi.mock('@/lib/scheduled-task-api', () => ({
  scheduledTaskApi: { listTasks }
}))

import { useScheduledTaskStore } from './scheduled-task-store'

beforeEach(() => {
  listTasks.mockReset()
  useScheduledTaskStore.setState({
    tasks: [],
    selectedTaskId: null,
    runs: [],
    audit: [],
    loading: false,
    mutating: false,
    error: null
  })
})

it('loads the Se-wide catalog without a project filter', async () => {
  listTasks.mockResolvedValue({ success: true, data: [] })

  await useScheduledTaskStore.getState().load()

  expect(listTasks).toHaveBeenCalledOnce()
  expect(listTasks).toHaveBeenCalledWith()
  expect(useScheduledTaskStore.getState()).toMatchObject({
    tasks: [],
    selectedTaskId: null,
    loading: false,
    error: null
  })
})
