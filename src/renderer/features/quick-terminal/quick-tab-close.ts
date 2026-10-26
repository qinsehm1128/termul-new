/** The visible quick workspace owns close commands, not the hidden project panes. */
let activeClaim: { close: () => void } | null = null

export function isQuickTerminalPath(path: string): boolean {
  return path === '/quick-terminals' || path.startsWith('/quick-terminals/')
}

export function claimQuickTabClose(handler: () => void): () => void {
  const claim = { close: handler }
  activeClaim = claim
  return () => {
    if (activeClaim === claim) activeClaim = null
  }
}

/** An empty quick workspace or the quick list deliberately consumes this as a no-op. */
export function closeQuickTab(): void {
  activeClaim?.close()
}
