import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/utils'

export function ListEmptyState({
  title,
  message,
  tone = 'empty'
}: {
  title?: string
  message: string
  tone?: 'empty' | 'error'
}): React.JSX.Element {
  return (
    <div className="px-3 py-5" role={tone === 'error' ? 'alert' : 'status'}>
      {title ? <p className="text-xs font-medium text-foreground">{title}</p> : null}
      <p className={cn('text-xs leading-relaxed text-muted-foreground', title && 'mt-1')}>
        {message}
      </p>
    </div>
  )
}

export function ListLoadingState({
  label,
  rows = 5,
  density = 'compact'
}: {
  label: string
  rows?: number
  density?: 'compact' | 'comfortable'
}): React.JSX.Element {
  return (
    <div className="flex flex-col gap-1 px-2 py-2" role="status" aria-busy="true">
      <span className="px-1 pb-1 text-xs text-muted-foreground">{label}</span>
      {Array.from({ length: rows }, (_, index) => (
        <div
          key={index}
          className={cn('rounded-md px-2', density === 'comfortable' ? 'py-2' : 'py-1.5')}
        >
          <Skeleton className="h-3 w-2/5" />
          <Skeleton className="mt-1.5 h-2.5 w-4/5" />
          <Skeleton className="mt-1.5 h-2 w-1/3" />
        </div>
      ))}
    </div>
  )
}
