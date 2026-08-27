import * as TooltipPrimitive from '@radix-ui/react-tooltip'
import * as React from 'react'

import { cn } from '@/lib/utils'

const TooltipProvider = TooltipPrimitive.Provider

const Tooltip = TooltipPrimitive.Root

const TooltipTrigger = TooltipPrimitive.Trigger

/**
 * Tooltip surface — transitions.dev open/close adapted for Radix:
 * fade + scale(0.98→1) on open (150ms), short leave (50ms, no delay).
 * Open delay is TooltipProvider `delayDuration` (--tt-delay), not CSS.
 */
const TooltipContent = React.forwardRef<
  React.ElementRef<typeof TooltipPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof TooltipPrimitive.Content>
>(({ className, sideOffset = 4, ...props }, ref) => (
  <TooltipPrimitive.Content
    ref={ref}
    sideOffset={sideOffset}
    className={cn(
      'z-50 rounded-md border border-border/80 bg-popover px-2 py-1 text-xs text-popover-foreground shadow-[0_6px_18px_hsl(var(--background)/0.5)]',
      'animate-tooltip-in',
      className
    )}
    {...props}
  />
))
TooltipContent.displayName = TooltipPrimitive.Content.displayName

export { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger }
