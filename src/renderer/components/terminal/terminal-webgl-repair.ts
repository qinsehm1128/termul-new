import type { Terminal } from '@xterm/xterm'
import { logFrontendError } from '@/lib/log-api'

/** Wait until the wheel/write burst ends before refreshing leftover WebGL rows. */
export const WEBGL_SCROLL_REPAIR_IDLE_MS = 120

/**
 * Upper bound on how long a sustained write stream can defer a repair. 2.5x the
 * idle window, so it only engages during a real burst; `repairNow` does a
 * full-viewport `_clearModel(true)` + `refresh`, so a tighter bound trades
 * residue for frame jitter.
 */
export const WEBGL_SCROLL_REPAIR_MAX_WAIT_MS = 300

type RenderServiceHandle = {
  clear?: () => void
}

type TerminalCoreHandle = {
  _renderService?: RenderServiceHandle
}

/**
 * Drop this terminal's WebGL cell model without touching the shared atlas.
 * Same local rebuild a window resize uses (`WebglRenderer.clear`), so
 * leftover glyphs do not wait for the user to drag the window.
 *
 * `isWebglActive` is required rather than optional on purpose. `RenderService`
 * forwards `clear()` to whichever renderer is attached, and `DomRenderer.clear`
 * calls `replaceChildren()` on the row container immediately while the repaint
 * waits for the next frame — a visible black flash, held up to a second when
 * DEC 2026 synchronized output is on. Only the WebGL renderer rebuilds its
 * model cheaply, so the caller has to state which one it is looking at.
 */
export function clearWebglRenderModel(terminal: object, isWebglActive: boolean): void {
  if (!isWebglActive) return
  const renderService = (terminal as { _core?: TerminalCoreHandle })._core?._renderService
  renderService?.clear?.()
}

export interface WebglScrollRepair {
  /** Atlas pages changed — refresh after the current burst, never clear the atlas. */
  markAtlasDirty: () => void
  noteAtlasMerged: () => void
  onScroll: () => void
  /**
   * PTY/ZLE in-place redraws update the cell buffer but often leave the
   * previous WebGL row. Same idle refresh as scroll.
   */
  onWrite: () => void
  repairNow: () => void
  dispose: () => void
}

/**
 * Rebuild this terminal's WebGL model after scroll, writes, or atlas merges.
 *
 * Do not call `clearTextureAtlas`. xterm's WebGL addon shares one atlas
 * across matching terminals; clearing it leaves sibling render models with
 * stale UVs and leftover glyphs. Hide/show already disposes and recreates
 * the addon, which builds a fresh renderer model.
 */
export function createWebglScrollRepair(args: {
  getTerminal: () => Pick<Terminal, 'refresh' | 'rows'> | null
  rebuildSurface?: (terminal: Pick<Terminal, 'refresh' | 'rows'>) => void
  idleMs?: number
  maxWaitMs?: number
}): WebglScrollRepair {
  let idleTimer: ReturnType<typeof setTimeout> | null = null
  let maxWaitTimer: ReturnType<typeof setTimeout> | null = null
  let trailingPending = false
  const idleMs = args.idleMs ?? WEBGL_SCROLL_REPAIR_IDLE_MS
  const maxWaitMs = args.maxWaitMs ?? WEBGL_SCROLL_REPAIR_MAX_WAIT_MS

  const reportFailure = (action: string, error: unknown): void => {
    const message = error instanceof Error ? error.message : String(error)
    void logFrontendError({
      level: 'warn',
      source: 'ConnectedTerminal:webgl-scroll-repair',
      message: `${action} failed: ${message}`,
      stack: error instanceof Error ? error.stack : undefined
    })
  }

  const repairNow = (): void => {
    const terminal = args.getTerminal()
    if (!terminal) return
    if (args.rebuildSurface) {
      try {
        args.rebuildSurface(terminal)
      } catch (error) {
        reportFailure('rebuild', error)
      }
    }
    try {
      terminal.refresh(0, Math.max(0, terminal.rows - 1))
    } catch (error) {
      reportFailure('refresh', error)
    }
  }

  const clearTimers = (): void => {
    if (idleTimer !== null) {
      clearTimeout(idleTimer)
      idleTimer = null
    }
    if (maxWaitTimer !== null) {
      clearTimeout(maxWaitTimer)
      maxWaitTimer = null
    }
  }

  const flush = (): void => {
    clearTimers()
    trailingPending = false
    repairNow()
  }

  const onIdle = (): void => {
    idleTimer = null
    if (trailingPending) {
      flush()
      return
    }
    // The burst ended quietly. Close it so the next event is a leading edge
    // again instead of being coalesced into a burst nobody is feeding.
    clearTimers()
  }

  const onMaxWait = (): void => {
    maxWaitTimer = null
    if (!trailingPending) return
    flush()
    // The stream is still live, so re-open the burst from this repair: the next
    // write is a continuation, not a fresh leading edge that would repair twice
    // in a row, and the next bound is measured from the repair that just ran.
    idleTimer = setTimeout(onIdle, idleMs)
    maxWaitTimer = setTimeout(onMaxWait, maxWaitMs)
  }

  /**
   * Leading edge plus a bounded maximum wait.
   *
   * A pure trailing debounce is starved by sustained PTY output — the idle
   * window never arrives while `onWrite` fires per chunk — which is exactly the
   * continuous-typing case. The first event of a burst repairs synchronously,
   * and both bounds are armed from that repair, so no two repairs in a live
   * stream are further apart than `maxWaitMs`. Later events only coalesce, so N
   * events never produce N repairs.
   * See .workflow/sessions/20260824-ralph-termul-leftover-glyphs/dod-amendment-01.md
   */
  const scheduleIdleRepair = (): void => {
    if (idleTimer === null && maxWaitTimer === null && !trailingPending) {
      repairNow()
      idleTimer = setTimeout(onIdle, idleMs)
      maxWaitTimer = setTimeout(onMaxWait, maxWaitMs)
      return
    }
    trailingPending = true
    if (idleTimer !== null) clearTimeout(idleTimer)
    idleTimer = setTimeout(onIdle, idleMs)
  }

  /**
   * Trailing only — for scroll.
   *
   * The leading edge above exists for one reason: a continuous `onWrite` stream
   * never lets the idle window arrive, so a pure trailing debounce is starved.
   * The amended definition-of-done states that requirement for `onWrite` and
   * only `onWrite`. A scroll burst terminates on its own, so it cannot starve
   * anything, and it inherited the leading edge and the max-wait re-arm for no
   * reason.
   *
   * That inheritance is expensive: `repairNow` is a full-viewport
   * `_clearModel(true)` plus a whole-screen `refresh`, and discarding the model
   * forces every cell to be re-uploaded on the next frame. Wired to scroll it
   * ran synchronously on every wheel flick and again every `maxWaitMs` while
   * scrolling continued — worst case on the heavily styled output an agent CLI
   * produces. Residue only has to be gone once the view settles, never mid-flick.
   */
  const scheduleTrailingRepair = (): void => {
    trailingPending = true
    if (idleTimer !== null) clearTimeout(idleTimer)
    idleTimer = setTimeout(onIdle, idleMs)
    // Deliberately does not arm `maxWaitTimer`: a mid-burst flush is exactly
    // the full-viewport cost being removed. A write burst that armed it keeps
    // it — writes still get their bounded wait.
  }

  return {
    markAtlasDirty(): void {
      scheduleIdleRepair()
    },
    noteAtlasMerged(): void {
      scheduleIdleRepair()
    },
    onScroll(): void {
      scheduleTrailingRepair()
    },
    onWrite(): void {
      scheduleIdleRepair()
    },
    repairNow,
    dispose(): void {
      clearTimers()
      trailingPending = false
    }
  }
}

/**
 * After a hidden terminal is shown again, PTY bytes may have updated the
 * buffer while WebGL was disposed. Drop leftover pixel-scroll translate and
 * refresh; the addon is recreated on show and already has a new model.
 */
export function restoreVisibleTerminalSurface(args: {
  resetPixelOffset?: () => void
  repair?: Pick<WebglScrollRepair, 'repairNow'> | null
  terminal?: Pick<Terminal, 'refresh' | 'rows'> | null
}): void {
  args.resetPixelOffset?.()
  if (args.repair) {
    args.repair.repairNow()
    return
  }
  const terminal = args.terminal
  if (!terminal) return
  try {
    terminal.refresh(0, Math.max(0, terminal.rows - 1))
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    void logFrontendError({
      level: 'warn',
      source: 'ConnectedTerminal:webgl-scroll-repair',
      message: `refresh failed: ${message}`,
      stack: error instanceof Error ? error.stack : undefined
    })
  }
}
