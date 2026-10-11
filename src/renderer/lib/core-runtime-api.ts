import { invoke } from '@tauri-apps/api/core'
import { relaunch } from '@tauri-apps/plugin-process'

/** The Cores the user can restart on their own; the GUI restarts via `relaunch`. */
export type RestartableCore = 'acpCore' | 'terminalCore'

/** One Core as `core_runtime_status` reports it. */
export interface CoreProcessStatus {
  /** `separate`: its own process; `inProcess`: runs inside the GUI process. */
  mode: 'separate' | 'inProcess'
  running: boolean
  pid: number | null
  startedAtMs: number | null
  buildId: string | null
  /** Running terminals (Terminal Core) or agent sessions (ACP Core). */
  activeResources: number
}

export interface GuiProcessStatus {
  pid: number
  startedAtMs: number | null
  buildId: string
  version: string
}

/** One PTY the Terminal Core holds. */
export interface CoreTerminalStatus {
  id: string
  shell: string
  cwd: string
  /** Shell process id. */
  pid: number
  projectId?: string
  conversationId?: string
  active: boolean
  lifecycle: string
}

export interface CoreRuntimeStatus {
  gui: GuiProcessStatus
  acpCore: CoreProcessStatus
  terminalCore: CoreProcessStatus
  /** `null` when the Terminal Core could not be asked. */
  terminals: CoreTerminalStatus[] | null
  /** Live ACP agent ids; `null` when the ACP Core could not be asked. */
  agents: string[] | null
}

/** Desktop-only: a browser client has no local processes to inspect. */
export const coreRuntimeApi = {
  status: (): Promise<CoreRuntimeStatus> => invoke<CoreRuntimeStatus>('core_runtime_status'),
  /** Ends everything the Core runs; resolves once the new Core is connected. */
  restartCore: (role: RestartableCore): Promise<void> => invoke<void>('restart_core', { role }),
  /** Terminals and agents keep running in their Cores and are adopted again. */
  restartGui: (): Promise<void> => relaunch()
}
