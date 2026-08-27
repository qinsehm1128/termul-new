import * as React from 'react'

import { cn } from '@/lib/utils'

const Input = React.forwardRef<HTMLInputElement, React.ComponentProps<'input'>>(
  ({ className, type, ...props }, ref) => {
    return (
      <input
        type={type}
        className={cn(
          'flex h-8 w-full rounded-md border border-input/80 bg-secondary/35 px-2.5 py-1.5 text-sm text-foreground transition-[border-color,background-color] file:border-0 file:bg-transparent file:text-xs file:font-medium file:text-foreground placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:bg-secondary/50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring/35 disabled:cursor-not-allowed disabled:opacity-45',
          className
        )}
        ref={ref}
        {...props}
      />
    )
  }
)
Input.displayName = 'Input'

export { Input }
