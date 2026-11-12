import { Download, Puzzle, RefreshCw, Search, WifiOff } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
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
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { PluginDetails } from '@/features/plugins/manager/PluginDetails'
import { PluginList } from '@/features/plugins/manager/PluginList'
import { logFrontendError } from '@/lib/log-api'
import { tauriDialogApi } from '@/lib/tauri-dialog-api'
import { isTauriContext } from '@/lib/tauri-runtime'
import { usePluginsStore } from '@/stores/plugins-store'

type Confirmation =
  | { type: 'install'; path: string }
  | { type: 'upgrade'; id: string; path: string }
  | { type: 'uninstall'; id: string }
type StatusFilter = 'all' | 'running' | 'disabled' | 'error'

export default function PluginsPage(): React.JSX.Element {
  const { t } = useTranslation('plugins')
  const desktop = isTauriContext()
  const { plugins, connected, loading, busy, error, install, setEnabled, reload, uninstall, load } =
    usePluginsStore()
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null)
  const [picking, setPicking] = useState(false)
  const [pickerFailed, setPickerFailed] = useState(false)
  const [query, setQuery] = useState('')
  const [status, setStatus] = useState<StatusFilter>('all')
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const disabled = !!busy || loading || !connected || picking
  const search = query.trim().toLocaleLowerCase()
  const filtered = plugins.filter((plugin) => {
    const matchesStatus =
      status === 'all' ||
      (status === 'running' && plugin.state === 'active') ||
      (status === 'disabled' && !plugin.desiredEnabled) ||
      (status === 'error' && ['quarantined', 'failed'].includes(plugin.state))
    return (
      matchesStatus &&
      [plugin.manifest.name, plugin.id, plugin.manifest.version].some((value) =>
        value.toLocaleLowerCase().includes(search)
      )
    )
  })
  // Reconcile selection against the authoritative snapshot and current filters.
  const selected = filtered.find((plugin) => plugin.id === selectedId) ?? filtered[0]

  async function chooseFile(id?: string): Promise<void> {
    if (!desktop || disabled) return
    setPicking(true)
    setPickerFailed(false)
    try {
      const selected = await tauriDialogApi.selectFile({
        filters: [{ name: t('manager.fileType'), extensions: ['seplugin', 'dylib', 'so', 'dll'] }]
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
      <header className="flex flex-wrap items-center justify-between gap-4 border-b border-border px-6 py-4">
        <div>
          <div className="flex items-center gap-2">
            <Puzzle size={16} className="text-primary" aria-hidden="true" />
            <h1 className="text-lg font-medium text-foreground">{t('manager.title')}</h1>
          </div>
          <p className="mt-1 text-sm text-muted-foreground">{t('manager.subtitle')}</p>
        </div>
        <div className="flex items-center gap-2">
          <Button variant="outline" disabled={!!busy || loading} onClick={() => void load()}>
            <RefreshCw size={14} aria-hidden="true" />
            {t('manager.refresh')}
          </Button>
          {desktop && (
            <Button disabled={disabled} onClick={() => void chooseFile()}>
              <Download size={14} aria-hidden="true" />
              {t('manager.install')}
            </Button>
          )}
        </div>
      </header>
      <div className="mx-auto w-full max-w-6xl space-y-4 p-6">
        <>
          {!desktop && (
            <p role="status" className="text-sm text-muted-foreground">
              {t('manager.managedOnDesktop')}
            </p>
          )}
          {!connected && (
            <div
              role="status"
              className="flex flex-wrap items-center gap-3 rounded-md border border-border bg-muted/40 p-4 text-sm"
            >
              <WifiOff size={16} className="shrink-0 text-muted-foreground" aria-hidden="true" />
              <p className="min-w-0 flex-1">{t('state.disconnected')}</p>
              <Button
                variant="outline"
                size="sm"
                disabled={!!busy || loading}
                onClick={() => void load()}
              >
                {t('manager.reconnect')}
              </Button>
            </div>
          )}
          {loading && (
            <p role="status" className="text-sm text-muted-foreground">
              {t('view.loading')}
            </p>
          )}
          {(error || pickerFailed) && (
            <p role="alert" className="text-sm text-destructive">
              {error?.code === 'PLUGIN_CORE_UNAVAILABLE'
                ? t('manager.coreUnavailable')
                : t('errors.generic')}
              {error && error.code !== 'PLUGIN_CORE_UNAVAILABLE' ? ` (${error.code})` : ''}
            </p>
          )}
          <div className="flex flex-wrap items-center gap-3">
            <div className="relative min-w-48 flex-1">
              <Search
                size={15}
                className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground"
                aria-hidden="true"
              />
              <Input
                type="search"
                aria-label={t('manager.search')}
                placeholder={t('manager.search')}
                className="pl-9"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
              />
            </div>
            <select
              aria-label={t('manager.statusFilter')}
              value={status}
              onChange={(event) => setStatus(event.target.value as StatusFilter)}
              className="h-9 rounded-md border border-input bg-background px-3 text-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            >
              {(['all', 'running', 'disabled', 'error'] as const).map((filter) => (
                <option key={filter} value={filter}>
                  {t(`manager.filters.${filter}`)}
                </option>
              ))}
            </select>
          </div>
          {!loading && plugins.length === 0 ? (
            <div className="flex min-h-80 flex-col items-center justify-center gap-3 rounded-lg border border-dashed border-border p-8 text-center">
              <Puzzle size={32} className="mb-1 text-muted-foreground" aria-hidden="true" />
              <h2 className="text-base font-medium">{t('manager.empty')}</h2>
              <p className="max-w-md text-sm leading-relaxed text-muted-foreground">
                {t('manager.emptyDescription')}
              </p>
              {desktop && (
                <Button variant="outline" disabled={disabled} onClick={() => void chooseFile()}>
                  <Download size={14} aria-hidden="true" />
                  {t('manager.installFirst')}
                </Button>
              )}
            </div>
          ) : loading && plugins.length === 0 ? null : filtered.length === 0 ? (
            <div className="rounded-lg border border-border p-10 text-center">
              <p className="text-sm text-muted-foreground">{t('manager.noResults')}</p>
              <Button
                variant="ghost"
                className="mt-2"
                onClick={() => {
                  setQuery('')
                  setStatus('all')
                }}
              >
                {t('manager.clearFilters')}
              </Button>
            </div>
          ) : (
            <div className="grid overflow-hidden rounded-lg border border-border md:grid-cols-[minmax(15rem,0.8fr)_minmax(0,1.7fr)]">
              <aside className="max-h-[60vh] overflow-y-auto border-b border-border md:max-h-none md:border-b-0 md:border-r">
                <div className="border-b border-border/60 px-4 py-3 text-xs font-medium text-muted-foreground">
                  {t('manager.installed')} · {filtered.length}
                </div>
                <PluginList
                  plugins={filtered}
                  selectedId={selected?.id}
                  disabled={disabled}
                  manageable={desktop}
                  onSelect={setSelectedId}
                  onToggle={(id, enabled) => void setEnabled(id, enabled)}
                />
              </aside>
              {selected && (
                <PluginDetails
                  key={selected.id}
                  plugin={selected}
                  plugins={plugins}
                  connected={connected}
                  disabled={disabled}
                  manageable={desktop}
                  onRetry={() => void setEnabled(selected.id, true)}
                  onReload={() => void reload(selected.id)}
                  onUpgrade={() => void chooseFile(selected.id)}
                  onUninstall={() => setConfirmation({ type: 'uninstall', id: selected.id })}
                />
              )}
            </div>
          )}
        </>
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
