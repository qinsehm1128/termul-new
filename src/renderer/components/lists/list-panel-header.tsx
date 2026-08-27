import { Search, X } from 'lucide-react'
import type { ReactNode } from 'react'
import { cn } from '@/lib/utils'

export interface ListPanelHeaderProps {
  title: string
  shown?: number
  total?: number
  countLabel?: string
  search?: string
  onSearchChange?: (value: string) => void
  searchLabel: string
  searchPlaceholder?: string
  clearSearchLabel?: string
  actions?: ReactNode
  children?: ReactNode
  className?: string
}

export function ListPanelHeader({
  title,
  shown,
  total,
  countLabel,
  search,
  onSearchChange,
  searchLabel,
  searchPlaceholder,
  clearSearchLabel,
  actions,
  children,
  className
}: ListPanelHeaderProps): React.JSX.Element {
  return (
    <div className={cn('shrink-0 border-b border-sidebar-border/70', className)}>
      <div className="flex h-8 items-center justify-between gap-2 px-2.5">
        <div className="min-w-0">
          <span className="label-section text-sidebar-foreground">{title}</span>
          {countLabel ? (
            <span className="ml-1.5 text-2xs tabular-nums text-muted-foreground">{countLabel}</span>
          ) : shown !== undefined && total !== undefined ? (
            <span className="ml-1.5 text-2xs tabular-nums text-muted-foreground">
              {shown} / {total}
            </span>
          ) : null}
        </div>
        {actions ? <div className="flex shrink-0 items-center gap-0.5">{actions}</div> : null}
      </div>
      {onSearchChange ? (
        <div className="relative px-2.5 pb-1.5">
          <Search
            className="pointer-events-none absolute left-4 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
            aria-hidden="true"
          />
          <input
            type="search"
            value={search ?? ''}
            onChange={(event) => onSearchChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Escape' && search) {
                event.preventDefault()
                onSearchChange('')
              }
            }}
            placeholder={searchPlaceholder ?? searchLabel}
            aria-label={searchLabel}
            className="h-8 w-full rounded-md border-0 bg-secondary/35 pl-7 pr-7 text-xs text-foreground outline-none placeholder:text-muted-foreground/60 focus-visible:ring-1 focus-visible:ring-ring/50 [&::-webkit-search-cancel-button]:hidden"
          />
          {search ? (
            <button
              type="button"
              className="absolute right-3 top-1/2 inline-flex size-7 -translate-y-1/2 items-center justify-center rounded-md text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              aria-label={clearSearchLabel ?? searchLabel}
              onClick={() => onSearchChange('')}
            >
              <X className="size-3.5" aria-hidden="true" />
            </button>
          ) : null}
        </div>
      ) : null}
      {children ? <div className="space-y-1.5 px-2.5 pb-1.5">{children}</div> : null}
    </div>
  )
}

export function ListScopeSwitch<T extends string>({
  value,
  onChange,
  options,
  ariaLabel
}: {
  value: T
  onChange: (value: T) => void
  options: Array<{ value: T; label: string }>
  ariaLabel: string
}): React.JSX.Element {
  return (
    <div
      role="radiogroup"
      aria-label={ariaLabel}
      className="flex min-w-0 flex-wrap gap-1 rounded-md bg-secondary/35 p-0.5"
    >
      {options.map((option) => {
        const selected = option.value === value
        return (
          <button
            key={option.value}
            type="button"
            role="radio"
            aria-checked={selected}
            className={cn(
              'h-7 min-w-0 flex-1 truncate rounded-sm px-2 text-2xs transition-colors duration-150 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
              selected
                ? 'bg-background text-foreground shadow-sm'
                : 'text-muted-foreground hover:text-foreground'
            )}
            onClick={() => onChange(option.value)}
          >
            {option.label}
          </button>
        )
      })}
    </div>
  )
}
