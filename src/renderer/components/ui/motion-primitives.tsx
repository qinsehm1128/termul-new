import { AnimatePresence, motion, type Transition, useReducedMotion } from 'framer-motion'
import type { ReactNode } from 'react'
import type { TerminalBoardStatusKey } from '@/lib/terminal-board'
import { cn } from '@/lib/utils'

/** Snappy tactile spring for shared-element pills, tabs and segmented controls. */
export const SE_SPRING_SNAPPY: Transition = {
  type: 'spring',
  stiffness: 520,
  damping: 36,
  mass: 0.7
}

/** Smooth panel spring for side rails and collapsible drawers (zero overshoot). */
export const SE_SPRING_PANEL: Transition = {
  type: 'spring',
  stiffness: 420,
  damping: 40,
  mass: 0.85
}

/**
 * Shared semantic status dot tokens for terminal boards and switchers.
 * Always derives from theme CSS variables (`--success`, `--warning`,
 * `--destructive`, `--muted-foreground`) instead of hardcoded Tailwind palettes.
 */
export const TERMINAL_STATUS_DOT_CLASS: Record<TerminalBoardStatusKey, string> = {
  live: 'bg-success',
  hidden: 'bg-muted-foreground/60',
  exited: 'bg-muted-foreground/60',
  disconnected: 'bg-destructive',
  attention: 'bg-warning ring-2 ring-warning/30'
}

interface SpringSlidingPillProps {
  layoutId: string
  className?: string
}

/**
 * Shared-element spring indicator for segmented controls and active tabs.
 * Falls back to instant placement when `prefers-reduced-motion` is enabled.
 */
export function SpringSlidingPill({
  layoutId,
  className
}: SpringSlidingPillProps): React.JSX.Element {
  const reduced = useReducedMotion() ?? false
  return (
    <motion.span
      layoutId={layoutId}
      aria-hidden="true"
      transition={reduced ? { duration: 0 } : SE_SPRING_SNAPPY}
      className={cn('pointer-events-none absolute inset-0 rounded-[inherit]', className)}
    />
  )
}

interface SpringCollapsibleWidthProps {
  open: boolean
  children: ReactNode
  className?: string
  'data-testid'?: string
}

/**
 * Animates a horizontal side panel smoothly in and out by width + opacity so
 * adjacent workspace panes yield and reclaim space continuously instead of
 * snapping on unmount.
 */
export function SpringCollapsibleWidth({
  open,
  children,
  className,
  'data-testid': testId
}: SpringCollapsibleWidthProps): React.JSX.Element {
  const reduced = useReducedMotion() ?? false

  return (
    <AnimatePresence initial={false}>
      {open ? (
        <motion.div
          key="collapsible-panel"
          data-testid={testId}
          initial={reduced ? { opacity: 1 } : { width: 0, opacity: 0 }}
          animate={{ width: 'auto', opacity: 1 }}
          exit={reduced ? { opacity: 0 } : { width: 0, opacity: 0 }}
          transition={reduced ? { duration: 0 } : SE_SPRING_PANEL}
          className={cn('h-full min-h-0 shrink-0 overflow-hidden', className)}
        >
          {children}
        </motion.div>
      ) : null}
    </AnimatePresence>
  )
}

interface WorkspaceSkeletonProps {
  className?: string
  compact?: boolean
}

/**
 * Structured loading placeholder for workspace panes and side panels.
 * Replaces bare full-bleed gray rectangles with calm, minimal layout cues.
 */
export function WorkspaceSkeleton({
  className,
  compact = false
}: WorkspaceSkeletonProps): React.JSX.Element {
  return (
    <div
      role="status"
      aria-busy="true"
      className={cn(
        'flex h-full w-full flex-col gap-3 overflow-hidden bg-background p-4',
        className
      )}
    >
      {!compact && (
        <div className="flex items-center gap-2 border-b border-border/50 pb-2.5">
          <div className="h-4 w-24 animate-pulse rounded bg-secondary/70" />
          <div className="h-4 w-16 animate-pulse rounded bg-secondary/50" />
          <div className="ml-auto h-4 w-12 animate-pulse rounded bg-secondary/40" />
        </div>
      )}
      <div className="space-y-2.5">
        <div className="h-3 w-3/5 animate-pulse rounded bg-secondary/60" />
        <div className="h-3 w-4/5 animate-pulse rounded bg-secondary/45" />
        <div className="h-3 w-2/5 animate-pulse rounded bg-secondary/35" />
      </div>
    </div>
  )
}
