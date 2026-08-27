import { afterEach, describe, expect, it, vi } from 'vitest'
import { logFrontendError } from '@/lib/log-api'
import {
  clearWebglRenderModel,
  createWebglScrollRepair,
  restoreVisibleTerminalSurface,
  WEBGL_SCROLL_REPAIR_IDLE_MS,
  WEBGL_SCROLL_REPAIR_MAX_WAIT_MS
} from './terminal-webgl-repair'

vi.mock('@/lib/log-api', () => ({
  logFrontendError: vi.fn()
}))

describe('createWebglScrollRepair', () => {
  afterEach(() => {
    vi.useRealTimers()
    vi.clearAllMocks()
  })

  // dod-amendment-01.md mandates the leading edge for `onWrite` and only
  // `onWrite` — a scroll burst ends on its own and cannot starve the idle
  // window. `repairNow` is a full-viewport _clearModel + refresh, so a leading
  // edge on scroll cost one of those per wheel flick.
  it('coalesces a scroll burst into a single trailing repair', async () => {
    vi.useFakeTimers()
    const refresh = vi.fn()
    const repair = createWebglScrollRepair({
      getTerminal: () => ({ refresh, rows: 24 })
    })

    repair.onScroll()
    repair.onScroll()
    expect(refresh).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(WEBGL_SCROLL_REPAIR_IDLE_MS)

    expect(refresh).toHaveBeenCalledTimes(1)
    expect(refresh).toHaveBeenCalledWith(0, 23)
    repair.dispose()
  })

  it('does not repair mid-scroll while the burst is still going', async () => {
    vi.useFakeTimers()
    const refresh = vi.fn()
    const repair = createWebglScrollRepair({
      getTerminal: () => ({ refresh, rows: 24 })
    })

    // Keep the burst alive well past the write path's bounded max wait.
    for (let elapsed = 0; elapsed < WEBGL_SCROLL_REPAIR_MAX_WAIT_MS * 3; elapsed += 40) {
      repair.onScroll()
      await vi.advanceTimersByTimeAsync(40)
    }

    // Sustained scrolling must not re-arm the max wait; that cadence is what
    // made scrolling an agent conversation feel slow.
    expect(refresh).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(WEBGL_SCROLL_REPAIR_IDLE_MS)
    expect(refresh).toHaveBeenCalledTimes(1)
    repair.dispose()
  })

  it('repairs on the leading edge of the first write in a burst', () => {
    vi.useFakeTimers()
    const refresh = vi.fn()
    const rebuildSurface = vi.fn()
    const repair = createWebglScrollRepair({
      getTerminal: () => ({ refresh, rows: 24 }),
      rebuildSurface
    })

    repair.onWrite()

    // No timer advance: the first event of a burst must not wait.
    expect(rebuildSurface).toHaveBeenCalledTimes(1)
    expect(refresh).toHaveBeenCalledTimes(1)
    expect(refresh).toHaveBeenCalledWith(0, 23)
    repair.dispose()
  })

  it('repairs within the bounded max wait under a continuous onWrite stream', async () => {
    vi.useFakeTimers()
    const at: number[] = []
    const refresh = vi.fn(() => {
      at.push(Date.now())
    })
    const repair = createWebglScrollRepair({
      getTerminal: () => ({ refresh, rows: 24 })
    })

    // 20ms cadence is far tighter than the 120ms idle window, so a trailing-only
    // debounce is starved and never fires at all.
    const streamMs = 1200
    const stepMs = 20
    for (let elapsed = 0; elapsed < streamMs; elapsed += stepMs) {
      repair.onWrite()
      await vi.advanceTimersByTimeAsync(stepMs)
    }

    // 0 repairs = starved trailing debounce; 1 = leading edge with no max wait.
    expect(at.length).toBeGreaterThan(1)
    for (let i = 1; i < at.length; i += 1) {
      expect(at[i] - at[i - 1]).toBeLessThanOrEqual(WEBGL_SCROLL_REPAIR_MAX_WAIT_MS)
    }
    // Coalescing: 60 onWrite calls must not produce 60 forced remodels.
    expect(at.length).toBeLessThanOrEqual(8)
    repair.dispose()
  })

  it('refreshes after atlas-dirty idle without clearing a shared atlas', async () => {
    vi.useFakeTimers()
    const refresh = vi.fn()
    const repair = createWebglScrollRepair({
      getTerminal: () => ({ refresh, rows: 24 })
    })

    repair.markAtlasDirty()
    await vi.advanceTimersByTimeAsync(WEBGL_SCROLL_REPAIR_IDLE_MS)

    expect(refresh).toHaveBeenCalledWith(0, 23)

    repair.onScroll()
    await vi.advanceTimersByTimeAsync(WEBGL_SCROLL_REPAIR_IDLE_MS)
    expect(refresh).toHaveBeenCalledTimes(2)
    repair.dispose()
  })

  // dod-amendment-01.md: an atlas merge is the leading edge of its own burst,
  // so it repairs synchronously instead of waiting for the gesture to go idle.
  it('repairs on the leading edge of an atlas merge', () => {
    vi.useFakeTimers()
    const refresh = vi.fn()
    const repair = createWebglScrollRepair({
      getTerminal: () => ({ refresh, rows: 12 })
    })

    repair.noteAtlasMerged()

    expect(refresh).toHaveBeenCalledWith(0, 11)
    repair.dispose()
  })

  it('logs and continues when refresh throws', () => {
    const repair = createWebglScrollRepair({
      getTerminal: () => ({
        rows: 8,
        refresh: () => {
          throw new Error('renderer disposed')
        }
      })
    })

    repair.repairNow()

    expect(logFrontendError).toHaveBeenCalledWith(
      expect.objectContaining({
        level: 'warn',
        source: 'ConnectedTerminal:webgl-scroll-repair',
        message: 'refresh failed: renderer disposed'
      })
    )
    repair.dispose()
  })

  // dod-amendment-01.md: the leading edge already repaired once, so dispose is
  // asserted against the pending trailing AND max-wait timers instead.
  it('cancels the pending trailing and max-wait repairs on dispose', async () => {
    vi.useFakeTimers()
    const refresh = vi.fn()
    const repair = createWebglScrollRepair({
      getTerminal: () => ({ refresh, rows: 24 })
    })

    // Writes, not scrolls: the leading edge and the max-wait timer both live on
    // the write path now, which is what this test is actually about.
    repair.onWrite()
    repair.onWrite()
    expect(refresh).toHaveBeenCalledTimes(1)

    repair.dispose()
    await vi.advanceTimersByTimeAsync(WEBGL_SCROLL_REPAIR_MAX_WAIT_MS * 2)

    expect(refresh).toHaveBeenCalledTimes(1)
  })

  // dod-amendment-01.md: PTY writes are what starve a trailing debounce, so the
  // model rebuild now happens on the burst's leading edge.
  it('rebuilds the local WebGL model on the leading edge of a PTY write burst', () => {
    vi.useFakeTimers()
    const refresh = vi.fn()
    const rebuildSurface = vi.fn()
    const repair = createWebglScrollRepair({
      getTerminal: () => ({ refresh, rows: 24 }),
      rebuildSurface
    })

    repair.onWrite()

    expect(rebuildSurface).toHaveBeenCalledTimes(1)
    expect(refresh).toHaveBeenCalledTimes(1)
    expect(refresh).toHaveBeenCalledWith(0, 23)
    repair.dispose()
  })

  it('logs and continues when rebuild throws', () => {
    const refresh = vi.fn()
    const repair = createWebglScrollRepair({
      getTerminal: () => ({ refresh, rows: 8 }),
      rebuildSurface: () => {
        throw new Error('renderer disposed')
      }
    })

    repair.repairNow()

    expect(logFrontendError).toHaveBeenCalledWith(
      expect.objectContaining({
        level: 'warn',
        source: 'ConnectedTerminal:webgl-scroll-repair',
        message: 'rebuild failed: renderer disposed'
      })
    )
    expect(refresh).toHaveBeenCalledWith(0, 7)
    repair.dispose()
  })
})

describe('clearWebglRenderModel', () => {
  it('clears this renderer model and never touches a shared atlas', () => {
    const clear = vi.fn()
    const clearTextureAtlas = vi.fn()

    clearWebglRenderModel(
      {
        _core: { _renderService: { clear, clearTextureAtlas } }
      },
      true
    )

    expect(clear).toHaveBeenCalledTimes(1)
    expect(clearTextureAtlas).not.toHaveBeenCalled()
  })

  it('leaves the DOM renderer alone', () => {
    // RenderService.clear() forwards to whatever renderer is attached, and
    // DomRenderer.clear() calls replaceChildren() straight away while the
    // repaint waits for the next frame. Under the DOM renderer this function
    // is not a cheap model rebuild, it is a black flash — up to a second of
    // one while DEC 2026 synchronized output holds the repaint back.
    const clear = vi.fn()

    clearWebglRenderModel({ _core: { _renderService: { clear } } }, false)

    expect(clear).not.toHaveBeenCalled()
  })

  it('no-ops when the renderer is not attached', () => {
    expect(() => clearWebglRenderModel({}, true)).not.toThrow()
  })
})

describe('restoreVisibleTerminalSurface', () => {
  afterEach(() => {
    vi.clearAllMocks()
  })

  it('resets the pixel offset and refreshes after a hide/show cycle', () => {
    const resetPixelOffset = vi.fn()
    const repairNow = vi.fn()
    const refresh = vi.fn()

    restoreVisibleTerminalSurface({
      resetPixelOffset,
      repair: { repairNow },
      terminal: { refresh, rows: 24 }
    })

    expect(resetPixelOffset).toHaveBeenCalledTimes(1)
    expect(repairNow).toHaveBeenCalledTimes(1)
    expect(refresh).not.toHaveBeenCalled()
  })

  it('falls back to a full refresh when WebGL repair is unavailable', () => {
    const refresh = vi.fn()
    restoreVisibleTerminalSurface({
      resetPixelOffset: vi.fn(),
      repair: null,
      terminal: { refresh, rows: 12 }
    })
    expect(refresh).toHaveBeenCalledWith(0, 11)
  })
})
