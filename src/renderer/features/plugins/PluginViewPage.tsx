import { useTranslation } from 'react-i18next'
import { useParams } from 'react-router-dom'
import { ErrorBoundary } from '@/components/ErrorBoundary'
import { Button } from '@/components/ui/button'
import { usePluginsStore } from '@/stores/plugins-store'
import { PluginViewPanel } from './PluginViewPanel'

export function PluginViewPage(): React.JSX.Element {
  const { pluginId, viewId } = useParams<{ pluginId: string; viewId: string }>()
  const { t } = useTranslation('plugins')
  const isWeb = usePluginsStore(
    (state) =>
      state.plugins
        .find((plugin) => plugin.id === pluginId)
        ?.manifest.contributes.views.find((view) => view.id === viewId)?.kind === 'web'
  )

  return (
    <div className="flex h-full min-h-0 flex-col overflow-auto bg-background text-foreground">
      <div
        className={
          isWeb
            ? 'flex h-full min-h-0 w-full flex-1 flex-col'
            : 'mx-auto w-full max-w-6xl p-3 sm:p-6'
        }
      >
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
