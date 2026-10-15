import * as TabsPrimitive from '@radix-ui/react-tabs'
import * as React from 'react'

import { cn } from '@/lib/utils'

/**
 * Top-of-page section tabs for configuration pages: one section on screen at
 * a time instead of every section stacked in a narrow column.
 */
const PageTabs = TabsPrimitive.Root

const PageTabsList = React.forwardRef<
  React.ElementRef<typeof TabsPrimitive.List>,
  React.ComponentPropsWithoutRef<typeof TabsPrimitive.List>
>(({ className, ...props }, ref) => (
  <TabsPrimitive.List
    ref={ref}
    className={cn(
      'flex max-w-full items-center gap-1 overflow-x-auto rounded-xl border border-border/70 bg-card/60 p-1 shadow-[inset_0_1px_0_hsl(var(--foreground)/0.03)]',
      className
    )}
    {...props}
  />
))
PageTabsList.displayName = 'PageTabsList'

const PageTabsTrigger = React.forwardRef<
  React.ElementRef<typeof TabsPrimitive.Trigger>,
  React.ComponentPropsWithoutRef<typeof TabsPrimitive.Trigger> & { icon?: React.ReactNode }
>(({ className, icon, children, ...props }, ref) => (
  <TabsPrimitive.Trigger
    ref={ref}
    className={cn(
      'inline-flex h-9 shrink-0 items-center gap-2 whitespace-nowrap rounded-lg px-4 text-sm text-muted-foreground transition-colors hover:bg-secondary/50 hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-45 data-[state=active]:bg-secondary data-[state=active]:font-medium data-[state=active]:text-foreground data-[state=active]:shadow-sm [&_svg]:size-4 [&_svg]:shrink-0',
      className
    )}
    {...props}
  >
    {icon}
    {children}
  </TabsPrimitive.Trigger>
))
PageTabsTrigger.displayName = 'PageTabsTrigger'

/**
 * Inactive sections stay mounted, only hidden, so a half-filled form survives
 * a look at another tab.
 */
const PageTabsContent = React.forwardRef<
  React.ElementRef<typeof TabsPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof TabsPrimitive.Content>
>(({ className, ...props }, ref) => (
  <TabsPrimitive.Content
    ref={ref}
    forceMount
    className={cn('mt-5 focus-visible:outline-none data-[state=inactive]:hidden', className)}
    {...props}
  />
))
PageTabsContent.displayName = 'PageTabsContent'

export { PageTabs, PageTabsContent, PageTabsList, PageTabsTrigger }
