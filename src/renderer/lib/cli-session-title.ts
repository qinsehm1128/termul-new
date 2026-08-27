import type { DiscoveredCliSession } from '@shared/types/cli-session.types'

export function cliSessionListTitle(
  session: DiscoveredCliSession,
  loading: string,
  untitled: string
): string {
  if (!session.resumable && !session.title.trim() && !session.sessionId) {
    return loading
  }
  const title = session.title.trim()
  if (title && title !== session.sessionId) {
    return title
  }
  return untitled
}
