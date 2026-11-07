/** Independent desktop MCP connections; source and presentation mode are orthogonal. */

import { MCP_GATEWAY_SOURCES, type McpGatewaySource } from '@shared/types/mcp-gateway.types'

import { ChevronDown, Copy, Loader2, Play, RotateCw } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Input } from '@/components/ui/input'
import { clipboardApi } from '@/lib/api'
import {
  codexToml,
  httpServerJson,
  MCP_GATEWAY_MODES,
  type McpClient,
  mcpServersJson
} from '@/lib/mcp-gateway-api'
import { isTauriContext } from '@/lib/tauri-runtime'
import { cn } from '@/lib/utils'
import { type McpGatewaySourceState, useMcpGatewayStore } from '@/stores/mcp-gateway-store'

const VIEW_POLL_MS = 4000

function canExportHttp(source: McpGatewaySource, state: McpGatewaySourceState): boolean {
  const { connection, view, mode } = state
  return (
    connection?.source === source &&
    connection.mode === mode &&
    connection.httpReady === true &&
    view?.source === source &&
    view.state === 'running' &&
    Number.isInteger(view.port) &&
    view.port !== null &&
    view.port > 0 &&
    view.port <= 65535 &&
    view.port === connection.port &&
    (!view.status ||
      (view.status.source === source &&
        view.status.protocol === 'se-mcp-gateway' &&
        view.status.protocolVersion === 1))
  )
}

function GatewayStatus({ source }: { source: McpGatewaySource }): React.JSX.Element {
  const { t } = useTranslation('mcp')
  const candidate = useMcpGatewayStore((state) => state.sources[source].view)
  const view = candidate?.source === source ? candidate : null
  const busy = useMcpGatewayStore((state) => state.sources[source].busy)
  const start = useMcpGatewayStore((state) => state.start)
  const restart = useMcpGatewayStore((state) => state.restart)
  const setPort = useMcpGatewayStore((state) => state.setPort)
  const [portDraft, setPortDraft] = useState('')

  const port = view?.port ?? null
  useEffect(() => {
    if (port !== null) setPortDraft(String(port))
  }, [port])

  const running = view?.state === 'running'
  const upstreams = view?.status?.upstreams ?? []
  const count = (state: string) => upstreams.filter((item) => item.state === state).length
  const draft = Number(portDraft)
  const portValid = Number.isInteger(draft) && draft >= 1024 && draft <= 65535

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <span
          aria-hidden="true"
          className={cn(
            'size-2 shrink-0 rounded-full',
            running
              ? 'bg-emerald-500'
              : view?.state === 'error'
                ? 'bg-red-500'
                : 'bg-muted-foreground/40'
          )}
        />
        <span className="text-sm text-foreground">
          {running
            ? t('gateway.running', { version: view?.status?.version ?? '' })
            : view?.state === 'error'
              ? t('gateway.error')
              : t('gateway.stopped')}
        </span>
        {running ? (
          <span className="text-xs text-muted-foreground">
            {t('gateway.counts', {
              connected: count('connected'),
              failed: count('failed'),
              connecting: count('connecting')
            })}
          </span>
        ) : null}
        <div className="ml-auto flex items-center gap-2">
          {running ? (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={busy !== null}
              onClick={() => void restart(source)}
            >
              {busy === 'restart' ? <Loader2 className="animate-spin" /> : <RotateCw />}
              {t('gateway.restart')}
            </Button>
          ) : (
            <Button
              type="button"
              size="sm"
              disabled={busy !== null}
              onClick={() => void start(source)}
            >
              {busy === 'start' ? <Loader2 className="animate-spin" /> : <Play />}
              {t('gateway.start')}
            </Button>
          )}
        </div>
      </div>
      {view?.error ? (
        <p className="text-xs text-destructive wrap-anywhere">{t('gateway.error')}</p>
      ) : null}
      {view?.status?.configError ? (
        <p className="text-xs text-destructive wrap-anywhere">
          {t('gateway.configError', { error: t('gateway.error') })}
        </p>
      ) : null}
      <form
        className="flex items-center gap-2"
        onSubmit={(event) => {
          event.preventDefault()
          if (portValid && draft !== port) void setPort(source, draft)
        }}
      >
        <label htmlFor={`mcp-gateway-port-${source}`} className="text-xs text-muted-foreground">
          {t('gateway.port')}
        </label>
        <Input
          id={`mcp-gateway-port-${source}`}
          inputMode="numeric"
          className="h-7 w-24 font-mono"
          value={portDraft}
          onChange={(event) => setPortDraft(event.target.value.replace(/[^0-9]/g, ''))}
        />
        <Button
          type="submit"
          size="sm"
          variant="outline"
          disabled={!portValid || draft === port || busy !== null}
        >
          {busy === 'port' ? <Loader2 className="animate-spin" /> : null}
          {t('gateway.applyPort')}
        </Button>
        <span
          className="min-w-0 truncate text-2xs text-muted-foreground"
          title={view?.settingsPath}
        >
          {t('gateway.recordedIn', { path: view?.settingsPath ?? '' })}
        </span>
      </form>
    </div>
  )
}

function ModePicker({ source }: { source: McpGatewaySource }): React.JSX.Element {
  const { t } = useTranslation('mcp')
  const mode = useMcpGatewayStore((state) => state.sources[source].mode)
  const busy = useMcpGatewayStore((state) => state.sources[source].busy)
  const setMode = useMcpGatewayStore((state) => state.setMode)
  return (
    <fieldset aria-label={t('gateway.modeTitle')} className="grid gap-2 sm:grid-cols-3">
      {MCP_GATEWAY_MODES.map((option) => (
        <button
          key={option}
          type="button"
          aria-pressed={mode === option}
          disabled={busy !== null}
          onClick={() => void setMode(source, option)}
          className={cn(
            'rounded-md border p-3 text-left transition-colors',
            mode === option
              ? 'border-foreground/60 bg-secondary'
              : 'border-border hover:bg-secondary/50'
          )}
        >
          <p className="text-sm font-medium text-foreground">{t(`gateway.modes.${option}.name`)}</p>
          <p className="mt-1 text-2xs leading-relaxed text-muted-foreground">
            {t(`gateway.modes.${option}.description`)}
          </p>
        </button>
      ))}
    </fieldset>
  )
}

type ClientStateLabel = 'connected' | 'stale' | 'notConnected' | 'notInstalled'

function clientState(client: McpClient): {
  label: ClientStateLabel
  tone: 'ok' | 'stale' | 'none'
} {
  if (client.synced && client.upToDate) return { label: 'connected', tone: 'ok' }
  if (client.synced) return { label: 'stale', tone: 'stale' }
  return { label: client.installed ? 'notConnected' : 'notInstalled', tone: 'none' }
}

function ClientRow({
  client,
  source
}: {
  client: McpClient
  source: McpGatewaySource
}): React.JSX.Element {
  const { t } = useTranslation('mcp')
  const busy = useMcpGatewayStore((state) => state.sources[source].busy)
  const syncClient = useMcpGatewayStore((state) => state.syncClient)
  const unsyncClient = useMcpGatewayStore((state) => state.unsyncClient)
  const working = busy === `sync:${client.id}`
  const state = clientState(client)
  const blocked = busy !== null || Boolean(client.error)

  const sync = async (): Promise<void> => {
    if (await syncClient(source, client.id))
      toast.success(t('gateway.clients.synced', { name: client.name }))
  }
  const unsync = async (): Promise<void> => {
    if (await unsyncClient(source, client.id))
      toast.success(t('gateway.clients.removed', { name: client.name }))
  }

  return (
    <li className="flex flex-wrap items-center gap-2 px-3 py-2">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium text-foreground">{client.name}</span>
          <Badge
            variant={state.tone === 'none' ? 'outline' : 'secondary'}
            className={cn(
              state.tone === 'ok' && 'text-emerald-600 dark:text-emerald-400',
              state.tone === 'stale' && 'text-amber-600 dark:text-amber-400'
            )}
          >
            {t(`gateway.clients.state.${state.label}`)}
          </Badge>
        </div>
        <p className="truncate font-mono text-2xs text-muted-foreground" title={client.configPath}>
          {client.configPath}
        </p>
        {client.error ? <p className="text-2xs text-destructive">{t('gateway.error')}</p> : null}
      </div>
      {working ? <Loader2 className="size-3.5 animate-spin text-muted-foreground" /> : null}
      {client.synced ? (
        <>
          {!client.upToDate ? (
            <Button type="button" size="xs" disabled={blocked} onClick={() => void sync()}>
              {t('gateway.clients.update')}
            </Button>
          ) : null}
          <Button
            type="button"
            size="xs"
            variant="outline"
            disabled={blocked}
            onClick={() => void unsync()}
          >
            {t('gateway.clients.remove')}
          </Button>
        </>
      ) : (
        <Button
          type="button"
          size="xs"
          disabled={blocked || !client.installed}
          onClick={() => void sync()}
        >
          {t('gateway.clients.connect')}
        </Button>
      )}
    </li>
  )
}

function Snippet({
  label,
  text,
  allowed
}: {
  label: string
  text: string
  allowed: () => boolean
}): React.JSX.Element {
  const { t } = useTranslation('mcp')
  const copy = async (): Promise<void> => {
    if (!isTauriContext() || !allowed()) return
    try {
      const result = await clipboardApi.writeText(text)
      if (result.success) toast.success(t('gateway.manual.copied'))
      else toast.error(t('gateway.manual.copyFailed'))
    } catch {
      toast.error(t('gateway.manual.copyFailed'))
    }
  }
  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between">
        <span className="text-2xs font-medium text-muted-foreground">{label}</span>
        <Button type="button" size="xs" variant="ghost" onClick={() => void copy()}>
          <Copy />
          {t('gateway.manual.copy')}
        </Button>
      </div>
      <pre className="max-h-40 overflow-auto rounded-md border border-border/70 bg-secondary/30 p-2 font-mono text-2xs leading-relaxed text-foreground">
        {text}
      </pre>
    </div>
  )
}

function McpGatewaySourceSection({ source }: { source: McpGatewaySource }): React.JSX.Element {
  const { t } = useTranslation('mcp')
  const sourceState = useMcpGatewayStore((state) => state.sources[source])
  const { clients, connection: candidate, error } = sourceState
  const connection =
    candidate?.source === source && candidate.mode === sourceState.mode ? candidate : null
  const refreshView = useMcpGatewayStore((state) => state.refreshView)

  useEffect(() => {
    const timer = window.setInterval(() => void refreshView(source), VIEW_POLL_MS)
    return () => window.clearInterval(timer)
  }, [refreshView, source])

  return (
    <section
      aria-label={t(`gateway.sources.${source}.title`)}
      className="space-y-4 rounded-lg border border-border bg-secondary/20 p-4"
    >
      <div>
        <p className="text-sm font-medium text-foreground">
          {t(`gateway.sources.${source}.title`)}
        </p>
        <p className="text-xs text-muted-foreground">
          {t(`gateway.sources.${source}.description`)}
        </p>
      </div>

      <GatewayStatus source={source} />

      <div className="space-y-2">
        <p className="text-xs font-medium text-foreground">{t('gateway.modeTitle')}</p>
        <ModePicker source={source} />
      </div>

      <div className="space-y-2">
        <p className="text-xs font-medium text-foreground">{t('gateway.clients.title')}</p>
        {connection && !connection.bridgeAvailable ? (
          <p className="text-2xs text-amber-600 dark:text-amber-400">
            {t('gateway.bridgeMissing', { path: connection.bridge.command })}
          </p>
        ) : null}
        <ul className="divide-y divide-border rounded-md border border-border">
          {clients.map((client) => (
            <ClientRow key={client.id} client={client} source={source} />
          ))}
        </ul>
      </div>

      {error ? <p className="text-xs text-destructive">{t('gateway.error')}</p> : null}

      {connection ? (
        <Collapsible>
          <CollapsibleTrigger className="group inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground">
            <ChevronDown className="size-3.5 transition-transform group-data-[state=open]:rotate-180" />
            {t('gateway.manual.title')}
          </CollapsibleTrigger>
          <CollapsibleContent className="mt-2 space-y-3">
            <Snippet
              label={t('gateway.manual.json')}
              text={mcpServersJson(source, connection.bridge)}
              allowed={() =>
                useMcpGatewayStore.getState().sources[source].connection === connection
              }
            />
            <Snippet
              label={t('gateway.manual.toml')}
              text={codexToml(source, connection.bridge)}
              allowed={() =>
                useMcpGatewayStore.getState().sources[source].connection === connection
              }
            />
            {canExportHttp(source, sourceState) ? (
              <Snippet
                label={t('gateway.manual.http')}
                text={httpServerJson(connection)}
                allowed={() => {
                  const current = useMcpGatewayStore.getState().sources[source]
                  return current.connection === connection && canExportHttp(source, current)
                }}
              />
            ) : (
              <p className="text-2xs text-muted-foreground">{t('gateway.httpNotReady')}</p>
            )}
            <p className="text-2xs text-muted-foreground">{t('gateway.manual.httpNote')}</p>
          </CollapsibleContent>
        </Collapsible>
      ) : null}
    </section>
  )
}

function McpGatewayConnections(): React.JSX.Element {
  const loadMode = useMcpGatewayStore((state) => state.loadMode)
  const refresh = useMcpGatewayStore((state) => state.refresh)
  useEffect(() => {
    let mounted = true
    void loadMode().then(() => {
      if (mounted) for (const source of MCP_GATEWAY_SOURCES) void refresh(source)
    })
    return () => {
      mounted = false
    }
  }, [loadMode, refresh])
  return (
    <div className="space-y-6">
      {MCP_GATEWAY_SOURCES.map((source) => (
        <McpGatewaySourceSection key={source} source={source} />
      ))}
    </div>
  )
}

export function McpGatewayPanel(): React.JSX.Element {
  const { t } = useTranslation('mcp')
  if (!isTauriContext()) {
    return <p className="text-sm text-muted-foreground">{t('export.desktopOnly')}</p>
  }
  return <McpGatewayConnections />
}
