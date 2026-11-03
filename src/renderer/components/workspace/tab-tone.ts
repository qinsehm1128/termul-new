import { cn } from '@/lib/utils'

/**
 * Colour tone of an editor-style tab, shared by every tab strip so all of them
 * follow the user's colour theme (Preferences → colour theme) the same way.
 * Only theme tokens are used: `--background`, `--foreground`, `--muted-foreground`,
 * `--secondary` and `--primary` are rewritten by `applyColorTheme`.
 *
 * The active tab takes the content background and sits 1px over the strip's
 * bottom border, so it reads as connected to the panel beneath it; a thin
 * primary line on top marks it. Inactive tabs stay muted until hovered.
 */
export function tabToneClass(isActive: boolean): string {
  return isActive
    ? 'z-[1] -mb-px bg-background text-foreground shadow-[inset_0_1px_0_0_hsl(var(--primary)/0.55)]'
    : 'text-muted-foreground hover:bg-secondary/40 hover:text-foreground'
}

/** Strip that hosts `tabToneClass` tabs. */
export const TAB_STRIP_CLASS = 'flex h-8 items-center border-b border-border/70 bg-sidebar'

/** Close button inside a tab: visible on the active tab, on hover otherwise. */
export function tabCloseButtonClass(isActive: boolean): string {
  return cn(
    'ml-auto flex h-4 w-4 shrink-0 items-center justify-center text-muted-foreground transition-opacity duration-150 ease-[var(--ease-out)] hover:bg-secondary hover:text-foreground disabled:cursor-wait',
    isActive ? 'opacity-70 hover:opacity-100' : 'opacity-0 group-hover:opacity-100'
  )
}
