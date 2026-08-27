import { Code2, Eye, List } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { useTocIsVisible, useTocSettingsStore } from '@/stores/toc-settings-store'

interface EditorToolbarProps {
  viewMode: 'code' | 'markdown'
  onToggleViewMode: () => void
  filePath: string
}

export function EditorToolbar({
  viewMode,
  onToggleViewMode,
  filePath
}: EditorToolbarProps): React.JSX.Element {
  const { t } = useTranslation('workspace')
  const { t: settingsT } = useTranslation('settings')
  const fileName = filePath.split(/[\\/]/).pop() || filePath
  const isTocVisible = useTocIsVisible()
  const toggleTocVisibility = useTocSettingsStore((state) => state.toggleVisibility)

  return (
    <div className="flex h-8 shrink-0 items-center justify-between border-b border-border/70 bg-sidebar px-2.5 shadow-[inset_0_1px_0_hsl(var(--foreground)/0.025)]">
      <span className="truncate text-xs text-muted-foreground">{fileName}</span>
      <div className="flex items-center gap-1">
        <Button
          variant="ghost"
          size="sm"
          className={cn(
            'h-6 gap-1 px-2 text-xs text-muted-foreground transition-colors duration-150 hover:text-foreground',
            isTocVisible &&
              'bg-sidebar-accent text-foreground shadow-[inset_0_1px_0_hsl(var(--foreground)/0.035)] ring-1 ring-inset ring-primary/35'
          )}
          onClick={toggleTocVisibility}
          title={t('toc.toggle')}
          aria-pressed={isTocVisible}
        >
          <List size={14} />
          <span>{t('toc.contents')}</span>
        </Button>

        <Button
          variant="ghost"
          size="sm"
          onClick={onToggleViewMode}
          className="h-6 gap-1 px-2 text-xs text-muted-foreground transition-colors duration-150 hover:bg-secondary hover:text-foreground"
          title={
            viewMode === 'markdown'
              ? settingsT('editor.switchToSource')
              : settingsT('editor.switchToWysiwyg')
          }
        >
          {viewMode === 'markdown' ? (
            <>
              <Code2 size={14} />
              <span>{settingsT('editor.source')}</span>
            </>
          ) : (
            <>
              <Eye size={14} />
              <span>{settingsT('appearance.preview')}</span>
            </>
          )}
        </Button>
      </div>
    </div>
  )
}
