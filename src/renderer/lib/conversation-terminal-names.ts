/**
 * Display names for the Conversations that own terminals.
 *
 * Every surface that lists terminals — the switcher bar, the terminal board,
 * the list panel — needs to say *which* Conversation a terminal belongs to.
 * Each building that map itself would give the same Conversation a different
 * label per surface, and the untitled fallback is exactly where they would
 * drift first.
 */

import type { ConversationRecordV2 } from '@shared/types/conversation.types'

/**
 * Map Conversation id → the label to show, for the Conversations that actually
 * own one of `terminals`.
 *
 * Scoped to the terminals in hand rather than every Conversation on record:
 * the map exists to label chips that are on screen, and the store holds every
 * Conversation the user has ever opened.
 */
export function buildConversationTerminalNames(
  terminals: readonly { conversationId?: string }[],
  summariesById: Readonly<Record<string, Pick<ConversationRecordV2, 'title'> | undefined>>,
  untitledLabel: string
): Map<string, string> {
  const names = new Map<string, string>()
  for (const terminal of terminals) {
    const conversationId = terminal.conversationId
    if (!conversationId || names.has(conversationId)) continue
    const title = summariesById[conversationId]?.title?.trim()
    names.set(conversationId, title || untitledLabel)
  }
  return names
}
