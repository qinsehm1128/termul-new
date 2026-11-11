import * as TooltipPrimitive from '@radix-ui/react-tooltip'
import { useRef, useState } from 'react'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'

interface TruncatedTextProps {
  text: string
  className?: string
  side?: 'top' | 'right' | 'bottom' | 'left'
}

/**
 * Single-line text that shows its full value in a tooltip, but only when it is
 * actually clipped. A native `title` is unreliable in the desktop WebView, so
 * long names stayed unreadable on hover.
 */
export function TruncatedText({ text, className, side = 'right' }: TruncatedTextProps) {
  const ref = useRef<HTMLSpanElement>(null)
  const [open, setOpen] = useState(false)

  const handleOpenChange = (next: boolean): void => {
    if (!next) {
      setOpen(false)
      return
    }
    const el = ref.current
    setOpen(el !== null && el.scrollWidth > el.clientWidth)
  }

  return (
    <TooltipProvider delayDuration={300}>
      <Tooltip open={open} onOpenChange={handleOpenChange}>
        <TooltipTrigger asChild>
          <span ref={ref} className={className}>
            {text}
          </span>
        </TooltipTrigger>
        {/* Portal: the sidebar clips overflow and its rows can carry transforms. */}
        <TooltipPrimitive.Portal>
          <TooltipContent side={side} sideOffset={8} className="max-w-80 break-all">
            {text}
          </TooltipContent>
        </TooltipPrimitive.Portal>
      </Tooltip>
    </TooltipProvider>
  )
}
