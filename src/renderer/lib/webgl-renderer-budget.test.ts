import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  _resetWebglBudgetForTesting,
  _webglSlotIdsForTesting,
  claimWebglSlot,
  dropWebglSlot,
  MAX_WEBGL_CONTEXTS,
  releaseWebglVisibility
} from './webgl-renderer-budget'

describe('webgl renderer budget', () => {
  beforeEach(() => {
    _resetWebglBudgetForTesting()
  })

  it('evicts nothing while under the budget', () => {
    const releases = new Map<string, ReturnType<typeof vi.fn>>()
    for (let i = 0; i < MAX_WEBGL_CONTEXTS; i++) {
      const release = vi.fn()
      releases.set(`t${i}`, release)
      expect(claimWebglSlot(`t${i}`, release)).toEqual([])
    }
    for (const release of releases.values()) {
      expect(release).not.toHaveBeenCalled()
    }
    expect(_webglSlotIdsForTesting()).toHaveLength(MAX_WEBGL_CONTEXTS)
  })

  it('evicts the least recently used HIDDEN slot when the budget is exceeded', () => {
    const releases: Record<string, ReturnType<typeof vi.fn>> = {}
    for (let i = 0; i < MAX_WEBGL_CONTEXTS; i++) {
      releases[`t${i}`] = vi.fn()
      claimWebglSlot(`t${i}`, releases[`t${i}`])
    }

    // t0 was claimed first and is hidden longest; t1 is hidden more recently.
    releaseWebglVisibility('t1')
    releaseWebglVisibility('t0')
    // `releaseWebglVisibility` pushes to the LRU end, so t0 is now last.

    const newRelease = vi.fn()
    const evicted = claimWebglSlot('t-new', newRelease)

    expect(evicted).toEqual(['t0'])
    expect(releases.t0).toHaveBeenCalledTimes(1)
    expect(releases.t1).not.toHaveBeenCalled()
    expect(newRelease).not.toHaveBeenCalled()
    expect(_webglSlotIdsForTesting()).not.toContain('t0')
  })

  it('never evicts a visible slot, even when that means exceeding the budget', () => {
    const releases: ReturnType<typeof vi.fn>[] = []
    for (let i = 0; i <= MAX_WEBGL_CONTEXTS; i++) {
      const release = vi.fn()
      releases.push(release)
      claimWebglSlot(`t${i}`, release)
    }

    // Reclaiming the context of something the user is looking at is exactly
    // the churn this budget exists to stop. Going over is the lesser evil —
    // the caller already recovers from browser-driven context loss.
    for (const release of releases) {
      expect(release).not.toHaveBeenCalled()
    }
    expect(_webglSlotIdsForTesting()).toHaveLength(MAX_WEBGL_CONTEXTS + 1)
  })

  it('re-claiming refreshes recency and marks the slot visible again', () => {
    claimWebglSlot('a', vi.fn())
    claimWebglSlot('b', vi.fn())
    releaseWebglVisibility('a')
    expect(_webglSlotIdsForTesting()).toEqual(['b', 'a'])

    claimWebglSlot('a', vi.fn())
    expect(_webglSlotIdsForTesting()).toEqual(['a', 'b'])

    // Now visible again, so it must survive budget pressure.
    const releases: ReturnType<typeof vi.fn>[] = []
    for (let i = 0; i < MAX_WEBGL_CONTEXTS; i++) {
      const release = vi.fn()
      releases.push(release)
      claimWebglSlot(`filler${i}`, release)
    }
    expect(_webglSlotIdsForTesting()).toContain('a')
  })

  it('does not re-register a slot on claim, so a terminal never double-counts', () => {
    claimWebglSlot('a', vi.fn())
    claimWebglSlot('a', vi.fn())
    claimWebglSlot('a', vi.fn())
    expect(_webglSlotIdsForTesting()).toEqual(['a'])
  })

  it('drops a slot without releasing it (the caller is already disposing)', () => {
    const release = vi.fn()
    claimWebglSlot('a', release)

    dropWebglSlot('a')

    expect(release).not.toHaveBeenCalled()
    expect(_webglSlotIdsForTesting()).toEqual([])
  })

  it('uses the latest release callback when a slot is re-claimed', () => {
    const stale = vi.fn()
    const fresh = vi.fn()
    claimWebglSlot('a', stale)
    claimWebglSlot('a', fresh)
    releaseWebglVisibility('a')

    for (let i = 0; i < MAX_WEBGL_CONTEXTS; i++) {
      claimWebglSlot(`filler${i}`, vi.fn())
    }

    expect(fresh).toHaveBeenCalledTimes(1)
    expect(stale).not.toHaveBeenCalled()
  })

  it('ignores visibility release and drop for unknown ids', () => {
    expect(() => releaseWebglVisibility('nope')).not.toThrow()
    expect(() => dropWebglSlot('nope')).not.toThrow()
    expect(_webglSlotIdsForTesting()).toEqual([])
  })
})
