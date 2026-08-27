import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  LONG_PRESS_MS,
  LONG_PRESS_SLOP_PX,
  resolveLongPressDropTarget,
  useTreeLongPressDrag
} from './use-tree-long-press-drag'

function row(attributes: Record<string, string>): HTMLElement {
  const node = document.createElement('div')
  for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, value)
  document.body.appendChild(node)
  return node
}

describe('resolveLongPressDropTarget', () => {
  afterEach(() => {
    document.body.innerHTML = ''
  })

  it('should resolve a directory row to an entry target', () => {
    const node = row({ 'data-path': '/project/src', 'data-entry-type': 'directory' })
    expect(resolveLongPressDropTarget(node)).toEqual({ kind: 'entry', path: '/project/src' })
  })

  it('should resolve nothing for a file row', () => {
    // Silently retargeting to the parent directory would move the entry
    // somewhere the user never aimed at.
    const node = row({ 'data-path': '/project/a.ts', 'data-entry-type': 'file' })
    expect(resolveLongPressDropTarget(node)).toBeNull()
  })

  it('should resolve a project row to a project target', () => {
    const node = row({ 'data-project-path': '/work/other' })
    expect(resolveLongPressDropTarget(node)).toEqual({ kind: 'project', path: '/work/other' })
  })

  it('should find the target from a descendant of the row', () => {
    // The finger lands on the label or the icon, never on the row element.
    const node = row({ 'data-path': '/project/src', 'data-entry-type': 'directory' })
    const label = document.createElement('span')
    node.appendChild(label)
    expect(resolveLongPressDropTarget(label)).toEqual({ kind: 'entry', path: '/project/src' })
  })

  it('should resolve nothing outside any row', () => {
    expect(resolveLongPressDropTarget(document.body)).toBeNull()
    expect(resolveLongPressDropTarget(null)).toBeNull()
  })
})

describe('useTreeLongPressDrag', () => {
  const onDragStart = vi.fn()
  const onDrop = vi.fn()
  const onCancel = vi.fn()
  let elementFromPoint: ReturnType<typeof vi.fn>

  function setup(paths: string[] = ['/project/a.ts']) {
    return renderHook(() =>
      useTreeLongPressDrag({
        getDragPaths: () => paths,
        onDragStart,
        onDrop,
        onCancel
      })
    )
  }

  function touch(overrides: Partial<React.PointerEvent> = {}): React.PointerEvent {
    return {
      pointerType: 'touch',
      clientX: 0,
      clientY: 0,
      ...overrides
    } as React.PointerEvent
  }

  beforeEach(() => {
    vi.useFakeTimers()
    onDragStart.mockClear()
    onDrop.mockClear()
    onCancel.mockClear()
    elementFromPoint = vi.fn(() => null)
    // jsdom has no layout, so hit-testing has to be stubbed.
    document.elementFromPoint = elementFromPoint as unknown as typeof document.elementFromPoint
  })

  afterEach(() => {
    vi.useRealTimers()
    document.body.innerHTML = ''
  })

  it('should start dragging only after the hold completes', () => {
    const { result } = setup()

    act(() => result.current.handlers.onPointerDown(touch()))
    act(() => vi.advanceTimersByTime(LONG_PRESS_MS - 1))
    expect(onDragStart).not.toHaveBeenCalled()

    act(() => vi.advanceTimersByTime(1))
    expect(onDragStart).toHaveBeenCalledWith(['/project/a.ts'])
    expect(result.current.isDragging).toBe(true)
  })

  it('should treat finger travel before the hold as a scroll', () => {
    const { result } = setup()

    act(() => result.current.handlers.onPointerDown(touch()))
    act(() => result.current.handlers.onPointerMove(touch({ clientX: LONG_PRESS_SLOP_PX + 1 })))
    act(() => vi.advanceTimersByTime(LONG_PRESS_MS))

    // Otherwise every flick through a long tree would arm a drag.
    expect(onDragStart).not.toHaveBeenCalled()
  })

  it('should ignore mouse pointers so the native drag keeps working', () => {
    const { result } = setup()

    act(() => result.current.handlers.onPointerDown(touch({ pointerType: 'mouse' })))
    act(() => vi.advanceTimersByTime(LONG_PRESS_MS))

    expect(onDragStart).not.toHaveBeenCalled()
  })

  it('should drop onto the row under the finger', () => {
    const target = row({ 'data-path': '/project/src', 'data-entry-type': 'directory' })
    elementFromPoint.mockReturnValue(target)
    const { result } = setup()

    act(() => result.current.handlers.onPointerDown(touch()))
    act(() => vi.advanceTimersByTime(LONG_PRESS_MS))
    act(() => result.current.handlers.onPointerUp(touch({ clientX: 5, clientY: 5 })))

    expect(onDrop).toHaveBeenCalledWith(['/project/a.ts'], {
      kind: 'entry',
      path: '/project/src'
    })
  })

  it('should cancel instead of dropping when the finger lifts over nothing', () => {
    const { result } = setup()

    act(() => result.current.handlers.onPointerDown(touch()))
    act(() => vi.advanceTimersByTime(LONG_PRESS_MS))
    act(() => result.current.handlers.onPointerUp(touch()))

    expect(onDrop).not.toHaveBeenCalled()
    expect(onCancel).toHaveBeenCalled()
  })

  it('should not fire cancel for a plain tap', () => {
    const { result } = setup()

    act(() => result.current.handlers.onPointerDown(touch()))
    act(() => result.current.handlers.onPointerUp(touch()))

    // A tap is a selection, not an aborted drag; reporting it as one would
    // clear the drag state a different row is relying on.
    expect(onCancel).not.toHaveBeenCalled()
    expect(onDrop).not.toHaveBeenCalled()
  })

  it('should track the hovered target while dragging', () => {
    const target = row({ 'data-project-path': '/work/other' })
    elementFromPoint.mockReturnValue(target)
    const { result } = setup()

    act(() => result.current.handlers.onPointerDown(touch()))
    act(() => vi.advanceTimersByTime(LONG_PRESS_MS))
    act(() => result.current.handlers.onPointerMove(touch({ clientX: 40, clientY: 40 })))

    expect(result.current.hoverTarget).toEqual({ kind: 'project', path: '/work/other' })
  })

  it('should not arm a drag when the row has nothing to drag', () => {
    const { result } = setup([])

    act(() => result.current.handlers.onPointerDown(touch()))
    act(() => vi.advanceTimersByTime(LONG_PRESS_MS))

    expect(onDragStart).not.toHaveBeenCalled()
    expect(result.current.isDragging).toBe(false)
  })
})
