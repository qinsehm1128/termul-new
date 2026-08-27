import * as SwitchPrimitives from '@radix-ui/react-switch'
import * as React from 'react'

import { cn } from '@/lib/utils'

const Switch = React.forwardRef<
  React.ElementRef<typeof SwitchPrimitives.Root>,
  React.ComponentPropsWithoutRef<typeof SwitchPrimitives.Root>
>(({ className, ...props }, ref) => (
  <SwitchPrimitives.Root
    className={cn(
      'peer inline-flex h-[18px] w-8 shrink-0 cursor-pointer items-center rounded-full border border-border transition-colors data-[state=checked]:border-primary/70 data-[state=checked]:bg-primary data-[state=unchecked]:bg-input focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-45',
      className
    )}
    {...props}
    ref={ref}
  >
    <SwitchPrimitives.Thumb
      className={cn(
        'pointer-events-none block size-3.5 translate-x-px rounded-full bg-foreground/90 ring-0 transition-transform data-[state=checked]:translate-x-[15px] data-[state=unchecked]:translate-x-px'
      )}
    />
  </SwitchPrimitives.Root>
))
Switch.displayName = SwitchPrimitives.Root.displayName

export { Switch }
