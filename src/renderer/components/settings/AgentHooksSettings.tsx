import {
  type AgentKind,
  HOOK_AGENT_KINDS,
  type HookInstallStatus
} from '@shared/types/agent-hooks.types'
import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { ConfirmDialog } from '@/components/ConfirmDialog'
import { agentHooksApi } from '@/lib/agent-hooks-api'

const LABELS: Record<AgentKind, string> = {
  'claude-code': 'Claude Code',
  pi: 'pi',
  'qin-code': 'qin-code'
}

/** Shared by desktop and browser preferences. Mounting only probes; it never installs. */
export function AgentHooksSettings(): React.JSX.Element {
  const { t } = useTranslation('settings')
  const [statuses, setStatuses] = useState<HookInstallStatus[]>([])
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<AgentKind | null>(null)
  const [preview, setPreview] = useState<HookInstallStatus | null>(null)
  const [previewDiff, setPreviewDiff] = useState('')
  const showPreview = async (status: HookInstallStatus) => {
    if (busy) return
    setBusy(status.agent)
    setError(null)
    setPreview(null)
    setPreviewDiff('')
    try {
      const diff = await agentHooksApi.preview(status.agent)
      if (!diff.trim()) throw new Error('Hook preview is empty; installation was not started.')
      setPreviewDiff(diff)
      setPreview(status)
    } catch (err) {
      setError(String(err))
    } finally {
      setBusy(null)
    }
  }
  const refresh = useCallback(async () => {
    setLoading(true)
    try {
      const response = await agentHooksApi.status()
      setStatuses(Array.isArray(response) ? response : [])
      setError(null)
    } catch (err) {
      setError(String(err))
    } finally {
      setLoading(false)
    }
  }, [])
  useEffect(() => {
    void refresh()
  }, [refresh])

  const mutate = async (status: HookInstallStatus, uninstall: boolean) => {
    if (busy) return
    setBusy(status.agent)
    setError(null)
    try {
      if (uninstall) await agentHooksApi.uninstall(status.agent)
      else await agentHooksApi.install(status.agent)
      setPreview(null)
      await refresh()
    } catch (err) {
      setError(String(err))
    } finally {
      setBusy(null)
    }
  }
  const text = (key: string, fallback: string) => t(`agentHooks.${key}`, { defaultValue: fallback })
  return (
    <section className="space-y-3" aria-label={text('title', 'Agent integrations')}>
      <h3 className="text-sm font-medium">{text('title', 'Agent integrations')}</h3>
      <p className="text-xs text-muted-foreground">
        {text(
          'hint',
          'Hooks report terminal agent activity. Startup only checks status; configuration changes require your confirmation. Restart existing agents after installing.'
        )}
      </p>
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      )}
      <button
        type="button"
        disabled={loading || !!busy}
        onClick={() => void refresh()}
        className="text-xs underline"
      >
        {text('refresh', 'Refresh status')}
      </button>
      {HOOK_AGENT_KINDS.map((agent) => {
        const status = statuses.find((row) => row.agent === agent)
        const installed = status?.state === 'installed' || status?.state === 'outdated'
        return (
          <div key={agent} className="rounded-md border border-border p-3 space-y-2">
            <div className="flex justify-between gap-2 text-sm">
              <strong>{LABELS[agent]}</strong>
              <span>
                {status?.state ??
                  (loading ? text('loading', 'Loading…') : text('unknown', 'Unavailable'))}
              </span>
            </div>
            {status && (
              <>
                <p className="text-xs text-muted-foreground break-all">
                  {status.cli_path}
                  {status.version ? ` · ${status.version}` : ''}
                </p>
                <p className="text-xs break-all">
                  <span>{text('configPath', 'Configuration file')}: </span>
                  <code>{status.config_path}</code>
                </p>
                <p className="text-xs text-muted-foreground">{status.detail}</p>
                <div className="flex gap-3 text-xs">
                  <button
                    type="button"
                    className="underline"
                    disabled={!!busy || !status.config_path || status.state === 'agent_not_found'}
                    onClick={() => void showPreview(status)}
                  >
                    {installed ? text('reinstall', 'Reinstall') : text('install', 'Install')}
                  </button>
                  <button
                    type="button"
                    className="underline"
                    disabled={!!busy || !status.config_path || status.state === 'agent_not_found'}
                    onClick={() => void showPreview(status)}
                  >
                    {text('viewChanges', 'View changes')}
                  </button>
                  <button
                    type="button"
                    className="underline"
                    disabled={!!busy || !installed}
                    onClick={() => void mutate(status, true)}
                  >
                    {text('uninstall', 'Uninstall')}
                  </button>
                </div>
              </>
            )}
          </div>
        )
      })}
      <ConfirmDialog
        isOpen={preview !== null}
        title={text('previewTitle', 'Preview hook configuration change')}
        message={text(
          'previewHint',
          'This changes the host configuration file below. Only app-managed hook entries are added or replaced; unrelated hooks are preserved. Uninstall removes only app-managed entries. No changes happen until you confirm.'
        )}
        confirmLabel={text('confirm', 'Confirm install')}
        isLoading={!!busy}
        onCancel={() => {
          if (!busy) setPreview(null)
        }}
        onConfirm={() => {
          if (preview && !busy) void mutate(preview, false)
        }}
      >
        {preview && (
          <div className="my-3 space-y-2 text-xs break-all">
            <strong>{LABELS[preview.agent]}</strong>
            <p>
              <code>{preview.config_path}</code>
            </p>
            <pre className="whitespace-pre-wrap rounded border p-2">{previewDiff}</pre>
            <p>{preview.detail}</p>
          </div>
        )}
      </ConfirmDialog>
    </section>
  )
}
