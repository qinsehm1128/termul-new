import { usePersistedPanelSize } from '@/hooks/use-persisted-panel-size'
import { cn } from '@/lib/utils'
import { PanelResizeHandle } from './PanelResizeHandle'

interface ResizableRailProps {
  side: 'left' | 'right'
  storageKey: string
  initial?: number
  min?: number
  max?: number
  className?: string
  children: React.ReactNode
  resizeTitle: string
  resizeLabel: string
}

export function ResizableRail({
  side,
  storageKey,
  initial = 240,
  min = 180,
  max = 480,
  className,
  children,
  resizeTitle,
  resizeLabel
}: ResizableRailProps): React.JSX.Element {
  const [width, setWidth] = usePersistedPanelSize(storageKey, { initial, min, max })

  return (
    <div
      className={cn('relative h-full min-h-0 min-w-0 shrink-0', className)}
      style={{ width }}
      data-testid={`resizable-rail-${side}`}
    >
      <div className="h-full min-h-0 w-full min-w-0 overflow-hidden">{children}</div>
      <PanelResizeHandle
        axis="x"
        invert={side === 'right'}
        value={width}
        min={min}
        max={max}
        onChange={setWidth}
        title={resizeTitle}
        label={resizeLabel}
        className={side === 'right' ? 'left-0' : 'right-0'}
      />
    </div>
  )
}
