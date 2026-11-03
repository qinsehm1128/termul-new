import { useTranslation } from 'react-i18next'
import { useParams } from 'react-router-dom'
import { ErrorBoundary } from '@/components/ErrorBoundary'
import { Button } from '@/components/ui/button'
import { PluginViewPanel } from './PluginViewPanel'

export function PluginViewPage(): React.JSX.Element {
  const { pluginId, viewId } = useParams<{ pluginId: string; viewId: string }>()
  const { t } = useTranslation('plugins')

  return (
    <div className="flex h-full flex-col overflow-auto bg-background text-foreground">
      <div className="mx-auto w-full max-w-6xl p-6">
        {pluginId && viewId ? (
          <ErrorBoundary
            key={`${pluginId}/${viewId}`}
            fallback={(_error, retry) => (
              <div className="flex flex-col gap-3">
                <p role="alert" className="text-destructive">
                  {t('errors.generic')}
                </p>
                <Button variant="outline" onClick={retry}>
                  {t('view.retry')}
                </Button>
              </div>
            )}
          >
            <PluginViewPanel pluginId={pluginId} viewId={viewId} />
          </ErrorBoundary>
        ) : (
          <p role="alert" className="text-destructive">
            {t('view.unavailable')}
          </p>
        )}
      </div>
    </div>
  )
}

export default PluginViewPage
