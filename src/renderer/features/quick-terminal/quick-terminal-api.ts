import type { QuickTerminalApi } from '@shared/types/quick-terminal.types'
import { isTauriContext } from '@/lib/tauri-runtime'
import { tauriQuickTerminalApi } from './tauri-quick-terminal-api'
import { webQuickTerminalApi } from './web-quick-terminal-api'

export const quickTerminalApi: QuickTerminalApi = isTauriContext()
  ? tauriQuickTerminalApi
  : webQuickTerminalApi
