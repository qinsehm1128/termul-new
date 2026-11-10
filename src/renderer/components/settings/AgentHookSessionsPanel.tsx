import type { DerivedState } from '@shared/types/agent-hooks.types'
import { Info } from 'lucide-react'
import { useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { agentHooksApi } from '@/lib/agent-hooks-api'
import { logFrontendError } from '@/lib/log-api'
import { cn } from '@/lib/utils'
import { type AgentHookRuntimeState, useAgentHookStateStore } from '@/stores/agent-hook-state-store'

const STATE_DOTS: Record<DerivedState, string> = {
  busy: 'bg-amber-500',
  idle: 'bg-emerald-500',
  needs_approval: 'bg-amber-500',
  blocked: 'bg-destructive',
  error: 'bg-destructive',
  ended: 'bg-muted-foreground/50'
}

/** Opaque ids can be long; the panel only needs enough to tell sessions apart. */
function shortId(value: string): string {
  return value.length > 10 ? `${value.slice(0, 8)}…` : value
}

function sortHookSessions(rows: AgentHookRuntimeState[]): AgentHookRuntimeState[] {
  return [...rows].sort((a, b) =>
    a.project_id === b.project_id
      ? a.terminal_id.localeCompare(b.terminal_id)
      : a.project_id.localeCompare(b.project_id)
  )
}

/**
 * Read-only view of hook runtime state. The app-level subscription feeds
 * `useAgentHookStateStore`; one `state()` call here covers a cold settings mount.
 * Never renders tokens, payloads or transcript paths.
 */
export function AgentHookSessionsPanel(): React.JSX.Element {
  const { t } = useTranslation('settings')
  const text = (key: string, fallback: string) =>
    t(`agentHooks.sessions.${key}`, { defaultValue: fallback })
  const states = useAgentHookStateStore((state) => state.states)
  const rows = sortHookSessions(Object.values(states))

  useEffect(() => {
    let disposed = false
    void agentHooksApi
      .state()
      .then((response) => {
        if (!disposed) useAgentHookStateStore.getState().applySync(response)
      })
      .catch(() => {
        // Boundary only: never forward ids, payloads or transcript paths.
        if (!disposed)
          void logFrontendError({
            source: 'AgentHookSessionsPanel',
            message: 'Agent hook state load failed'
          })
      })
    return () => {
      disposed = true
    }
  }, [])

  return (
    <section className="space-y-3" aria-label={text('title', 'Live sessions')}>
      <div className="flex flex-wrap items-center gap-3">
        <h3 className="text-sm font-medium">{text('title', 'Live sessions')}</h3>
        <TooltipProvider>
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                type="button"
                aria-label={text('helpLabel', 'About live hook sessions')}
                className="text-muted-foreground"
              >
                <Info size={14} aria-hidden="true" />
              </button>
            </TooltipTrigger>
            <TooltipContent className="max-w-xs">
              {text('help', 'Live state reported by installed hooks. Read-only.')}
            </TooltipContent>
          </Tooltip>
        </TooltipProvider>
      </div>
      {rows.length === 0 ? (
        <p className="text-xs text-muted-foreground">{text('empty', 'No hook sessions')}</p>
      ) : (
        <ul className="space-y-2">
          {rows.map((row) => (
            <li
              key={`${row.instance_id}\0${row.terminal_id}`}
              className="space-y-1 rounded-md border border-border/70 px-3 py-2 text-xs"
            >
              <div className="flex flex-wrap items-center gap-2">
                <span
                  className={cn('size-2 shrink-0 rounded-full', STATE_DOTS[row.state])}
                  aria-hidden="true"
                />
                <code className="truncate">{row.identity.adapter_id}</code>
                <span className="text-muted-foreground" aria-hidden="true">
                  ·
                </span>
                <code className="truncate text-muted-foreground">
                  {shortId(row.identity.agent_session_id)}
                </code>
                <span className="ml-auto font-medium">
                  {text(`states.${row.state}`, row.state)}
                </span>
              </div>
              <dl className="grid grid-cols-2 gap-x-3 gap-y-0.5 text-muted-foreground sm:grid-cols-4">
                <div className="flex gap-1">
                  <dt>{text('terminal', 'Terminal')}</dt>
                  <dd>
                    <code>{shortId(row.terminal_id)}</code>
                  </dd>
                </div>
                <div className="flex gap-1">
                  <dt>{text('project', 'Project')}</dt>
                  <dd>
                    <code>{shortId(row.project_id)}</code>
                  </dd>
                </div>
                <div className="flex gap-1">
                  <dt>{text('ready', 'Ready')}</dt>
                  <dd>{row.ready ? text('yes', 'Yes') : text('no', 'No')}</dd>
                </div>
                <div className="flex gap-1">
                  <dt>{text('restore', 'Restore')}</dt>
                  <dd>{text(`restoreStates.${row.restore_state}`, row.restore_state)}</dd>
                </div>
              </dl>
            </li>
          ))}
        </ul>
      )}
    </section>
  )
}
