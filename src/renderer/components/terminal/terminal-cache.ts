/**
 * Terminal Cache
 *
 * Keeps xterm Terminal instances alive across project switches by caching
 * them in memory instead of calling terminal.dispose() on unmount.
 *
 * Without this, terminal.dispose() destroys ALL terminal state (scrollback,
 * alt buffer, cursor position). The transcript replay mechanism can only
 * partially reconstruct it, producing garbled/truncated output — especially
 * for TUI apps running in the alt buffer.
 *
 * Usage:
 *   On unmount:  cacheTerminal(ptyId, { terminal, fitAddon, searchAddon })
 *   On remount:  takeCachedTerminal(ptyId) → CachedTerminalSession | undefined
 */

import type { FitAddon } from '@xterm/addon-fit'
import type { SearchAddon } from '@xterm/addon-search'
import type { Terminal } from '@xterm/xterm'

export interface CachedTerminalSession {
  terminal: Terminal
  fitAddon: FitAddon
  searchAddon: SearchAddon
}

const MAX_CACHED_TERMINALS = 20

/** Map of PTY ID → cached xterm session, ordered from oldest to newest. */
const cache = new Map<string, CachedTerminalSession>()

function disposeSession(session: CachedTerminalSession): void {
  try {
    session.terminal.dispose()
  } catch {
    // Already disposed in another lifecycle path.
  }
}

function evictOldestSession(): void {
  const oldest = cache.entries().next().value as [string, CachedTerminalSession] | undefined
  if (!oldest) return
  cache.delete(oldest[0])
  disposeSession(oldest[1])
}

/**
 * Store a terminal and its persistent addons, then detach its DOM element.
 * Call this in the cleanup (unmount) path instead of terminal.dispose().
 *
 * If a terminal is already cached for the same PTY ID (which can happen
 * during rapid project switches A -> B -> A -> B where React schedules
 * effects across two renders), we dispose the previous occupant before
 * caching the new one. Without this, the older xterm instance leaks and
 * its detached element keeps event listeners alive that can starve the
 * live renderer of focus / keystrokes — visible to users as "terminal
 * freezes after rapid switching".
 */
export function cacheTerminal(ptyId: string, session: CachedTerminalSession): void {
  const existing = cache.get(ptyId)
  if (existing && existing.terminal !== session.terminal) {
    // Different instance already cached for this PTY — dispose the old
    // one to release its WebGL context, addons, and DOM node.
    disposeSession(existing)
    cache.delete(ptyId)
  } else if (existing?.terminal === session.terminal) {
    // Same session re-cached — refresh its recency and ensure DOM is detached.
    cache.delete(ptyId)
    session.terminal.element?.remove()
    cache.set(ptyId, session)
    return
  }

  // Detach the xterm element from the DOM so it doesn't linger in the
  // old container. The element is preserved for reattachment later.
  session.terminal.element?.remove()

  cache.set(ptyId, session)
  while (cache.size > MAX_CACHED_TERMINALS) {
    evictOldestSession()
  }
}

/**
 * Retrieve and remove a cached terminal session.
 * Returns undefined if no cached terminal exists for this PTY ID.
 * The caller is responsible for reattaching the terminal element via:
 *   container.appendChild(session.terminal.element!)
 */
export function takeCachedTerminal(ptyId: string): CachedTerminalSession | undefined {
  const session = cache.get(ptyId)
  if (session) {
    cache.delete(ptyId)
  }
  return session
}

/**
 * Check if a terminal is in the cache without removing it.
 */
export function hasCachedTerminal(ptyId: string): boolean {
  return cache.has(ptyId)
}

/** Dispose one cached renderer after its terminal resource is explicitly closed. */
export function disposeCachedTerminal(ptyId: string): boolean {
  const session = cache.get(ptyId)
  if (!session) return false
  cache.delete(ptyId)
  disposeSession(session)
  return true
}

/**
 * Clear all cached terminals — useful for testing or app-wide cleanup.
 */
export function clearTerminalCache(): void {
  for (const session of cache.values()) {
    disposeSession(session)
  }
  cache.clear()
}
