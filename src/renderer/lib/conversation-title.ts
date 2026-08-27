import type { ConversationRecordV2 } from '@shared/types/conversation.types'

const canonicalConversationId = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i

export function isOpaqueConversationLabel(value: string): boolean {
  return canonicalConversationId.test(value.trim())
}

function workspaceFolderName(workspaceCwd: string): string {
  return workspaceCwd.split(/[\\/]/).filter(Boolean).pop() ?? ''
}

/** Prefer a stored alias, then an ACP session title. Never show a UUID folder name. */
export function displayConversationTitle(
  conversation: Pick<ConversationRecordV2, 'title' | 'conversationId' | 'workspaceCwd'>,
  fallbacks: { sessionTitle?: string | null; untitled: string }
): string {
  const titled = conversation.title?.trim()
  if (titled && !isOpaqueConversationLabel(titled)) return titled
  const sessionTitle = fallbacks.sessionTitle?.trim()
  if (sessionTitle && !isOpaqueConversationLabel(sessionTitle)) return sessionTitle
  const folder = workspaceFolderName(conversation.workspaceCwd)
  if (folder && !isOpaqueConversationLabel(folder)) return folder
  return fallbacks.untitled
}

export function sessionTitleForConversation(
  conversationId: string,
  sessions: Record<string, { conversationId?: string; title?: string | null }>,
  index: Array<{ conversationId?: string; title?: string }>
): string | undefined {
  const live = Object.values(sessions).find(
    (session) => session.conversationId === conversationId
  )?.title
  if (live?.trim()) return live.trim()
  const indexed = index.find((entry) => entry.conversationId === conversationId)?.title
  if (indexed?.trim()) return indexed.trim()
  return undefined
}

/** Keep a human title when an open/get payload arrives untitled. */
export function mergeConversationTitle(
  current: ConversationRecordV2 | undefined,
  incoming: ConversationRecordV2
): ConversationRecordV2 {
  if (incoming.title?.trim()) return incoming
  if (!current?.title?.trim()) return incoming
  return {
    ...incoming,
    title: current.title,
    titleSource: current.titleSource ?? incoming.titleSource
  }
}
