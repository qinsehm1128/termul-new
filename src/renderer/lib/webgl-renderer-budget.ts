/**
 * Process-wide budget for xterm WebGL renderer contexts.
 *
 * Terminals used to release their WebGL context the moment their tab stopped
 * being the active one. That was motivated by a real constraint — browsers cap
 * live WebGL contexts (commonly ~16) — but tying the release to visibility
 * made every ordinary tab switch destroy one context and build another, and
 * left the hidden terminal running on xterm's DOM renderer, which is its
 * SLOWEST path. Hiding a terminal therefore made it more expensive, not less,
 * and the cost scaled with how much output the background terminals produced.
 *
 * Budgeting fixes both halves: contexts are only reclaimed when the cap is
 * actually under pressure, and the victim is the least recently used HIDDEN
 * terminal rather than whichever one the user just navigated away from.
 */

/**
 * How many WebGL contexts may be live at once.
 *
 * Deliberately well under the typical ~16 browser ceiling: the app is not the
 * only thing on the page that may want a context, and losing one to the
 * browser's own reclaim triggers the (more expensive) context-lost recovery.
 */
export const MAX_WEBGL_CONTEXTS = 8

interface WebglSlot {
  id: string
  visible: boolean
  release: () => void
}

/** MRU-ordered — index 0 is the most recently used. */
const slots: WebglSlot[] = []

function indexOfSlot(id: string): number {
  return slots.findIndex((slot) => slot.id === id)
}

/**
 * Claim or refresh the slot for `id`, evicting hidden slots if the budget is
 * over. Returns the ids evicted to make room; their `release` has already run.
 *
 * A visible terminal is never evicted — reclaiming the context of something
 * the user is looking at is exactly the churn this budget exists to stop. If
 * every slot is visible the budget is knowingly exceeded and the browser's own
 * context-loss handling takes over, which the caller already recovers from.
 */
export function claimWebglSlot(id: string, release: () => void): string[] {
  const existing = indexOfSlot(id)
  if (existing >= 0) {
    const [slot] = slots.splice(existing, 1)
    slot.visible = true
    slot.release = release
    slots.unshift(slot)
  } else {
    slots.unshift({ id, visible: true, release })
  }

  const evicted: string[] = []
  while (slots.length > MAX_WEBGL_CONTEXTS) {
    let victim = -1
    for (let i = slots.length - 1; i >= 0; i--) {
      if (!slots[i].visible) {
        victim = i
        break
      }
    }
    if (victim < 0) break

    const [slot] = slots.splice(victim, 1)
    evicted.push(slot.id)
    slot.release()
  }
  return evicted
}

/**
 * Mark `id` as no longer visible.
 *
 * Keeps its context — that is the point — but makes it the next candidate for
 * eviction once the budget comes under pressure. Moves it to the LRU end so
 * the terminal hidden longest is reclaimed first.
 */
export function releaseWebglVisibility(id: string): void {
  const index = indexOfSlot(id)
  if (index < 0) return
  const [slot] = slots.splice(index, 1)
  slot.visible = false
  slots.push(slot)
}

/** Forget `id` entirely, without releasing (the caller is disposing anyway). */
export function dropWebglSlot(id: string): void {
  const index = indexOfSlot(id)
  if (index >= 0) slots.splice(index, 1)
}

/** @internal Testing only — MRU-ordered slot ids. */
export function _webglSlotIdsForTesting(): string[] {
  return slots.map((slot) => slot.id)
}

/** @internal Testing only. */
export function _resetWebglBudgetForTesting(): void {
  slots.length = 0
}
