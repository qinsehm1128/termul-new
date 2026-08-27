import type {
  ScheduledTaskAuditEventV1,
  ScheduledTaskId,
  ScheduledTaskRecordV1,
  ScheduledTaskRunV1
} from '@shared/types/scheduled-task.types'
import { create } from 'zustand'
import { scheduledTaskApi } from '@/lib/scheduled-task-api'

interface ScheduledTaskState {
  tasks: ScheduledTaskRecordV1[]
  selectedTaskId: ScheduledTaskId | null
  runs: ScheduledTaskRunV1[]
  audit: ScheduledTaskAuditEventV1[]
  loading: boolean
  mutating: boolean
  error: string | null
  load: () => Promise<void>
  select: (taskId: ScheduledTaskId | null) => Promise<void>
  activate: (task: ScheduledTaskRecordV1) => Promise<boolean>
  pause: (task: ScheduledTaskRecordV1) => Promise<boolean>
  resume: (task: ScheduledTaskRecordV1) => Promise<boolean>
  runNow: (taskId: ScheduledTaskId) => Promise<boolean>
}

function replaceTask(
  tasks: ScheduledTaskRecordV1[],
  replacement: ScheduledTaskRecordV1
): ScheduledTaskRecordV1[] {
  return tasks.map((task) => (task.taskId === replacement.taskId ? replacement : task))
}

export const useScheduledTaskStore = create<ScheduledTaskState>((set, get) => ({
  tasks: [],
  selectedTaskId: null,
  runs: [],
  audit: [],
  loading: false,
  mutating: false,
  error: null,

  load: async () => {
    set({ loading: true, error: null })
    const result = await scheduledTaskApi.listTasks()
    if (!result.success) {
      set({ loading: false, error: result.error })
      return
    }
    const selectedTaskId = get().selectedTaskId
    set({
      tasks: result.data,
      selectedTaskId:
        selectedTaskId && result.data.some((task) => task.taskId === selectedTaskId)
          ? selectedTaskId
          : (result.data[0]?.taskId ?? null),
      loading: false
    })
    const next = get().selectedTaskId
    if (next) await get().select(next)
  },

  select: async (taskId) => {
    set({ selectedTaskId: taskId, runs: [], audit: [], error: null })
    if (!taskId) return
    const [runs, audit] = await Promise.all([
      scheduledTaskApi.listRuns(taskId),
      scheduledTaskApi.listAudit(taskId)
    ])
    if (get().selectedTaskId !== taskId) return
    if (!runs.success || !audit.success) {
      set({ error: !runs.success ? runs.error : !audit.success ? audit.error : null })
      return
    }
    set({ runs: runs.data, audit: audit.data })
  },

  activate: async (task) => {
    set({ mutating: true, error: null })
    const result = await scheduledTaskApi.activateTask(task.taskId, task.revision, task.draftHash)
    if (!result.success) {
      set({ mutating: false, error: result.error })
      return false
    }
    set((state) => ({ tasks: replaceTask(state.tasks, result.data), mutating: false }))
    return true
  },

  pause: async (task) => {
    set({ mutating: true, error: null })
    const result = await scheduledTaskApi.pauseTask(task.taskId, task.revision)
    if (!result.success) {
      set({ mutating: false, error: result.error })
      return false
    }
    set((state) => ({ tasks: replaceTask(state.tasks, result.data), mutating: false }))
    return true
  },

  resume: async (task) => {
    set({ mutating: true, error: null })
    const result = await scheduledTaskApi.resumeTask(task.taskId, task.revision)
    if (!result.success) {
      set({ mutating: false, error: result.error })
      return false
    }
    set((state) => ({ tasks: replaceTask(state.tasks, result.data), mutating: false }))
    return true
  },

  runNow: async (taskId) => {
    set({ mutating: true, error: null })
    const result = await scheduledTaskApi.runNow(taskId)
    if (!result.success) {
      set({ mutating: false, error: result.error })
      return false
    }
    set((state) => ({ runs: [result.data, ...state.runs], mutating: false }))
    return true
  }
}))

export const useSelectedScheduledTask = (): ScheduledTaskRecordV1 | undefined =>
  useScheduledTaskStore((state) => state.tasks.find((task) => task.taskId === state.selectedTaskId))
