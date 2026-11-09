import type { AgentHookKind } from '@shared/types/agent-hooks.types'
import type { TerminalAgentSession } from '@shared/types/cli-session.types'
import { useAgentHookStateStore } from '@/stores/agent-hook-state-store'
import type { AgentTerminalState } from './agent-terminal-state'

function runtimeForSession(session: TerminalAgentSession | undefined) {
  if (!session) return undefined
  const store = useAgentHookStateStore.getState()
  const candidates = Object.values(store.states).filter((runtime) => {
    if (
      runtime.instance_id !== store.instanceId ||
      runtime.identity.adapter_id !== session.agentId ||
      runtime.identity.agent_session_id !== session.sessionId
    )
      return false
    const ref = session.descriptor_ref
    return (
      !ref ||
      (runtime.identity.descriptor_id === ref.descriptor_id &&
        runtime.generation === ref.generation)
    )
  })
  // Legacy identities without a descriptor must not borrow another PTY's
  // evidence when the same agent session is attached in more than one place.
  return candidates.length === 1 ? candidates[0] : undefined
}

/** Explicit hook lifecycle evidence outranks screen/title heuristics. */
export function hookTerminalState(
  session: TerminalAgentSession | undefined
): AgentTerminalState | null {
  const runtime = runtimeForSession(session)
  switch (runtime?.state) {
    case 'busy':
      return 'working'
    case 'idle':
      return 'idle'
    case 'needs_approval':
    case 'blocked':
    case 'error':
      return 'blocked'
    default:
      return null
  }
}

export const HOOK_STALE_MS = 120_000

/** Silence alone cannot invalidate hooks: require output newer than the hook as well. */
export function activeHookState(
  session: TerminalAgentSession | undefined,
  lastActivityAt: number | undefined,
  now = Date.now()
): AgentTerminalState | null {
  const state = hookTerminalState(session)
  if (state === null) return null
  const at = runtimeForSession(session)?.hook_at
  const mayYield =
    typeof at === 'number' &&
    now - at > HOOK_STALE_MS &&
    typeof lastActivityAt === 'number' &&
    lastActivityAt > at
  return mayYield ? null : state
}

/** Missing/unknown screen evidence never overrides even stale hook evidence. */
export function resolveHookFallback(
  session: TerminalAgentSession | undefined,
  lastActivityAt: number | undefined,
  derived: AgentTerminalState,
  now = Date.now()
): AgentTerminalState {
  return (
    activeHookState(session, lastActivityAt, now) ??
    (derived === 'unknown' ? (hookTerminalState(session) ?? derived) : derived)
  )
}

export function hookKindState(kind: AgentHookKind): AgentTerminalState | null {
  switch (kind) {
    case 'needs_approval':
    case 'blocked':
      return 'blocked'
    case 'prompt_submitted':
    case 'tool_start':
    case 'tool_end':
      return 'working'
    case 'session_start':
    case 'idle':
    case 'turn_done':
      return 'idle'
    case 'error':
      return 'blocked'
    case 'session_end':
      return null
  }
}
