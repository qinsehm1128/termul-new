import type { ReactNode } from 'react'
import { cn } from '@/lib/utils'

export type ListRowDensity = 'compact' | 'comfortable'

export interface ListRowProps {
  title: ReactNode
  preview?: ReactNode
  meta?: ReactNode
  trailing?: ReactNode
  details?: ReactNode
  expanded?: boolean
  active?: boolean
  disabled?: boolean
  density?: ListRowDensity
  onClick?: () => void
  className?: string
  buttonClassName?: string
  titleAttr?: string
}

export function ListRow({
  title,
  preview,
  meta,
  trailing,
  details,
  expanded = false,
  active = false,
  disabled = false,
  density = 'compact',
  onClick,
  className,
  buttonClassName,
  titleAttr
}: ListRowProps): React.JSX.Element {
  const comfortable = density === 'comfortable'
  return (
    <div
      className={cn(
        'group group/list-row mx-1 flex flex-col rounded-md pr-0.5 transition-colors duration-150 ease-[var(--ease-out)]',
        comfortable ? 'px-2 py-2' : 'px-1.5 py-1.5',
        active ? 'bg-sidebar-accent text-foreground' : 'hover:bg-sidebar-accent/50',
        disabled && 'opacity-50',
        className
      )}
      data-list-row=""
      data-active={active || undefined}
      data-density={density}
    >
      <div className="flex min-w-0 items-start gap-1">
        <button
          type="button"
          disabled={disabled && !onClick}
          title={titleAttr}
          onClick={onClick}
          className={cn(
            'min-w-0 flex-1 text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring',
            disabled && 'cursor-default',
            buttonClassName
          )}
        >
          <span
            className={cn(
              'block min-w-0 truncate font-medium leading-5 text-foreground',
              comfortable ? 'text-[13px]' : 'text-xs'
            )}
          >
            {title}
          </span>
          {preview ? (
            <span
              className={cn(
                'mt-0.5 block min-w-0 text-muted-foreground',
                comfortable ? 'line-clamp-2 text-[12px] leading-4' : 'truncate text-2xs leading-3.5'
              )}
            >
              {preview}
            </span>
          ) : null}
          {meta ? <div className="mt-1 min-w-0">{meta}</div> : null}
        </button>
        {trailing ? (
          <div
            className={cn(
              'flex shrink-0 items-center gap-0.5 pt-0.5 text-muted-foreground',
              'opacity-100 [@media(hover:hover)]:opacity-0 [@media(hover:hover)]:group-hover/list-row:opacity-100 [@media(hover:hover)]:group-focus-within/list-row:opacity-100'
            )}
            data-list-row-trailing=""
          >
            {trailing}
          </div>
        ) : null}
      </div>
      {expanded && details ? (
        <div className="mt-2 min-w-0 border-t border-sidebar-border/70 pt-2 text-2xs leading-4 text-muted-foreground">
          {details}
        </div>
      ) : null}
    </div>
  )
}

export function ListRowMeta({
  items
}: {
  items: Array<ReactNode | null | undefined | false>
}): React.JSX.Element | null {
  const visible = items.filter((item) => item !== null && item !== undefined && item !== false)
  if (visible.length === 0) return null
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-0.5 text-2xs leading-4 text-muted-foreground">
      {visible.map((item, index) => (
        <span key={index} className="inline-flex min-w-0 items-center gap-1 truncate">
          {index > 0 ? <span className="text-muted-foreground/50">·</span> : null}
          {item}
        </span>
      ))}
    </div>
  )
}

export type ListRowStatusTone = 'need' | 'working' | 'idle'

export function ListRowStatus({
  status,
  label,
  className
}: {
  status: ListRowStatusTone
  label: string
  className?: string
}): React.JSX.Element {
  return (
    <span
      data-list-row-status={status}
      className={cn(
        'inline-flex shrink-0 items-center gap-1 rounded-sm px-1 py-px text-2xs leading-3.5',
        status === 'need' && 'bg-accent/15 text-accent',
        status === 'working' && 'text-connection',
        status === 'idle' && 'text-muted-foreground',
        className
      )}
    >
      {status === 'working' ? (
        <span className="size-1 shrink-0 rounded-full bg-connection" aria-hidden="true" />
      ) : null}
      {label}
    </span>
  )
}
