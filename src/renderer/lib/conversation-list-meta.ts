export function agentIdForConversation(
  conversationId: string,
  sessions: Record<string, { conversationId?: string; agentId?: string }>,
  index: Array<{ conversationId?: string; agentId?: string }>
): string | undefined {
  const live = Object.values(sessions).find(
    (session) => session.conversationId === conversationId
  )?.agentId
  if (live) return live
  return index.find((entry) => entry.conversationId === conversationId)?.agentId
}
