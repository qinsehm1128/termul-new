import { useCallback, useEffect, useRef, useState } from 'react'

/** Hold this long before a touch turns into a drag rather than a tap or a scroll. */
export const LONG_PRESS_MS = 450

/** Finger travel that cancels the pending long press, treating it as a scroll. */
export const LONG_PRESS_SLOP_PX = 10

export interface LongPressDropTarget {
  kind: 'entry' | 'project'
  path: string
}

/**
 * Where a finger lifted, expressed as a drop target.
 *
 * Touch drags cannot use HTML5 drag-and-drop — it never fires for touch
 * pointers — so the row under the finger has to be found by hit-testing
 * instead of by the browser dispatching `dragover` at it. Only directories and
 * project rows are destinations; a file row resolves to nothing rather than to
 * its parent, so a mis-aimed drop does nothing instead of something surprising.
 */
export function resolveLongPressDropTarget(element: Element | null): LongPressDropTarget | null {
  if (!element) return null

  const projectRow = element.closest('[data-project-path]')
  const projectPath = projectRow?.getAttribute('data-project-path')
  if (projectPath) return { kind: 'project', path: projectPath }

  const entryRow = element.closest('[data-path]')
  if (!entryRow) return null
  if (entryRow.getAttribute('data-entry-type') !== 'directory') return null
  const path = entryRow.getAttribute('data-path')
  return path ? { kind: 'entry', path } : null
}

interface LongPressDragOptions {
  /** Paths this row would drag. Evaluated when the press completes, not on mount. */
  getDragPaths: () => string[]
  onDragStart: (paths: string[]) => void
  onDrop: (paths: string[], target: LongPressDropTarget) => void
  onCancel: () => void
}

export interface LongPressDragHandlers {
  onPointerDown: (event: React.PointerEvent) => void
  onPointerMove: (event: React.PointerEvent) => void
  onPointerUp: (event: React.PointerEvent) => void
  onPointerCancel: () => void
}

export interface LongPressDragState {
  isDragging: boolean
  hoverTarget: LongPressDropTarget | null
  handlers: LongPressDragHandlers
}

/**
 * Long-press-then-drag for touch surfaces.
 *
 * Mouse and pen keep the native HTML5 drag; only `pointerType === 'touch'` is
 * intercepted, because a mouse already has press-and-drag and hijacking it
 * would break text selection and the existing drag-to-pane payload.
 */
export function useTreeLongPressDrag(options: LongPressDragOptions): LongPressDragState {
  const { getDragPaths, onDragStart, onDrop, onCancel } = options

  const timerRef = useRef<number | null>(null)
  const originRef = useRef<{ x: number; y: number } | null>(null)
  const pathsRef = useRef<string[]>([])
  const [isDragging, setIsDragging] = useState(false)
  const [hoverTarget, setHoverTarget] = useState<LongPressDropTarget | null>(null)

  const clearTimer = useCallback((): void => {
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current)
      timerRef.current = null
    }
  }, [])

  // A press that is still pending when the row unmounts (a refresh landing
  // mid-gesture) would otherwise fire into a dead component.
  useEffect(() => clearTimer, [clearTimer])

  const reset = useCallback((): void => {
    clearTimer()
    originRef.current = null
    pathsRef.current = []
    setIsDragging(false)
    setHoverTarget(null)
  }, [clearTimer])

  const onPointerDown = useCallback(
    (event: React.PointerEvent): void => {
      if (event.pointerType !== 'touch') return
      originRef.current = { x: event.clientX, y: event.clientY }
      clearTimer()
      timerRef.current = window.setTimeout(() => {
        timerRef.current = null
        const paths = getDragPaths()
        if (paths.length === 0) return
        pathsRef.current = paths
        setIsDragging(true)
        onDragStart(paths)
      }, LONG_PRESS_MS)
    },
    [clearTimer, getDragPaths, onDragStart]
  )

  const onPointerMove = useCallback(
    (event: React.PointerEvent): void => {
      if (event.pointerType !== 'touch') return

      if (!isDragging) {
        // Still waiting on the hold: treat travel as a scroll, not a drag.
        const origin = originRef.current
        if (!origin) return
        const travelled = Math.abs(event.clientX - origin.x) + Math.abs(event.clientY - origin.y)
        if (travelled > LONG_PRESS_SLOP_PX) {
          clearTimer()
          originRef.current = null
        }
        return
      }

      setHoverTarget(
        resolveLongPressDropTarget(document.elementFromPoint(event.clientX, event.clientY))
      )
    },
    [clearTimer, isDragging]
  )

  const onPointerUp = useCallback(
    (event: React.PointerEvent): void => {
      if (event.pointerType !== 'touch') return
      if (!isDragging) {
        reset()
        return
      }

      const target = resolveLongPressDropTarget(
        document.elementFromPoint(event.clientX, event.clientY)
      )
      const paths = pathsRef.current
      reset()
      if (target && paths.length > 0) {
        onDrop(paths, target)
      } else {
        onCancel()
      }
    },
    [isDragging, onCancel, onDrop, reset]
  )

  const onPointerCancel = useCallback((): void => {
    const wasDragging = isDragging
    reset()
    if (wasDragging) onCancel()
  }, [isDragging, onCancel, reset])

  return {
    isDragging,
    hoverTarget,
    handlers: { onPointerDown, onPointerMove, onPointerUp, onPointerCancel }
  }
}
