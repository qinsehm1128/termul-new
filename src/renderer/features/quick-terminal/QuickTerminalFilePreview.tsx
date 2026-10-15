import { brandCanonical } from '@shared/brand'
import { X } from 'lucide-react'
import { lazy, Suspense } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { ResizableRail } from '@/components/workspace/ResizableRail'

const EditorPanel = lazy(() =>
  import('@/components/editor/EditorPanel').then((module) => ({ default: module.EditorPanel }))
)

/** A file opened from the tree, shown beside the quick terminal's shell. */
export function QuickTerminalFilePreview({
  path,
  onClose
}: {
  path: string
  onClose: () => void
}): React.JSX.Element {
  const { t } = useTranslation('quickTerminal')
  const name = path.split(/[\\/]/).pop() || path

  return (
    <ResizableRail
      side="right"
      storageKey={`${brandCanonical().storageKeyPrefix}quick-terminal-preview-width`}
      initial={520}
      min={280}
      max={1200}
      className="border-l border-border/70"
      resizeTitle={t('resizePreview')}
      resizeLabel={t('resizePreview')}
    >
      <section aria-label={t('preview', { name })} className="flex h-full min-h-0 flex-col">
        <div className="flex h-9 shrink-0 items-center justify-between gap-2 border-b px-3">
          <span className="truncate text-xs text-muted-foreground" title={path}>
            {name}
          </span>
          <Button
            size="icon"
            variant="ghost"
            className="size-6 shrink-0"
            aria-label={t('closePreview')}
            onClick={onClose}
          >
            <X className="size-3.5" aria-hidden="true" />
          </Button>
        </div>
        <div className="relative min-h-0 flex-1">
          <Suspense fallback={null}>
            <EditorPanel filePath={path} isVisible />
          </Suspense>
        </div>
      </section>
    </ResizableRail>
  )
}
