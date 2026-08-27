import type { ScheduledTaskApi } from '@shared/types/scheduled-task.types'
import { isTauriContext } from './tauri-runtime'
import { tauriScheduledTaskApi } from './tauri-scheduled-task-api'
import { webScheduledTaskApi } from './web-scheduled-task-api'

export const scheduledTaskApi: ScheduledTaskApi = isTauriContext()
  ? tauriScheduledTaskApi
  : webScheduledTaskApi
