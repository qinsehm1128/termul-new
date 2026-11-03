import { Puzzle } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from '@/components/ui/alert-dialog'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { PluginSettingsForm } from '@/features/plugins/PluginSettingsForm'
import { logFrontendError } from '@/lib/log-api'
import { tauriDialogApi } from '@/lib/tauri-dialog-api'
import { isTauriContext } from '@/lib/tauri-runtime'
import { usePluginsStore } from '@/stores/plugins-store'

type Confirmation =
  | { type: 'install'; path: string }
  | { type: 'upgrade'; id: string; path: string }
  | { type: 'uninstall'; id: string }

export default function PluginsPage(): React.JSX.Element {
  const { t } = useTranslation('plugins')
  const desktop = isTauriContext()
  const { plugins, connected, loading, busy, error, install, setEnabled, reload, uninstall, load } =
    usePluginsStore()
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null)
  const [picking, setPicking] = useState(false)
  const [pickerFailed, setPickerFailed] = useState(false)
  const disabled = !!busy || loading || !connected || picking

  async function chooseFile(id?: string): Promise<void> {
    if (!desktop || disabled) return
    setPicking(true)
    setPickerFailed(false)
    try {
      const selected = await tauriDialogApi.selectFile({
        filters: [{ name: t('manager.fileType'), extensions: ['dylib', 'so', 'dll'] }]
      })
      if (!selected.success) throw new Error('Plugin file selection failed')
      const path = selected.data
      if (typeof path === 'string') {
        setConfirmation(id ? { type: 'upgrade', id, path } : { type: 'install', path })
      }
    } catch {
      setPickerFailed(true)
      void logFrontendError({
        level: 'warn',
        source: 'plugins-page',
        message: 'Plugin file selection failed'
      })
    } finally {
      setPicking(false)
    }
  }

  function confirm(): void {
    if (!confirmation || disabled) return
    if (confirmation.type === 'install') void install(confirmation.path)
    else if (confirmation.type === 'upgrade') void reload(confirmation.id, confirmation.path)
    else void uninstall(confirmation.id)
    setConfirmation(null)
  }

  return (
    <div className="flex h-full flex-col overflow-auto bg-background">
      <header className="border-b border-border px-6 py-4">
        <div className="flex items-center gap-2">
          <Puzzle size={16} className="text-primary" aria-hidden="true" />
          <h1 className="text-lg font-medium text-foreground">{t('manager.title')}</h1>
        </div>
        <p className="mt-1 text-sm text-muted-foreground">{t('manager.subtitle')}</p>
      </header>
      <div className="mx-auto w-full max-w-6xl space-y-4 p-6">
        {!desktop ? (
          <p role="status" className="text-sm text-muted-foreground">
            {t('manager.unsupported')}
          </p>
        ) : (
          <>
            <div className="flex flex-wrap gap-2">
              <Button disabled={disabled} onClick={() => void chooseFile()}>
                {t('manager.install')}
              </Button>
              <Button variant="outline" disabled={!!busy || loading} onClick={() => void load()}>
                {t('manager.refresh')}
              </Button>
            </div>
            {!connected && <p role="status">{t('state.disconnected')}</p>}
            {loading && <p role="status">{t('view.loading')}</p>}
            {(error || pickerFailed) && (
              <p role="alert" className="text-sm text-destructive">
                {t('errors.generic')}
                {error ? ` (${error.code})` : ''}
              </p>
            )}
            {!loading && plugins.length === 0 && (
              <p className="text-sm text-muted-foreground">{t('manager.empty')}</p>
            )}
            {plugins.map((plugin) => {
              const unavailable =
                disabled || ['pending', 'activating', 'reloading'].includes(plugin.state)
              return (
                <section
                  key={plugin.id}
                  aria-label={plugin.manifest.name}
                  className="space-y-4 rounded-lg border border-border p-4"
                >
                  <div className="flex flex-wrap items-center gap-2">
                    <h2 className="font-medium text-foreground">{plugin.manifest.name}</h2>
                    <Badge variant="secondary">{t(`state.${plugin.state}`)}</Badge>
                    <span className="text-sm text-muted-foreground">
                      {t('manager.version')} {plugin.manifest.version}
                    </span>
                  </div>
                  <p className="break-all text-xs text-muted-foreground">{plugin.id}</p>
                  {plugin.lastError && (
                    <p role="alert" className="break-words text-sm text-destructive">
                      {t('manager.lastError')}: {plugin.lastError.code}: {plugin.lastError.message}
                    </p>
                  )}
                  {plugin.state === 'quarantined' && (
                    <p role="alert" className="text-sm text-destructive">
                      {t('manager.quarantineWarning')}
                    </p>
                  )}
                  <div className="flex flex-wrap gap-2">
                    <Button
                      variant="outline"
                      disabled={unavailable}
                      onClick={() => void setEnabled(plugin.id, !plugin.desiredEnabled)}
                    >
                      {t(plugin.desiredEnabled ? 'manager.disable' : 'manager.enable')}
                    </Button>
                    {(plugin.state === 'quarantined' || plugin.state === 'failed') && (
                      <Button
                        variant="outline"
                        disabled={unavailable}
                        onClick={() => void setEnabled(plugin.id, true)}
                      >
                        {t('manager.retry')}
                      </Button>
                    )}
                    <Button
                      variant="outline"
                      disabled={unavailable || plugin.state === 'quarantined'}
                      onClick={() => void reload(plugin.id)}
                    >
                      {t('manager.reload')}
                    </Button>
                    <Button
                      variant="outline"
                      disabled={unavailable}
                      onClick={() => void chooseFile(plugin.id)}
                    >
                      {t('manager.upgrade')}
                    </Button>
                    <Button
                      variant="destructive"
                      disabled={unavailable}
                      onClick={() => setConfirmation({ type: 'uninstall', id: plugin.id })}
                    >
                      {t('manager.uninstall')}
                    </Button>
                  </div>
                  {connected && plugin.state === 'active' && (
                    <>
                      <details>
                        <summary className="cursor-pointer text-sm font-medium">
                          {t('settings.title')}
                        </summary>
                        <PluginSettingsForm pluginId={plugin.id} />
                      </details>
                      {plugin.manifest.contributes.views.length > 0 && (
                        <nav aria-label={t('manager.views')} className="flex flex-wrap gap-4">
                          {plugin.manifest.contributes.views.map((view) => (
                            <Link
                              className="text-sm text-primary underline"
                              key={view.id}
                              to={`/plugins/${encodeURIComponent(plugin.id)}/views/${encodeURIComponent(view.id)}`}
                            >
                              {view.title}
                            </Link>
                          ))}
                        </nav>
                      )}
                    </>
                  )}
                </section>
              )
            })}
          </>
        )}
      </div>
      <AlertDialog
        open={confirmation !== null}
        onOpenChange={(value) => !value && setConfirmation(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t(
                confirmation?.type === 'uninstall'
                  ? 'manager.uninstall'
                  : confirmation?.type === 'upgrade'
                    ? 'manager.upgrade'
                    : 'manager.install'
              )}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(
                confirmation?.type === 'uninstall'
                  ? 'manager.uninstallWarning'
                  : 'manager.trustWarning'
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t('manager.cancel')}</AlertDialogCancel>
            <AlertDialogAction disabled={disabled} onClick={confirm}>
              {t('manager.confirm')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
