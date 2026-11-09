import type { AgentHookEvent } from '@shared/types/agent-hooks.types'
import type { TerminalAgentSession } from '@shared/types/cli-session.types'
import { toTerminalAgentSession } from './live-agent-sessions'

/** Hook payloads are metadata, never executable argv supplied by another process. */
export function hookEventSession(event: AgentHookEvent): TerminalAgentSession | null {
  if (!['claude-code', 'pi', 'qin-code'].includes(event.agent)) return null
  if (!/^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,511}$/.test(event.session_id)) return null
  return {
    ...toTerminalAgentSession({
      rootPid: 0,
      agentId: event.agent,
      sessionId: event.session_id,
      cwd: null,
      selfDev: false
    }),
    source: 'hook',
    hook_at: Number.isFinite(event.at) && event.at > 0 ? event.at : Date.now(),
    hook_kind: event.kind,
    ...(event.transcript_path ? { transcript_path: event.transcript_path } : {})
  }
}
