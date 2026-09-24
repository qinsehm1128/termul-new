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

type WebglControlState = 'none' | 'escape' | 'csi'

export interface WebglModelRebuildDetector {
  scan: (data: string | Uint8Array) => boolean
  flush: () => boolean
  reset: () => void
}

/**
 * Return whether a PTY chunk can redraw existing cells rather than only append
 * rows. The detector retains the tiny amount of ANSI state needed when PTY
 * read boundaries split an ESC/CSI sequence across chunks.
 */
export function createWebglModelRebuildDetector(): WebglModelRebuildDetector {
  let state: WebglControlState = 'none'
  let pendingCarriageReturn = false

  return {
    scan(data: string | Uint8Array): boolean {
      let rebuild = false
      for (let index = 0; index < data.length; index += 1) {
        const byte = typeof data === 'string' ? data.charCodeAt(index) : data[index]
        const next = typeof data === 'string' ? data.charCodeAt(index + 1) : data[index + 1]

        if (pendingCarriageReturn) {
          pendingCarriageReturn = false
          if (byte !== 0x0a) rebuild = true
        }
        if (byte === 0x08) rebuild = true
        if (byte === 0x0d) {
          if (index + 1 < data.length) {
            if (next !== 0x0a) rebuild = true
          } else {
            pendingCarriageReturn = true
          }
        }

        if (state === 'escape') {
          if (byte === 0x5b) {
            state = 'csi'
            continue
          }
          state = byte === 0x1b ? 'escape' : 'none'
          // Non-CSI ESC controls are uncommon but may redraw the screen; be
          // conservative rather than allowing a stale model through.
          rebuild = true
          continue
        }

        if (state === 'csi') {
          if (byte >= 0x40 && byte <= 0x7e) {
            state = 'none'
            // SGR/style changes do not invalidate existing cell positions.
            if (byte !== 0x6d) rebuild = true
          }
          continue
        }

        if (byte === 0x1b) state = 'escape'
      }
      return rebuild
    },
    flush(): boolean {
      const rebuild = pendingCarriageReturn
      pendingCarriageReturn = false
      return rebuild
    },
    reset(): void {
      state = 'none'
      pendingCarriageReturn = false
    }
  }
}

/** Stateless convenience for complete chunks and unit callers. */
export function requiresWebglModelRebuild(data: string | Uint8Array): boolean {
  const detector = createWebglModelRebuildDetector()
  return detector.scan(data) || detector.flush()
}

export interface WebglScrollRepair {
  /** Atlas pages changed — refresh after the current burst, never clear the atlas. */
  markAtlasDirty: () => void
  noteAtlasMerged: () => void
  onScroll: () => void
  /**
   * PTY/ZLE in-place redraws update the cell buffer but often leave the
   * previous WebGL row. Append-only output only needs a refresh; callers pass
   * true when the chunk can invalidate the WebGL cell model.
   */
  onWrite: (requiresModelRebuild?: boolean) => void
  repairNow: (requiresModelRebuild?: boolean) => void
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
  let rebuildPending = false
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

  const repairNow = (requiresModelRebuild = true): void => {
    const terminal = args.getTerminal()
    if (!terminal) return
    if (requiresModelRebuild && args.rebuildSurface) {
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
    const requiresModelRebuild = rebuildPending
    rebuildPending = false
    repairNow(requiresModelRebuild)
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
  const scheduleIdleRepair = (requiresModelRebuild: boolean): void => {
    if (requiresModelRebuild) rebuildPending = true
    if (idleTimer === null && maxWaitTimer === null && !trailingPending) {
      const rebuild = rebuildPending
      rebuildPending = false
      repairNow(rebuild)
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
    rebuildPending = true
    if (idleTimer !== null) clearTimeout(idleTimer)
    idleTimer = setTimeout(onIdle, idleMs)
    // Deliberately does not arm `maxWaitTimer`: a mid-burst flush is exactly
    // the full-viewport cost being removed. A write burst that armed it keeps
    // it — writes still get their bounded wait.
  }

  return {
    markAtlasDirty(): void {
      scheduleIdleRepair(true)
    },
    noteAtlasMerged(): void {
      scheduleIdleRepair(true)
    },
    onScroll(): void {
      scheduleTrailingRepair()
    },
    onWrite(requiresModelRebuild = true): void {
      scheduleIdleRepair(requiresModelRebuild)
    },
    repairNow,
    dispose(): void {
      clearTimers()
      trailingPending = false
      rebuildPending = false
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
