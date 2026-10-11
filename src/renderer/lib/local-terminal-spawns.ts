/**
 * Which PTY spawns this renderer started itself.
 *
 * The host announces every spawn to every client (`terminal-spawned`) so the
 * host terminal catalog can pick up terminals opened on another device. A
 * spawn this renderer started must not be picked up that way: its caller adds
 * the record itself, sometimes only after further awaits — restore commits
 * every terminal at once at the end — and a catalog copy taken in between
 * leaves two records on one PTY. Every local spawn goes through
 * `terminalApi.spawn`, which marks it here.
 */

import type { IpcResult, SpawnedTerminal } from '@shared/types/ipc.types'

let inFlight = 0
const spawnedIds = new Set<string>()

/** How long a finished spawn still claims its late-arriving host event. */
const LATE_EVENT_WINDOW_MS = 10_000

/** Run one local spawn, marking it for as long as its host event may arrive. */
export async function trackLocalTerminalSpawn(
  spawn: () => Promise<IpcResult<SpawnedTerminal>>
): Promise<IpcResult<SpawnedTerminal>> {
  inFlight += 1
  try {
    const result = await spawn()
    if (result.success) {
      const id = result.data.id
      spawnedIds.add(id)
      setTimeout(() => spawnedIds.delete(id), LATE_EVENT_WINDOW_MS)
    }
    return result
  } finally {
    inFlight = Math.max(0, inFlight - 1)
  }
}

/**
 * Whether a `terminal-spawned` event belongs to a spawn this renderer started.
 * While any local spawn is awaiting its reply the event cannot be told apart
 * yet, so it counts as local; afterwards the returned id is matched once.
 */
export function isLocalTerminalSpawnEvent(terminalId: string): boolean {
  return inFlight > 0 || spawnedIds.delete(terminalId)
}
