import { lazy, Suspense, useLayoutEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { flushEditorContent } from '@/lib/editor-content-flush'
import { logFrontendError } from '@/lib/log-api'

const EditorPanel = lazy(() =>
  import('@/components/editor/EditorPanel').then((module) => ({ default: module.EditorPanel }))
)

/** Full-width editor content; the owning workspace handles tab close safety. */
export function QuickTerminalFilePreview({ path }: { path: string }): React.JSX.Element {
  const { t } = useTranslation('quickTerminal')
  const name = path.split(/[\\/]/).pop() || path
  useLayoutEffect(
    () => () => {
      // Layout cleanup precedes CodeEditor's passive flusher unregister on switch/navigation.
      void flushEditorContent(path).catch(() => {
        void logFrontendError({
          level: 'warn',
          source: 'quick-terminal-workspace',
          message: 'operation=editor-flush code=EDITOR_FAILED'
        }).catch(() => undefined)
      })
    },
    [path]
  )
  return (
    <section aria-label={t('preview', { name })} className="relative h-full min-h-0 min-w-0">
      <Suspense fallback={null}>
        <EditorPanel key={path} filePath={path} isVisible />
      </Suspense>
    </section>
  )
}
