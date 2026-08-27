export type ConversationRowStatus = 'need' | 'working' | 'idle'

export interface ConversationRowSession {
  conversationId?: string
  activeTurn?: boolean
  status?: string
}

export interface ConversationRowIndexEntry {
  id: string
  conversationId?: string
}

export interface ConversationRowPending {
  sessionId: string
}

function sessionIdsBoundToConversation(
  conversationId: string,
  sessions: Record<string, ConversationRowSession>,
  sessionIndex: readonly ConversationRowIndexEntry[]
): Set<string> {
  const bound = new Set<string>()
  for (const [sessionId, session] of Object.entries(sessions)) {
    if (session.conversationId === conversationId) bound.add(sessionId)
  }
  for (const entry of sessionIndex) {
    if (entry.conversationId === conversationId) bound.add(entry.id)
  }
  return bound
}

function pendingBelongsToConversation(
  pending: Record<string, ConversationRowPending>,
  boundSessionIds: Set<string>
): boolean {
  return Object.values(pending).some((item) => boundSessionIds.has(item.sessionId))
}

/**
 * Derive the conversation-list status chip from live ACP state.
 * Need wins over working; working is only live `activeTurn` or `initializing`.
 */
export function conversationRowStatus(
  conversationId: string,
  sessions: Record<string, ConversationRowSession>,
  sessionIndex: readonly ConversationRowIndexEntry[],
  pendingPermissions: Record<string, ConversationRowPending>,
  pendingQuestions: Record<string, ConversationRowPending> = {}
): ConversationRowStatus {
  const boundSessionIds = sessionIdsBoundToConversation(conversationId, sessions, sessionIndex)
  if (
    pendingBelongsToConversation(pendingPermissions, boundSessionIds) ||
    pendingBelongsToConversation(pendingQuestions, boundSessionIds)
  ) {
    return 'need'
  }

  for (const sessionId of boundSessionIds) {
    const live = sessions[sessionId]
    if (live && (live.activeTurn || live.status === 'initializing')) return 'working'
  }

  return 'idle'
}
