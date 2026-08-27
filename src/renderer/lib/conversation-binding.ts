import { isConversationId } from '@shared/types/conversation.types'
import { conversationApi } from '@/lib/conversation-api'

export function isLiveAcpSession(session?: { status?: string } | null): boolean {
  return Boolean(session && session.status && session.status !== 'closed')
}

/** Resolve the replaceable ACP session bound to a Conversation. */

export function resolveConversationSessionId(
  state: {
    sessions: Record<string, { id: string; conversationId?: string; status?: string }>
    sessionIndex: Array<{ id: string; conversationId?: string; storageKey?: string }>
  },
  conversationId: string
): string | null {
  const bound = Object.values(state.sessions).filter(
    (session) => session.conversationId === conversationId
  )
  const live = bound.find((session) => isLiveAcpSession(session))
  if (live) return live.id
  if (bound[0]) return bound[0].id
  return (
    state.sessionIndex.find((entry) => conversationIdForIndexEntry(entry) === conversationId)?.id ??
    null
  )
}

/** Ask the host for the Conversation's current Active ACP binding. */
export async function fetchHostBoundSession(conversationId: string): Promise<{
  sessionId: string
  runtimeAgentId: string
  executionCwd: string
} | null> {
  if (!isConversationId(conversationId)) return null
  try {
    const result = await conversationApi.getCurrentBinding(conversationId)
    if (!result.success) return null
    if (!result.data.binding) return null
    const binding = result.data.binding
    if (binding.state !== 'active' || !binding.agentSessionId) return null
    return {
      sessionId: binding.agentSessionId,
      runtimeAgentId: binding.runtimeAgentId,
      executionCwd: binding.executionCwd
    }
  } catch {
    return null
  }
}

/** Host summaries expose Conversation identity as `conversationId` or `storageKey`. */
export function conversationIdForIndexEntry(entry: {
  conversationId?: string
  storageKey?: string
}): string | undefined {
  if (entry.conversationId && isConversationId(entry.conversationId)) return entry.conversationId
  if (entry.storageKey && isConversationId(entry.storageKey)) return entry.storageKey
  return undefined
}
