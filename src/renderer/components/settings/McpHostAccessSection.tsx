import { ChevronDown } from 'lucide-react'
import type { ReactNode } from 'react'
import { useId } from 'react'

export function HostAccessSection({
  title,
  summary,
  open,
  onOpenChange,
  children
}: {
  title: string
  summary: ReactNode
  open: boolean
  onOpenChange: (open: boolean) => void
  children: ReactNode
}): React.JSX.Element {
  const id = useId()
  return (
    <section className="min-w-0 overflow-hidden rounded-lg border border-border bg-background/60">
      <h4>
        <button
          type="button"
          aria-expanded={open}
          aria-controls={id}
          onClick={() => onOpenChange(!open)}
          className="flex w-full min-w-0 items-center gap-3 px-4 py-3 text-left hover:bg-secondary/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
        >
          <ChevronDown
            aria-hidden="true"
            className={`size-4 shrink-0 text-muted-foreground ${open ? '' : '-rotate-90'}`}
          />
          <span className="min-w-0 flex-1 text-sm font-medium">{title}</span>
          <span className="shrink-0 text-xs tabular-nums text-muted-foreground">{summary}</span>
        </button>
      </h4>
      <div id={id} hidden={!open} className="border-t border-border px-4 py-3">
        {children}
      </div>
    </section>
  )
}

export const hostCheckboxClass =
  'size-4 shrink-0 rounded accent-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2'
export const hostInputClass =
  'h-9 w-full min-w-0 rounded-md border border-input bg-background px-3 text-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50'
