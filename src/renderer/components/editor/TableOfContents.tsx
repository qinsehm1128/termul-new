import { List, Settings2 } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'
import type { TocHeading } from '@/hooks/use-toc-headings'
import { cn } from '@/lib/utils'

const HEADING_LEVEL_OPTIONS = [1, 2, 3, 4, 5, 6]

interface TableOfContentsProps {
  headings: TocHeading[]
  activeHeadingId?: string
  maxHeadingLevel: number
  onHeadingClick: (heading: TocHeading) => void
  onMaxHeadingLevelChange: (level: number) => void
}

export function TableOfContents({
  headings,
  activeHeadingId,
  maxHeadingLevel,
  onHeadingClick,
  onMaxHeadingLevelChange
}: TableOfContentsProps): React.JSX.Element {
  const { t } = useTranslation('workspace')
  return (
    <div className="flex h-full min-h-0 flex-col border-l border-border/70 bg-sidebar shadow-[inset_1px_0_0_hsl(var(--background)/0.35)]">
      <div className="flex h-8 shrink-0 items-center justify-between gap-2 border-b border-border/70 px-2.5 shadow-[inset_0_1px_0_hsl(var(--foreground)/0.025)]">
        <div className="flex items-center gap-2 text-xs font-medium text-foreground">
          <List size={16} />
          <span>{t('toc.contents')}</span>
        </div>

        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon"
              className="size-7"
              title={t('toc.settings')}
              aria-label={t('toc.settings')}
            >
              <Settings2 size={16} />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuRadioGroup
              value={String(maxHeadingLevel)}
              onValueChange={(value) => onMaxHeadingLevelChange(Number(value))}
            >
              {HEADING_LEVEL_OPTIONS.map((level) => (
                <DropdownMenuRadioItem key={level} value={String(level)}>
                  {`H1-H${level}`}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      {headings.length === 0 ? (
        <div className="flex flex-1 items-center justify-center px-4 text-center text-sm text-muted-foreground">
          {t('toc.empty')}
        </div>
      ) : (
        <nav className="flex-1 overflow-auto py-2" aria-label={t('toc.aria')}>
          <ul className="space-y-1 px-2">
            {headings.map((heading) => {
              const isActive = heading.id === activeHeadingId

              return (
                <li key={heading.id}>
                  <button
                    type="button"
                    className={cn(
                      'w-full rounded-sm px-2 py-1.5 text-left text-sm transition-colors duration-150 hover:bg-sidebar-accent/50 hover:text-foreground',
                      isActive &&
                        'bg-sidebar-accent text-foreground shadow-[inset_0_1px_0_hsl(var(--foreground)/0.035)] ring-1 ring-inset ring-primary/35'
                    )}
                    style={{ paddingLeft: (heading.level - 1) * 12 + 8 }}
                    onClick={() => onHeadingClick(heading)}
                    title={heading.text}
                    aria-current={isActive ? 'location' : undefined}
                  >
                    <span className="block truncate">{heading.text}</span>
                  </button>
                </li>
              )
            })}
          </ul>
        </nav>
      )}
    </div>
  )
}
