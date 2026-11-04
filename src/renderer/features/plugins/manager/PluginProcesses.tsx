import type { PluginInfo } from '@shared/types/plugin.types'
import type { HostProcessLogsReply, HostProcessStatus } from '@shared/types/plugin-host.types'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { pluginApi } from '@/features/plugins/plugin-api'

const POLL_INTERVAL = 2000
const LOG_TAIL = 200
const EMPTY_PROCESSES: NonNullable<PluginInfo['manifest']['contributes']['processes']> = []
type Action = 'start' | 'stop' | 'restart'
interface Props {
  plugin: PluginInfo
  connected: boolean
  disabled?: boolean
}

export function PluginProcesses(props: Props): React.JSX.Element {
  const { plugin, connected, disabled } = props
  // A new generation must never display a previous generation's status or logs.
  return (
    <ProcessSession
      key={`${plugin.id}:${plugin.generation}:${plugin.state}:${plugin.desiredEnabled}:${connected}:${!!disabled}`}
      {...props}
    />
  )
}

function ProcessSession({ plugin, connected, disabled = false }: Props): React.JSX.Element {
  const { t } = useTranslation('plugins')
  const processes = plugin.manifest.contributes.processes ?? EMPTY_PROCESSES
  const available =
    connected &&
    plugin.state === 'active' &&
    plugin.desiredEnabled &&
    !!plugin.generation &&
    !disabled
  const [statuses, setStatuses] = useState<Record<string, HostProcessStatus>>({})
  const [logs, setLogs] = useState<Record<string, HostProcessLogsReply>>({})
  const [pollFailed, setPollFailed] = useState(false)
  const [controlFailed, setControlFailed] = useState(false)
  const [pending, setPending] = useState<string | null>(null)
  const session = useRef({ alive: false, revision: 0, pending: false })

  useEffect(() => {
    const current = session.current
    current.alive = true
    let disposed = false
    let timer: ReturnType<typeof setTimeout> | undefined
    async function refresh(): Promise<void> {
      if (!plugin.generation) return
      const revision = current.revision
      try {
        if (!current.pending) {
          const request = { id: plugin.id, generation: plugin.generation }
          const [statusReply, ...logReplies] = await Promise.all([
            pluginApi.listProcesses(request),
            ...processes.map(({ id }) =>
              pluginApi.getProcessLogs({
                ...request,
                processId: id,
                tail: LOG_TAIL
              })
            )
          ])
          if (!current.alive || current.revision !== revision) return
          if (statusReply.success) {
            setStatuses(Object.fromEntries(statusReply.data.processes.map((p) => [p.id, p.status])))
          } else {
            setStatuses({})
          }
          const nextLogs: Record<string, HostProcessLogsReply> = {}
          for (const [index, reply] of logReplies.entries()) {
            if (reply.success) nextLogs[processes[index].id] = reply.data
          }
          // W1 replies are bounded tail snapshots, not sequenced log deltas.
          setLogs(nextLogs)
          setPollFailed(!statusReply.success || logReplies.some((reply) => !reply.success))
        }
      } catch {
        if (current.alive && current.revision === revision) {
          setStatuses({})
          setLogs({})
          setPollFailed(true)
        }
      } finally {
        if (!disposed) timer = setTimeout(() => void refresh(), POLL_INTERVAL)
      }
    }
    if (available && processes.length) void refresh()
    return () => {
      disposed = true
      current.alive = false
      current.revision++
      clearTimeout(timer)
    }
  }, [available, plugin.id, plugin.generation, processes])

  async function control(processId: string, action: Action): Promise<void> {
    const current = session.current
    if (!available || !plugin.generation || current.pending) return
    current.pending = true
    const revision = ++current.revision
    setPending(processId)
    setControlFailed(false)
    try {
      const reply = await pluginApi.controlProcess({
        id: plugin.id,
        generation: plugin.generation,
        processId,
        action
      })
      if (!current.alive || current.revision !== revision) return
      if (reply.success) setStatuses((previous) => ({ ...previous, [processId]: reply.data }))
      else setControlFailed(true)
    } catch {
      if (current.alive && current.revision === revision) setControlFailed(true)
    } finally {
      current.pending = false
      if (current.alive) setPending(null)
    }
  }

  return (
    <section aria-label={t('processes.title')} className="space-y-4">
      <p className="text-xs text-muted-foreground">{t('processes.refreshHint')}</p>
      {!processes.length && <p className="text-sm text-muted-foreground">{t('processes.empty')}</p>}
      {!!processes.length && !available && (
        <p role="status" className="text-sm text-muted-foreground">
          {t('processes.unavailable')}
        </p>
      )}
      {(pollFailed || controlFailed) && (
        <p role="alert" className="text-sm text-destructive">
          {t('processes.error')}
        </p>
      )}
      {processes.map((process) => {
        const status = statuses[process.id]
        const tail = logs[process.id]
        const running = status?.state === 'running' || status?.state === 'starting'
        const busy = !available || !!pending || !status
        return (
          <section
            key={process.id}
            aria-label={process.id}
            className="space-y-3 rounded-md border border-border p-4"
          >
            <h3 className="break-all font-mono text-sm font-medium">{process.id}</h3>
            <dl className="flex flex-wrap gap-x-6 gap-y-2 text-xs">
              <div>
                <dt className="text-muted-foreground">{t('processes.state')}</dt>
                <dd>{t(`processes.states.${status?.state ?? 'unknown'}`)}</dd>
              </div>
              <div>
                <dt className="text-muted-foreground">{t('processes.pid')}</dt>
                <dd>{status?.pid ?? '—'}</dd>
              </div>
              <div>
                <dt className="text-muted-foreground">{t('processes.restarts')}</dt>
                <dd>{status?.restarts ?? '—'}</dd>
              </div>
            </dl>
            <div className="flex flex-wrap gap-2" aria-busy={pending === process.id}>
              {(['start', 'stop', 'restart'] as const).map((action) => (
                <Button
                  key={action}
                  variant="outline"
                  size="sm"
                  disabled={
                    busy ||
                    (action === 'start' && running) ||
                    (action === 'stop' && !running && status?.state !== 'backoff')
                  }
                  onClick={() => void control(process.id, action)}
                >
                  {t(`processes.${action}`)}
                </Button>
              ))}
            </div>
            <div className="space-y-1">
              <h4 className="text-xs font-medium">{t('processes.logs')}</h4>
              {tail?.lines.length ? (
                <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-all rounded bg-muted/40 p-3 font-mono text-xs">
                  {tail.lines
                    .slice(-LOG_TAIL)
                    .map((line) => `[${line.stream}] ${line.text}`)
                    .join('\n')}
                </pre>
              ) : (
                <p className="text-xs text-muted-foreground">{t('processes.emptyLogs')}</p>
              )}
              {(tail?.truncated || (tail?.lines.length ?? 0) > LOG_TAIL) && (
                <p className="text-xs text-muted-foreground">{t('processes.truncated')}</p>
              )}
            </div>
          </section>
        )
      })}
    </section>
  )
}
