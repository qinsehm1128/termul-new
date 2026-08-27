import * as React from 'react'

import { cn } from '@/lib/utils'

export type TextareaProps = React.TextareaHTMLAttributes<HTMLTextAreaElement>

const Textarea = React.forwardRef<HTMLTextAreaElement, TextareaProps>(
  ({ className, ...props }, ref) => {
    return (
      <textarea
        className={cn(
          'flex min-h-[72px] w-full rounded-md border border-input/80 bg-secondary/35 px-2.5 py-2 text-sm text-foreground transition-[border-color,background-color] placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:bg-secondary/50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring/35 disabled:cursor-not-allowed disabled:opacity-45',
          className
        )}
        ref={ref}
        {...props}
      />
    )
  }
)
Textarea.displayName = 'Textarea'

export { Textarea }
