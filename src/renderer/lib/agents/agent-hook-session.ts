import type { AgentHookEvent, HookStateDto } from '@shared/types/agent-hooks.types'
import type { TerminalAgentSession } from '@shared/types/cli-session.types'
import { toTerminalAgentSession } from './live-agent-sessions'

/** Host state identity only; runtime activity never enters the persisted layout. */
export function hookStateSession(state: HookStateDto): TerminalAgentSession | null {
  const identity = state.identity
  if (
    !identity.agent_session_id ||
    !/^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,511}$/.test(identity.agent_session_id)
  )
    return null
  return {
    agentId: identity.adapter_id,
    sessionId: identity.agent_session_id,
    source: identity.source,
    ...(identity.descriptor_id
      ? {
          descriptor_ref: { descriptor_id: identity.descriptor_id, generation: state.generation }
        }
      : {}),
    ...(identity.transcript_path ? { transcript_path: identity.transcript_path } : {}),
    ...(identity.cwd ? { cwd: identity.cwd } : {}),
    resumeArgv: [...(identity.resume_argv ?? [])]
  }
}

/** Legacy bridge is metadata only, never argv supplied by another process. */
export function hookEventSession(event: AgentHookEvent): TerminalAgentSession | null {
  if (!['claude-code', 'pi', 'qin-code'].includes(event.agent)) return null
  if (!/^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,511}$/.test(event.session_id)) return null
  return toTerminalAgentSession({
    rootPid: 0,
    agentId: event.agent,
    sessionId: event.session_id,
    cwd: null,
    selfDev: false,
    source: 'hook',
    ...(event.transcript_path ? { transcript_path: event.transcript_path } : {})
  })
}
