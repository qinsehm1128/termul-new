import { useEffect, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import { availableColors, getColorClasses } from '@/lib/colors'
import { cn } from '@/lib/utils'
import type { ProjectColor } from '@/types/project'

interface ColorPickerPopoverProps {
  x: number
  y: number
  currentColor: ProjectColor
  onSelectColor: (color: ProjectColor) => void
  onClose: () => void
}

export function ColorPickerPopover({
  x,
  y,
  currentColor,
  onSelectColor,
  onClose
}: ColorPickerPopoverProps): React.JSX.Element {
  const { t } = useTranslation('projects')
  const popoverRef = useRef<HTMLDivElement>(null)

  // Adjust position if popover would overflow viewport
  const adjustedPosition = (): { left: number; top: number } => {
    const popoverWidth = 200
    const popoverHeight = 80
    const viewportWidth = window.innerWidth
    const viewportHeight = window.innerHeight

    let left = x
    let top = y

    if (x + popoverWidth > viewportWidth) {
      left = viewportWidth - popoverWidth - 8
    }
    if (y + popoverHeight > viewportHeight) {
      top = viewportHeight - popoverHeight - 8
    }

    return { left, top }
  }

  const position = adjustedPosition()

  // Close on click outside or escape
  useEffect(() => {
    const handleClickOutside = (e: MouseEvent): void => {
      if (popoverRef.current && !popoverRef.current.contains(e.target as Node)) {
        onClose()
      }
    }

    const handleEscape = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        onClose()
      }
    }

    const timeoutId = setTimeout(() => {
      document.addEventListener('mousedown', handleClickOutside)
    }, 0)
    document.addEventListener('keydown', handleEscape)

    return () => {
      clearTimeout(timeoutId)
      document.removeEventListener('mousedown', handleClickOutside)
      document.removeEventListener('keydown', handleEscape)
    }
  }, [onClose])

  return (
    <div
      ref={popoverRef}
      className="fixed z-50 rounded-md border border-border/80 bg-popover p-3 shadow-[0_18px_60px_hsl(var(--background)/0.7),inset_0_1px_0_0_hsl(var(--foreground)/0.05)]"
      style={{ left: position.left, top: position.top }}
    >
      <p className="text-xs text-muted-foreground mb-2">{t('selectColor')}</p>
      <div className="flex gap-2 flex-wrap">
        {availableColors.map((color) => {
          const colors = getColorClasses(color)
          return (
            <button
              type="button"
              key={color}
              onClick={() => {
                onSelectColor(color)
                onClose()
              }}
              className={cn(
                'size-6 rounded-sm transition-[opacity,box-shadow]',
                colors.bg,
                currentColor === color
                  ? 'ring-1 ring-foreground/70 ring-offset-2 ring-offset-popover'
                  : 'hover:opacity-80'
              )}
            />
          )
        })}
      </div>
    </div>
  )
}
