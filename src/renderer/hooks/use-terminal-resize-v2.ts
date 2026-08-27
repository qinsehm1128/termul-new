import type { FitAddon } from '@xterm/addon-fit'
import type { Terminal } from '@xterm/xterm'
import { useCallback, useEffect, useRef } from 'react'

/**
 * Fit debounce + immediate PTY resize.
 *
 * Stage 1 (8ms): Debounces fitAddon.fit() so drag does not call fit() on
 * every ResizeObserver tick.
 *
 * After a successful fit the PTY is resized immediately. Same-size ioctl
 * suppression lives on the host so a retry after attach / phone-release
 * cannot be skipped just because the renderer already requested that grid.
 *
 * Hidden terminals skip observer work, and scroll position is preserved
 * across fit() calls.
 */

/** Debounce for fit() — keeps UI responsive during drag */
const FIT_DEBOUNCE_MS = 8

export interface UseTerminalResizeV2Options {
  /** Called with new cols/rows after a confirmed dimension change */
  onPtyResize: (cols: number, rows: number) => void
  /** Ref to the xterm.js Terminal instance (updated lazily) */
  terminalRef: React.RefObject<Terminal | null>
  /** Ref to the FitAddon instance (updated lazily) */
  fitAddonRef: React.RefObject<FitAddon | null>
  /** The container element to observe */
  containerRef: React.RefObject<HTMLDivElement | null>
  /** Whether the terminal is currently visible (skips processing when hidden) */
  isVisible?: boolean
}

export interface UseTerminalResizeV2Return {
  /** Force an immediate fit + PTY resize (used after visibility change, init, etc.) */
  forceFit: () => void
}

export function useTerminalResizeV2(
  options: UseTerminalResizeV2Options
): UseTerminalResizeV2Return {
  const { onPtyResize, terminalRef, fitAddonRef, containerRef, isVisible = true } = options

  // Refs for timer IDs — must be refs to avoid stale closures in ResizeObserver
  const fitTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  // Track last known dimensions to detect skips
  const lastContainerWidthRef = useRef<number>(0)
  const lastContainerHeightRef = useRef<number>(0)
  const lastColsRef = useRef<number>(0)
  const lastRowsRef = useRef<number>(0)

  // Track visibility to suppress resize processing when hidden
  const isVisibleRef = useRef(isVisible)
  isVisibleRef.current = isVisible

  // Stable callback refs
  const onPtyResizeRef = useRef(onPtyResize)
  onPtyResizeRef.current = onPtyResize

  /**
   * Perform fit() and conditionally schedule PTY resize.
   * Returns true if fit was actually performed (dimensions changed).
   */
  const performFit = useCallback(
    (force = false): boolean => {
      const fitAddon = fitAddonRef.current
      const terminal = terminalRef.current
      const container = containerRef.current
      if (!fitAddon || !terminal || !container) return false

      const rect = container.getBoundingClientRect()
      const width = Math.round(rect.width)
      const height = Math.round(rect.height)

      // Guard against fitting to a collapsed container. After a Windows
      // minimize→restore the webview reflows over several frames; during that
      // window getBoundingClientRect() can report a tiny height. Calling fit()
      // then collapses the terminal grid to 1-2 rows (the PTY redraws tiny,
      // showing "1-2 lines" until a later fit corrects it). Never fit unless
      // the container is large enough to hold a usable grid — this protects
      // every caller (ResizeObserver, forceFit, recovery).
      const MIN_FIT_WIDTH = 40
      const MIN_FIT_HEIGHT = 40
      if (width < MIN_FIT_WIDTH || height < MIN_FIT_HEIGHT) {
        return false
      }

      // Skip if dimensions haven't changed (and not forced)
      if (
        !force &&
        width === lastContainerWidthRef.current &&
        height === lastContainerHeightRef.current
      ) {
        return false
      }

      // Save scroll position before fit to preserve it across the v6 viewport rewrite.
      const buffer = terminal.buffer?.active
      const scrollTop = buffer?.viewportY ?? 0
      const baseY = buffer?.baseY ?? 0

      try {
        fitAddon.fit()
      } catch {
        // fit() can throw if terminal is not ready
        return false
      }

      // Update tracked dimensions after successful fit
      if (width > 0 && height > 0) {
        lastContainerWidthRef.current = width
        lastContainerHeightRef.current = height
      }

      // Restore scroll position if user was scrolled up
      if (scrollTop > 0 && scrollTop < baseY) {
        terminal.scrollToLine(scrollTop)
      }

      return true
    },
    [fitAddonRef, terminalRef, containerRef]
  )

  const performFitRef = useRef(performFit)
  performFitRef.current = performFit

  const pushPtyResize = useCallback((cols: number, rows: number, force: boolean): void => {
    if (!force && cols === lastColsRef.current && rows === lastRowsRef.current) {
      return
    }
    lastColsRef.current = cols
    lastRowsRef.current = rows
    onPtyResizeRef.current(cols, rows)
  }, [])

  /**
   * Force an immediate fit + PTY resize, bypassing the fit debounce.
   * Used when the terminal becomes visible after being hidden, or on init.
   * Always forwards the fitted grid so a previously ignored host resize
   * (phone-fit, attach race) can be retried. The PTY owner no-ops same-size.
   */
  const forceFit = useCallback((): void => {
    const didFit = performFitRef.current(true)
    const terminal = terminalRef.current
    if (didFit && terminal) {
      pushPtyResize(terminal.cols, terminal.rows, true)
    }
  }, [pushPtyResize, terminalRef])

  // Set up ResizeObserver for the fit debounce + immediate PTY resize
  useEffect(() => {
    const container = containerRef.current
    if (!container) return

    const handleResize = (): void => {
      // Skip resize processing when terminal is hidden
      if (!isVisibleRef.current) return

      if (fitTimerRef.current) {
        clearTimeout(fitTimerRef.current)
      }

      fitTimerRef.current = setTimeout(() => {
        fitTimerRef.current = null

        const didFit = performFitRef.current(false)
        if (!didFit) return

        const term = terminalRef.current
        if (!term) return

        pushPtyResize(term.cols, term.rows, false)
      }, FIT_DEBOUNCE_MS)
    }

    const observer = new ResizeObserver(handleResize)
    observer.observe(container)

    return () => {
      observer.disconnect()
      if (fitTimerRef.current) {
        clearTimeout(fitTimerRef.current)
        fitTimerRef.current = null
      }
    }
  }, [containerRef, pushPtyResize, terminalRef])

  return { forceFit }
}
