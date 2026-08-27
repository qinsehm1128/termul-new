import { type ReactNode, useEffect, useState } from 'react'
import { cn } from '@/lib/utils'

const FADE_MS = 150

function prefersReducedMotion(): boolean {
  return (
    typeof window !== 'undefined' &&
    window.matchMedia?.('(prefers-reduced-motion: reduce)').matches === true
  )
}

interface PanelFadeProps {
  open: boolean
  children: ReactNode
  className?: string
  'data-testid'?: string
}

/**
 * Opacity-only show/hide for IDE side rails. Width/height stay with the child
 * so toggling does not tween layout or scrollports. First paint is instant;
 * subsequent enter/exit fade for ~150ms unless reduced motion is requested.
 */
export function PanelFade({
  open,
  children,
  className,
  'data-testid': testId = 'panel-fade'
}: PanelFadeProps): React.JSX.Element | null {
  const reduced = prefersReducedMotion()
  const duration = reduced ? 0 : FADE_MS
  const [rendered, setRendered] = useState(open)
  const [visible, setVisible] = useState(open)

  useEffect(() => {
    if (open) {
      setRendered(true)
      if (reduced) {
        setVisible(true)
        return
      }
      const frame = window.requestAnimationFrame(() => setVisible(true))
      return () => window.cancelAnimationFrame(frame)
    }

    setVisible(false)
    if (duration === 0) {
      setRendered(false)
      return
    }
    const timeout = window.setTimeout(() => setRendered(false), duration)
    return () => window.clearTimeout(timeout)
  }, [duration, open, reduced])

  if (!rendered) return null

  return (
    <div
      data-testid={testId}
      data-panel-fade=""
      data-state={visible ? 'open' : 'closed'}
      className={cn(
        'min-h-0 overflow-hidden',
        !reduced &&
          'transition-opacity duration-150 ease-[var(--ease-out)] motion-reduce:transition-none',
        visible ? 'opacity-100' : 'pointer-events-none opacity-0',
        className
      )}
    >
      {children}
    </div>
  )
}
