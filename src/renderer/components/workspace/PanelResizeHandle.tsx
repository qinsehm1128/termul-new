import { useCallback, useEffect, useRef } from 'react'
import { cn } from '@/lib/utils'

interface PanelResizeHandleProps {
  axis: 'x' | 'y'
  /** When true, dragging toward the start of the axis shrinks the size. */
  invert?: boolean
  value: number
  min: number
  max: number
  onChange: (next: number) => void
  label: string
  title: string
  className?: string
}

export function PanelResizeHandle({
  axis,
  invert = false,
  value,
  min,
  max,
  onChange,
  label,
  title,
  className
}: PanelResizeHandleProps): React.JSX.Element {
  const dragRef = useRef<{ start: number; size: number } | null>(null)
  const cleanupRef = useRef<(() => void) | null>(null)

  const stopDrag = useCallback(() => {
    dragRef.current = null
    document.body.style.userSelect = ''
    document.body.style.cursor = ''
    cleanupRef.current?.()
    cleanupRef.current = null
  }, [])

  useEffect(() => stopDrag, [stopDrag])

  const handlePointerDown = useCallback(
    (event: React.PointerEvent<HTMLButtonElement>) => {
      if (event.button !== 0) return
      event.preventDefault()
      document.body.style.userSelect = 'none'
      document.body.style.cursor = axis === 'x' ? 'col-resize' : 'row-resize'
      dragRef.current = {
        start: axis === 'x' ? event.clientX : event.clientY,
        size: value
      }

      const onMove = (moveEvent: PointerEvent) => {
        const drag = dragRef.current
        if (!drag) return
        const current = axis === 'x' ? moveEvent.clientX : moveEvent.clientY
        const delta = current - drag.start
        onChange(invert ? drag.size - delta : drag.size + delta)
      }

      const onUp = () => {
        stopDrag()
      }

      document.addEventListener('pointermove', onMove)
      document.addEventListener('pointerup', onUp)
      window.addEventListener('blur', onUp)
      cleanupRef.current = () => {
        document.removeEventListener('pointermove', onMove)
        document.removeEventListener('pointerup', onUp)
        window.removeEventListener('blur', onUp)
      }
    },
    [axis, invert, onChange, stopDrag, value]
  )

  const handleKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLButtonElement>) => {
      const backward = axis === 'x' ? 'ArrowLeft' : 'ArrowUp'
      const forward = axis === 'x' ? 'ArrowRight' : 'ArrowDown'
      if (
        event.key !== backward &&
        event.key !== forward &&
        event.key !== 'Home' &&
        event.key !== 'End'
      ) {
        return
      }
      event.preventDefault()
      if (event.key === 'Home') {
        onChange(min)
        return
      }
      if (event.key === 'End') {
        onChange(max)
        return
      }
      const step = event.key === backward ? -16 : 16
      onChange(value + (invert ? -step : step))
    },
    [axis, invert, max, min, onChange, value]
  )

  return (
    <button
      type="button"
      onPointerDown={handlePointerDown}
      onKeyDown={handleKeyDown}
      title={title}
      aria-label={label}
      role="separator"
      aria-orientation={axis === 'x' ? 'vertical' : 'horizontal'}
      aria-valuenow={value}
      aria-valuemin={min}
      aria-valuemax={max}
      className={cn(
        'absolute z-20 bg-transparent hover:bg-ring/25 focus-visible:bg-ring/30 focus-visible:outline-none',
        axis === 'x'
          ? 'top-0 h-full w-1 cursor-col-resize'
          : 'left-0 h-1.5 w-full cursor-row-resize',
        className
      )}
    />
  )
}
