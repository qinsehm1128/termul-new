import type {
  ExecuteContributionRequest,
  HostNotificationRecord,
  HostProgressRecord,
  HostUiDismissRequest
} from '@shared/types/plugin-host.types'
import { X } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { cn } from '@/lib/utils'
import { usePluginsStore } from '@/stores/plugins-store'
import { usePluginHostUiStore } from './host-ui-store'

type Feedback =
  | { kind: 'notification'; record: HostNotificationRecord }
  | { kind: 'progress'; record: HostProgressRecord }

/** The identity excludes revisions so progress/notification upserts update in place. */
export function pluginFeedbackToastId({ kind, record }: Feedback): string {
  return `plugin-host:${JSON.stringify([record.pluginId, record.generation, kind, record.id])}`
}

function isCurrent({ kind, record }: Feedback): boolean {
  const { connected, plugins, bootId } = usePluginsStore.getState()
  const { snapshot } = usePluginHostUiStore.getState()
  return Boolean(
    connected &&
      snapshot?.bootId === bootId &&
      plugins.some(
        (plugin) =>
          plugin.id === record.pluginId &&
          plugin.desiredEnabled &&
          plugin.state === 'active' &&
          plugin.generation === record.generation
      ) &&
      (kind === 'notification' ? snapshot.notifications : snapshot.progress).some(
        (current) =>
          current.pluginId === record.pluginId &&
          current.generation === record.generation &&
          current.id === record.id &&
          current.revision === record.revision &&
          (current.expiresAt === null || current.expiresAt > Date.now())
      )
  )
}

function FeedbackCard({ feedback }: { feedback: Feedback }): React.JSX.Element {
  const { t } = useTranslation('plugins')
  const [pending, setPending] = useState(false)
  const [failed, setFailed] = useState(false)
  const contextReady = usePluginHostUiStore(
    (state) =>
      state.connected &&
      !state.contextPending &&
      state.context !== null &&
      state.contextRevision > 0
  )
  const { record } = feedback
  const tone =
    feedback.kind === 'notification'
      ? feedback.record.severity
      : feedback.record.state === 'failed'
        ? 'error'
        : feedback.record.state === 'completed'
          ? 'success'
          : 'info'
  const execute = async (actionIndex?: number): Promise<void> => {
    const { execute, contextRevision, connected, contextPending, context } =
      usePluginHostUiStore.getState()
    if (
      pending ||
      !isCurrent(feedback) ||
      !connected ||
      contextPending ||
      !context ||
      contextRevision <= 0
    )
      return
    setPending(true)
    setFailed(false)
    const projectId = context.activeProject?.id
    const request: ExecuteContributionRequest = {
      pluginId: record.pluginId,
      generation: record.generation,
      kind: feedback.kind,
      contributionId: record.id,
      contextRevision,
      target: {
        ...(projectId ? { projectId } : {}),
        ...(context.activeTerminalId ? { terminalId: context.activeTerminalId } : {}),
        ...(context.activeSessionId ? { sessionId: context.activeSessionId } : {})
      },
      ...(actionIndex === undefined ? {} : { actionIndex })
    }
    try {
      const result = await execute(request)
      setFailed(!result.success)
    } catch {
      setFailed(true)
    } finally {
      setPending(false)
    }
  }
  const dismiss = (): void => {
    if (!isCurrent(feedback)) return
    const request: HostUiDismissRequest = {
      pluginId: record.pluginId,
      generation: record.generation,
      kind: feedback.kind,
      id: record.id
    }
    void usePluginHostUiStore
      .getState()
      .dismiss(request)
      .catch(() => setFailed(true))
  }
  const buttonClass =
    'rounded px-2 py-1 text-xs hover:bg-secondary focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50'

  return (
    <section
      aria-label={`${record.pluginName}: ${record.title}`}
      className="w-[356px] max-w-[calc(100vw-2rem)] rounded-md border border-border bg-popover p-3 text-popover-foreground shadow-lg"
    >
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <p className="break-words text-xs text-muted-foreground">
            {t('feedback.attribution', { name: record.pluginName })} · {record.pluginId}
          </p>
          <p
            className={cn(
              'mt-1 break-words text-sm font-medium',
              tone === 'error' && 'text-destructive',
              tone === 'warning' && 'text-warning',
              tone === 'success' && 'text-success'
            )}
          >
            {record.title}
          </p>
        </div>
        <button
          type="button"
          aria-label={t('feedback.dismiss', { title: record.title })}
          className={buttonClass}
          onClick={dismiss}
        >
          <X size={14} aria-hidden="true" />
        </button>
      </div>
      {feedback.kind === 'notification' ? (
        <>
          {feedback.record.message && (
            <p className="mt-1 whitespace-pre-wrap break-words text-xs">
              {feedback.record.message}
            </p>
          )}
          {feedback.record.actions && feedback.record.actions.length > 0 && (
            <div className="mt-2 flex flex-wrap gap-1">
              {feedback.record.actions.map((action, index) => (
                <button
                  key={`${index}:${action.commandId}`}
                  type="button"
                  disabled={pending || !contextReady}
                  className={buttonClass}
                  onClick={() => void execute(index)}
                >
                  {action.label}
                </button>
              ))}
            </div>
          )}
        </>
      ) : (
        <>
          <p className="mt-1 text-xs text-muted-foreground">
            {t(`feedback.progress.${feedback.record.state}`)}
          </p>
          <progress
            aria-label={record.title}
            className="mt-2 h-1.5 w-full accent-primary"
            max={1}
            value={feedback.record.fraction}
          />
          {feedback.record.state === 'running' && feedback.record.cancelCommandId && (
            <button
              type="button"
              disabled={pending || !contextReady}
              className={cn('mt-2', buttonClass)}
              onClick={() => void execute()}
            >
              {t('feedback.cancel')}
            </button>
          )}
        </>
      )}
      {failed && (
        <p role="alert" className="mt-1 text-xs text-destructive">
          {t('errors.generic')}
        </p>
      )}
    </section>
  )
}

/** Mounted once by the host; plugins supply data, never markup or Sonner options. */
export function PluginFeedback(): null {
  const snapshot = usePluginHostUiStore((state) => state.snapshot)
  const connected = usePluginsStore((state) => state.connected)
  const plugins = usePluginsStore((state) => state.plugins)
  const bootId = usePluginsStore((state) => state.bootId)
  const shown = useRef(new Set<string>())

  useEffect(() => {
    const next = new Set<string>()
    const records: Feedback[] =
      connected && snapshot?.bootId === bootId
        ? [
            ...snapshot.notifications.map((record): Feedback => ({ kind: 'notification', record })),
            ...snapshot.progress.map((record): Feedback => ({ kind: 'progress', record }))
          ]
        : []
    for (const feedback of records) {
      if (
        !plugins.some(
          (plugin) =>
            plugin.id === feedback.record.pluginId &&
            plugin.desiredEnabled &&
            plugin.state === 'active' &&
            plugin.generation === feedback.record.generation
        ) ||
        !isCurrent(feedback)
      )
        continue
      const id = pluginFeedbackToastId(feedback)
      next.add(id)
      toast.custom(() => <FeedbackCard key={id} feedback={feedback} />, {
        id,
        duration:
          feedback.record.expiresAt === null
            ? Infinity
            : Math.max(0, feedback.record.expiresAt - Date.now()),
        dismissible: false
      })
    }
    for (const id of shown.current) if (!next.has(id)) toast.dismiss(id)
    shown.current = next
  }, [snapshot, connected, plugins, bootId])

  useEffect(
    () => () => {
      for (const id of shown.current) toast.dismiss(id)
      shown.current.clear()
    },
    []
  )
  return null
}
