import type { AgentHooksApi } from '@shared/types/agent-hooks.types'
import type { IpcResult } from '@shared/types/ipc.types'
import { invoke } from '@tauri-apps/api/core'
import { isTauriContext } from './tauri-runtime'
import { webServerAgentHooks } from './web-server-api'

function unwrap<T>(result: IpcResult<T>): T {
  if (!result.success) throw new Error(result.error)
  return result.data
}

export function createTauriAgentHooksApi(): AgentHooksApi {
  return {
    status: () => invoke('agent_hooks_status'),
    preview: (agent) => invoke('agent_hooks_preview', { agent }),
    install: (agent) => invoke('agent_hooks_install', { agent }),
    uninstall: (agent) => invoke('agent_hooks_uninstall', { agent }),
    events: (after) => invoke('agent_hook_events', { after: after ?? null })
  }
}

export const webAgentHooksApi: AgentHooksApi = {
  status: async () => unwrap(await webServerAgentHooks.status()),
  preview: async (agent) => unwrap(await webServerAgentHooks.preview(agent)),
  install: async (agent) => unwrap(await webServerAgentHooks.install(agent)),
  uninstall: async (agent) => unwrap(await webServerAgentHooks.uninstall(agent)),
  events: async (after) => unwrap(await webServerAgentHooks.events(after))
}

const desktop = createTauriAgentHooksApi()
/** Resolve the runtime at call time so both renderer roots share identical behavior. */
export const agentHooksApi: AgentHooksApi = {
  status: () => (isTauriContext() ? desktop : webAgentHooksApi).status(),
  preview: (agent) => (isTauriContext() ? desktop : webAgentHooksApi).preview(agent),
  install: (agent) => (isTauriContext() ? desktop : webAgentHooksApi).install(agent),
  uninstall: (agent) => (isTauriContext() ? desktop : webAgentHooksApi).uninstall(agent),
  events: (after) => (isTauriContext() ? desktop : webAgentHooksApi).events(after)
}
