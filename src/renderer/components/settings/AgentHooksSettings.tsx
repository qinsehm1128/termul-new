import type {
  AdapterStatus,
  AdapterStatusDto,
  HookAction,
  HookPreviewDto
} from '@shared/types/agent-hooks.types'
import { Info } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogTitle
} from '@/components/ui/alert-dialog'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { agentHooksApi } from '@/lib/agent-hooks-api'
import { logFrontendError } from '@/lib/log-api'
import { isTauriContext } from '@/lib/tauri-runtime'

const STATE_LABELS: Record<AdapterStatus, string> = {
  not_installed: 'Not installed',
  installed: 'Installed',
  outdated: 'Outdated',
  conflict: 'Conflict',
  source_missing: 'Source missing',
  invalid: 'Invalid'
}

/** Reloads definitions on mount. Probing and host mutations are never automatic. */
export function AgentHooksSettings(): React.JSX.Element {
  const { t } = useTranslation('settings')
  const text = (key: string, fallback: string) => t(`agentHooks.${key}`, { defaultValue: fallback })
  const [adapters, setAdapters] = useState<AdapterStatusDto[]>([])
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [preview, setPreview] = useState<HookPreviewDto | null>(null)
  const locked = useRef(false)
  const mounted = useRef(false)
  const trigger = useRef<HTMLButtonElement | null>(null)

  const reportFailure = (operation: string, cause: unknown, fallback: string) => {
    if (mounted.current) {
      setError(
        cause &&
          typeof cause === 'object' &&
          'code' in cause &&
          'message' in cause &&
          typeof cause.message === 'string'
          ? cause.message
          : fallback
      )
    }
    // Boundary failures only; never forward tokens, paths, diffs or arbitrary thrown payloads.
    void logFrontendError({
      source: 'AgentHooksSettings',
      message: `Agent hooks ${operation} failed`
    })
  }

  const reload = useCallback(async () => {
    if (locked.current) return
    locked.current = true
    setLoading(true)
    setError(null)
    try {
      const response = await agentHooksApi.reload()
      if (!Array.isArray(response)) throw new Error('Invalid adapter status response')
      if (mounted.current) setAdapters(response)
    } catch {
      if (mounted.current)
        setError(t('agentHooks.reloadFailed', { defaultValue: 'Unable to reload adapters' }))
      void logFrontendError({ source: 'AgentHooksSettings', message: 'Agent hooks reload failed' })
    } finally {
      locked.current = false
      if (mounted.current) setLoading(false)
    }
  }, [t])

  useEffect(() => {
    mounted.current = true
    void reload()
    return () => {
      mounted.current = false
    }
  }, [reload])

  const showPreview = async (adapter: AdapterStatusDto, action: HookAction) => {
    if (locked.current) return
    locked.current = true
    setBusy(true)
    setError(null)
    try {
      const next = await agentHooksApi.preview({
        adapter_id: adapter.id,
        action,
        event_options: {}
      })
      // Do not permit confirmation of a mismatched, empty or legacy preview response.
      if (
        next.adapter_id !== adapter.id ||
        next.action !== action ||
        !next.preview_id ||
        !next.fingerprint ||
        !Array.isArray(next.files) ||
        !Array.isArray(next.keys) ||
        next.files.length + next.keys.length === 0
      ) {
        throw new Error(text('noChanges', 'No changes to confirm'))
      }
      if (mounted.current) setPreview(next)
    } catch (cause) {
      reportFailure('preview', cause, text('previewFailed', 'Unable to preview changes'))
    } finally {
      locked.current = false
      if (mounted.current) setBusy(false)
    }
  }

  const confirm = async () => {
    if (!preview || locked.current) return
    locked.current = true
    setBusy(true)
    setError(null)
    const request = {
      adapter_id: preview.adapter_id,
      preview_id: preview.preview_id,
      fingerprint: preview.fingerprint
    }
    try {
      const result =
        preview.action === 'uninstall'
          ? await agentHooksApi.uninstall(request)
          : await agentHooksApi.install(request)
      if (mounted.current) {
        setAdapters((rows) =>
          rows.map((row) => (row.id === result.status.id ? result.status : row))
        )
      }
    } catch (cause) {
      reportFailure('confirm', cause, text('mutationFailed', 'Unable to apply changes'))
    } finally {
      // A rejected/stale token is never reused; the next attempt requires a fresh preview.
      if (mounted.current) {
        setPreview(null)
        setBusy(false)
      }
      locked.current = false
    }
  }

  const openFolder = async () => {
    if (locked.current || !isTauriContext()) return
    locked.current = true
    setBusy(true)
    setError(null)
    try {
      await agentHooksApi.openFolder()
    } catch (cause) {
      reportFailure(
        'open folder',
        cause,
        text('openFolderFailed', 'Unable to open adapters folder')
      )
    } finally {
      locked.current = false
      if (mounted.current) setBusy(false)
    }
  }
  const disabled = loading || busy || preview !== null
  const diagnosticList = (diagnostics: AdapterStatusDto['diagnostics']) =>
    diagnostics.length > 0 && (
      <ul
        className="space-y-1 text-xs text-destructive"
        aria-label={text('diagnostics', 'Diagnostics')}
      >
        {diagnostics.map((diagnostic, index) => (
          <li key={`${diagnostic.code}-${index}`}>
            <code>{diagnostic.code}</code>: {diagnostic.message}
          </li>
        ))}
      </ul>
    )
  const contents = (value: string | null) =>
    value === null ? (
      <span className="text-muted-foreground">{text('absent', 'Absent')}</span>
    ) : value === '' ? (
      <span className="text-muted-foreground">{text('empty', 'Empty')}</span>
    ) : (
      <pre className="overflow-x-auto whitespace-pre-wrap break-all">{value}</pre>
    )
  const beforeAfter = (before: string | null, after: string | null) => (
    <dl className="grid gap-1 text-xs">
      <dt className="font-medium">{text('before', 'Before')}</dt>
      <dd>{contents(before)}</dd>
      <dt className="font-medium">{text('after', 'After')}</dt>
      <dd>{contents(after)}</dd>
    </dl>
  )
  const previewName =
    adapters.find((adapter) => adapter.id === preview?.adapter_id)?.display_name ??
    preview?.adapter_id

  return (
    <section
      className="space-y-3"
      aria-label={text('title', 'Agent integrations')}
      aria-busy={loading || busy}
    >
      <div className="flex flex-wrap items-center gap-3">
        <h3 className="text-sm font-medium">{text('title', 'Agent integrations')}</h3>
        <TooltipProvider>
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                type="button"
                aria-label={text('helpLabel', 'About agent hooks')}
                className="text-muted-foreground"
              >
                <Info size={14} aria-hidden="true" />
              </button>
            </TooltipTrigger>
            <TooltipContent className="max-w-xs">
              {text(
                'help',
                'Hooks report terminal activity. Changes require confirmation. Restart agents after installation.'
              )}
            </TooltipContent>
          </Tooltip>
        </TooltipProvider>
        <button
          type="button"
          disabled={disabled}
          onClick={() => void reload()}
          className="text-xs underline disabled:opacity-50"
        >
          {text('reload', 'Reload')}
        </button>
        {isTauriContext() && (
          <button
            type="button"
            disabled={disabled}
            onClick={() => void openFolder()}
            className="text-xs underline disabled:opacity-50"
          >
            {text('openFolder', 'Open folder')}
          </button>
        )}
      </div>
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      )}
      {loading && (
        <p role="status" className="text-xs text-muted-foreground">
          {text('loading', 'Loading…')}
        </p>
      )}
      {!loading && !error && adapters.length === 0 && (
        <p className="text-xs text-muted-foreground">{text('emptyAdapters', 'No adapters')}</p>
      )}
      {adapters.map((adapter) => {
        const installed =
          adapter.install_id !== null ||
          adapter.installed_version !== null ||
          adapter.state === 'installed' ||
          adapter.state === 'outdated'
        const canInstall = adapter.state === 'not_installed' || adapter.state === 'outdated'
        const installLabel =
          adapter.state === 'outdated' ? text('upgrade', 'Upgrade') : text('install', 'Install')
        return (
          <article
            key={adapter.id}
            aria-label={adapter.display_name}
            className="space-y-2 rounded-md border border-border p-3"
          >
            <div className="flex flex-wrap items-center gap-2 text-sm">
              <strong>{adapter.display_name}</strong>
              <span className="rounded border px-1.5 text-xs text-muted-foreground">
                {text(
                  `source.${adapter.origin}`,
                  adapter.origin === 'builtin' ? 'Built-in' : 'Custom'
                )}
              </span>
              <span className="ml-auto text-xs">
                {text(`states.${adapter.state}`, STATE_LABELS[adapter.state])}
              </span>
            </div>
            <div className="flex flex-wrap gap-3 text-xs text-muted-foreground">
              <code>{adapter.id}</code>
              {adapter.version !== null && (
                <span>
                  {text('version', 'Version')}: {adapter.version}
                </span>
              )}
              {adapter.installed_version !== null && (
                <span>
                  {text('installedVersion', 'Installed version')}: {adapter.installed_version}
                </span>
              )}
            </div>
            {diagnosticList(adapter.diagnostics)}
            <div className="flex gap-3 text-xs">
              <button
                type="button"
                className="underline disabled:opacity-50"
                disabled={disabled || !canInstall}
                aria-label={`${installLabel} ${adapter.display_name}`}
                onClick={(event) => {
                  trigger.current = event.currentTarget
                  void showPreview(adapter, 'install')
                }}
              >
                {installLabel}
              </button>
              <button
                type="button"
                className="underline disabled:opacity-50"
                disabled={disabled || !installed}
                aria-label={`${text('uninstall', 'Uninstall')} ${adapter.display_name}`}
                onClick={(event) => {
                  trigger.current = event.currentTarget
                  void showPreview(adapter, 'uninstall')
                }}
              >
                {text('uninstall', 'Uninstall')}
              </button>
            </div>
          </article>
        )
      })}
      <AlertDialog
        open={preview !== null}
        onOpenChange={(open) => {
          if (!open && !locked.current) setPreview(null)
        }}
      >
        <AlertDialogContent
          className="max-h-[85vh] overflow-y-auto"
          onCloseAutoFocus={(event) => {
            event.preventDefault()
            trigger.current?.focus()
          }}
        >
          <AlertDialogTitle>
            {text('previewTitle', 'Review hook changes')} · {previewName}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {preview?.action === 'uninstall'
              ? text(
                  'uninstallConsequence',
                  'Remove this integration and restore the original configuration. New hook activity will no longer be reported.'
                )
              : text('previewHint', 'Only the changes below will be applied after confirmation.')}
          </AlertDialogDescription>
          {preview && (
            <div className="space-y-3 break-all">
              {diagnosticList(preview.diagnostics)}
              <dl className="grid gap-1 text-xs" aria-label={text('capabilities', 'Capabilities')}>
                <dt className="font-medium">{text('chat', 'Chat')}</dt>
                <dd>
                  {preview.capabilities.chat
                    ? text('available', 'Available')
                    : text('unavailable', 'Unavailable')}
                </dd>
                <dt className="font-medium">{text('resume', 'Resume')}</dt>
                <dd>
                  {preview.capabilities.resume
                    ? text('available', 'Available')
                    : text('unavailable', 'Unavailable')}
                </dd>
                {preview.capabilities.events.map((capability) => (
                  <div key={capability.native_event}>
                    <dt>
                      <code>{capability.native_event}</code>
                    </dt>
                    <dd>
                      {capability.hook_kinds.join(', ')} ·{' '}
                      {capability.available
                        ? capability.enabled_by_default
                          ? text('enabled', 'Enabled')
                          : text('disabled', 'Disabled')
                        : text('unavailable', 'Unavailable')}
                    </dd>
                  </div>
                ))}
              </dl>
              {preview.files.length > 0 && (
                <h4 className="text-sm font-medium">{text('files', 'Files')}</h4>
              )}
              {preview.files.map((file, index) => (
                <div key={`${file.path}-${index}`} className="space-y-2 rounded border p-2">
                  <code className="text-xs">{file.path}</code>
                  {file.mode !== null && (
                    <p className="text-xs">
                      {text('mode', 'Mode')}: <code>{file.mode}</code>
                    </p>
                  )}
                  {beforeAfter(file.before, file.after)}
                </div>
              ))}
              {preview.keys.length > 0 && (
                <h4 className="text-sm font-medium">{text('keys', 'Configuration keys')}</h4>
              )}
              {preview.keys.map((key, index) => (
                <div key={`${key.path}-${index}`} className="space-y-2 rounded border p-2">
                  <code className="block text-xs">{key.path}</code>
                  <code className="block text-xs">{JSON.stringify(key.key)}</code>
                  {beforeAfter(key.before, key.after)}
                </div>
              ))}
            </div>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={busy}>{text('cancel', 'Cancel')}</AlertDialogCancel>
            <AlertDialogAction
              disabled={busy}
              onClick={(event) => {
                event.preventDefault()
                void confirm()
              }}
            >
              {preview?.action === 'uninstall'
                ? text('confirmUninstall', 'Confirm uninstall')
                : text('confirmInstall', 'Confirm install')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  )
}
