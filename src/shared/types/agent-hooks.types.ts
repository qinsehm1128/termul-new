/** Host-owned hook installation and event transport contracts (wire fields are snake_case). */
export const HOOK_AGENT_KINDS = ['claude-code', 'pi', 'qin-code'] as const
export type AgentKind = (typeof HOOK_AGENT_KINDS)[number]
export type HookInstallState =
  | 'installed'
  | 'outdated'
  | 'not_installed'
  | 'agent_not_found'
  | 'error'

export interface HookInstallStatus {
  agent: AgentKind
  state: HookInstallState
  config_path: string
  detail: string | null
  cli_path?: string | null
  version?: string | null
}

export type AgentHookKind =
  | 'session_start'
  | 'session_end'
  | 'prompt_submitted'
  | 'tool_start'
  | 'tool_end'
  | 'needs_approval'
  | 'blocked'
  | 'idle'
  | 'turn_done'
  | 'error'

export interface AgentHookEvent {
  sequence: number
  terminal_id: string
  project_id: string
  agent: AgentKind
  session_id: string
  transcript_path?: string
  kind: AgentHookKind
  at: number
  tool_name?: string
  ok?: boolean
}

export interface AgentHooksApi {
  status(): Promise<HookInstallStatus[]>
  preview(agent: AgentKind): Promise<string>
  install(agent: AgentKind): Promise<HookInstallStatus>
  uninstall(agent: AgentKind): Promise<HookInstallStatus>
  events(after?: number): Promise<AgentHookEvent[]>
}
