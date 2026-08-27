let navigateFn: ((path: string) => void) | null = null

export function setRouterNavigate(fn: ((path: string) => void) | null): void {
  navigateFn = fn
}

/** Independent Conversation area: list + an open conversation. Not the project workspace. */
export function isConversationAreaPath(pathname: string): boolean {
  return pathname === '/conversations' || pathname.startsWith('/c/')
}

export function navigateToPath(path: string): void {
  if (!navigateFn) return
  if (window.location.hash !== `#${path}`) navigateFn(path)
}

export function navigateToConversation(conversationId: string): void {
  if (!navigateFn) return
  const target = `/c/${encodeURIComponent(conversationId)}`
  if (window.location.hash !== `#${target}`) navigateFn(target)
}

export function navigateToChatSession(sessionId: string): void {
  if (!navigateFn) return
  const target = `/legacy/session/${encodeURIComponent(sessionId)}`
  if (window.location.hash !== `#${target}`) {
    navigateFn(target)
  }
}

export function clearChatRoute(): void {
  if (!navigateFn) return
  if (window.location.hash.startsWith('#/c/') || window.location.hash.startsWith('#/legacy/')) {
    navigateFn('/conversations')
  }
}
