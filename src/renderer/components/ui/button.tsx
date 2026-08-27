import { Slot } from '@radix-ui/react-slot'
import { cva, type VariantProps } from 'class-variance-authority'
import * as React from 'react'

import { cn } from '@/lib/utils'

const buttonVariants = cva(
  'inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded-md text-xs font-medium transition-[background-color,color,border-color,opacity] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-45 [&_svg]:pointer-events-none [&_svg]:size-3.5 [&_svg]:shrink-0',
  {
    variants: {
      variant: {
        default: 'bg-foreground text-background hover:bg-foreground/88',
        destructive: 'bg-destructive text-destructive-foreground hover:bg-destructive/90',
        outline:
          'border border-border/80 bg-transparent text-foreground hover:bg-secondary hover:border-border',
        secondary: 'bg-secondary text-secondary-foreground hover:bg-secondary/80',
        ghost: 'text-muted-foreground hover:bg-secondary hover:text-foreground',
        link: 'text-foreground underline-offset-4 hover:underline'
      },
      size: {
        default: 'h-8 px-3',
        xs: 'h-6 px-2 text-2xs',
        sm: 'h-7 px-2.5',
        lg: 'h-9 px-4 text-sm',
        icon: 'h-8 w-8',
        'icon-xs': 'h-6 w-6',
        'icon-sm': 'h-7 w-7',
        'icon-lg': 'h-9 w-9'
      }
    },
    defaultVariants: {
      variant: 'default',
      size: 'default'
    }
  }
)

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  asChild?: boolean
}

const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, asChild = false, ...props }, ref) => {
    const Comp = asChild ? Slot : 'button'
    return (
      <Comp className={cn(buttonVariants({ variant, size, className }))} ref={ref} {...props} />
    )
  }
)
Button.displayName = 'Button'

export { Button, buttonVariants }
