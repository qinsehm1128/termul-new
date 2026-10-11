import { AlertTriangle, Loader2, RefreshCw, RotateCw } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { ConfirmDialog } from '@/components/ConfirmDialog'
import { useAcpStore } from '@/features/agent-session/stores/acp-store'
import {
  type CoreProcessStatus,
  type CoreRuntimeStatus,
  type CoreTerminalStatus,
  coreRuntimeApi
} from '@/lib/core-runtime-api'
import { logFrontendError } from '@/lib/log-api'
import { cn } from '@/lib/utils'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'

/**
 * What runs in the background and what each process holds.
 *
 * Terminals and agents live in their own Core processes so they survive a GUI
 * restart or update. Without this panel that is invisible: after a restart the
 * user cannot tell whether a terminal is still running somewhere or is gone.
 * Every PTY the Terminal Core holds is listed, including the ones no tab shows.
 */

const REFRESH_INTERVAL_MS = 3000

type RestartTarget = 'gui' | 'acpCore' | 'terminalCore'

function formatTime(ms: number | null): string | null {
  return ms === null ? null : new Date(ms).toLocaleString()
}

function shortBuild(buildId: string | null): string | null {
  return buildId === null ? null : buildId.slice(0, 12)
}

export function CoreRuntimeSettings(): React.JSX.Element {
  const { t } = useTranslation('settings')
  const [status, setStatus] = useState<CoreRuntimeStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [confirming, setConfirming] = useState<RestartTarget | null>(null)
  const [restarting, setRestarting] = useState<RestartTarget | null>(null)
  const mounted = useRef(true)

  const terminals = useTerminalStore((state) => state.terminals)
  const projects = useProjectStore((state) => state.projects)
  const acpSessions = useAcpStore((state) => state.sessions)
  const configToLiveAgent = useAcpStore((state) => state.configToLiveAgent)

  const refresh = useCallback(async (): Promise<void> => {
    try {
      const next = await coreRuntimeApi.status()
      if (mounted.current) setStatus(next)
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      if (mounted.current) setError(message)
    }
  }, [])

  useEffect(() => {
    mounted.current = true
    void refresh()
    const timer = window.setInterval(() => void refresh(), REFRESH_INTERVAL_MS)
    return () => {
      mounted.current = false
      window.clearInterval(timer)
    }
  }, [refresh])

  const restart = useCallback(
    async (target: RestartTarget): Promise<void> => {
      setConfirming(null)
      setRestarting(target)
      setError(null)
      try {
        if (target === 'gui') {
          await coreRuntimeApi.restartGui()
          return
        }
        await coreRuntimeApi.restartCore(target)
        await refresh()
      } catch (caught) {
        const message = caught instanceof Error ? caught.message : String(caught)
        if (mounted.current) setError(message)
        void logFrontendError({ source: 'CoreRuntimeSettings', message })
      } finally {
        if (mounted.current) setRestarting(null)
      }
    },
    [refresh]
  )

  const terminalLabel = (
    pty: CoreTerminalStatus
  ): { name: string | null; agent: string | null } => {
    const record = terminals.find((terminal) => terminal.ptyId === pty.id)
    return {
      name: record?.name ?? null,
      agent: record?.agentSession?.agentId ?? record?.agentName ?? null
    }
  }

  const projectName = (projectId: string | undefined): string | null =>
    projects.find((project) => project.id === projectId)?.name ?? null

  const agentLabel = (agentId: string): string => {
    const configId = Object.keys(configToLiveAgent).find(
      (key) => configToLiveAgent[key] === agentId
    )
    const titles = Object.values(acpSessions)
      .filter((session) => session.agentId === agentId)
      .map((session) => session.title)
      .filter((title): title is string => Boolean(title))
    return [configId ?? agentId, ...titles].join(' · ')
  }

  const terminalCount = status?.terminals?.length ?? status?.terminalCore.activeResources ?? 0
  const agentCount = status?.agents?.length ?? status?.acpCore.activeResources ?? 0

  const confirmCopy: Record<RestartTarget, { title: string; message: string }> = {
    gui: { title: t('runtime.restartGuiTitle'), message: t('runtime.restartGuiMessage') },
    terminalCore: {
      title: t('runtime.restartTerminalCoreTitle'),
      message: t('runtime.restartTerminalCoreMessage', { count: terminalCount })
    },
    acpCore: {
      title: t('runtime.restartAcpCoreTitle'),
      message: t('runtime.restartAcpCoreMessage', { count: agentCount })
    }
  }

  const restartButton = (target: RestartTarget, disabled: boolean): React.JSX.Element => (
    <button
      type="button"
      onClick={() => setConfirming(target)}
      disabled={disabled || restarting !== null}
      data-testid={`runtime-restart-${target}`}
      className="inline-flex h-7 flex-shrink-0 items-center gap-1.5 rounded-md bg-secondary/50 px-2.5 text-xs text-foreground transition-colors duration-150 hover:bg-secondary disabled:cursor-not-allowed disabled:opacity-50"
    >
      {restarting === target ? (
        <Loader2 size={12} className="animate-spin" />
      ) : (
        <RotateCw size={12} />
      )}
      {restarting === target ? t('runtime.restarting') : t('runtime.restart')}
    </button>
  )

  const processFacts = (pid: number | null, startedAtMs: number | null, buildId: string | null) => (
    <p className="mt-0.5 font-mono text-3xs text-muted-foreground/80">
      {[
        pid === null ? null : `pid ${pid}`,
        formatTime(startedAtMs) === null
          ? null
          : t('runtime.startedAt', { when: formatTime(startedAtMs) }),
        shortBuild(buildId) === null ? null : `build ${shortBuild(buildId)}`
      ]
        .filter(Boolean)
        .join(' · ')}
    </p>
  )

  const coreCard = (
    target: 'acpCore' | 'terminalCore',
    core: CoreProcessStatus,
    children: React.ReactNode
  ): React.JSX.Element => {
    const state = core.mode === 'inProcess' ? 'inProcess' : core.running ? 'running' : 'stopped'
    return (
      <div
        data-runtime-card={target}
        data-runtime-state={state}
        className="rounded-md bg-secondary/25 px-3 py-2.5 shadow-[inset_0_1px_0_0_hsl(var(--foreground)/0.035)]"
      >
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="text-sm text-foreground">{t(`runtime.${target}`)}</span>
              <span
                className={cn(
                  'rounded-full px-1.5 py-0.5 text-3xs ring-1 ring-inset',
                  state === 'stopped'
                    ? 'bg-red-500/10 text-red-500 ring-red-500/20'
                    : 'bg-green-500/10 text-green-500 ring-green-500/20'
                )}
              >
                {t(`runtime.state.${state}`)}
              </span>
            </div>
            {processFacts(core.pid, core.startedAtMs, core.buildId)}
          </div>
          {restartButton(target, core.mode !== 'separate')}
        </div>
        {children}
      </div>
    )
  }

  return (
    <div className="w-2/3 space-y-3" data-testid="core-runtime-settings">
      {error && (
        <div
          role="alert"
          data-testid="runtime-error"
          className="flex items-start gap-2 rounded-md border border-red-500/20 bg-red-500/10 px-3 py-2.5"
        >
          <AlertTriangle size={14} className="mt-0.5 flex-shrink-0 text-red-500" />
          <p className="text-xs text-foreground">{error}</p>
        </div>
      )}

      {status === null ? (
        <p className="text-xs text-muted-foreground">{t('runtime.loading')}</p>
      ) : (
        <>
          <div
            data-runtime-card="gui"
            className="rounded-md bg-secondary/25 px-3 py-2.5 shadow-[inset_0_1px_0_0_hsl(var(--foreground)/0.035)]"
          >
            <div className="flex items-start justify-between gap-3">
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="text-sm text-foreground">{t('runtime.gui')}</span>
                  <span className="text-3xs text-muted-foreground">v{status.gui.version}</span>
                </div>
                {processFacts(status.gui.pid, status.gui.startedAtMs, status.gui.buildId)}
              </div>
              {restartButton('gui', false)}
            </div>
          </div>

          {coreCard(
            'terminalCore',
            status.terminalCore,
            status.terminals !== null && (
              <div className="mt-2 space-y-1.5" data-testid="runtime-terminals">
                {status.terminals.length === 0 ? (
                  <p className="text-xs text-muted-foreground">{t('runtime.noTerminals')}</p>
                ) : (
                  status.terminals.map((pty) => {
                    const { name, agent } = terminalLabel(pty)
                    return (
                      <div
                        key={pty.id}
                        data-runtime-terminal={pty.id}
                        className="rounded bg-background/40 px-2 py-1.5"
                      >
                        <div className="flex flex-wrap items-center gap-1.5 text-xs text-foreground">
                          <span>{name ?? pty.id}</span>
                          {projectName(pty.projectId) && (
                            <span className="text-muted-foreground">
                              {projectName(pty.projectId)}
                            </span>
                          )}
                          {agent && (
                            <span className="rounded-full bg-primary/10 px-1.5 text-3xs text-primary">
                              {agent}
                            </span>
                          )}
                          {name === null && (
                            <span className="rounded-full bg-amber-500/10 px-1.5 text-3xs text-amber-500">
                              {t('runtime.noTab')}
                            </span>
                          )}
                          {!pty.active && (
                            <span className="rounded-full bg-secondary/60 px-1.5 text-3xs text-muted-foreground">
                              {pty.lifecycle}
                            </span>
                          )}
                        </div>
                        <p className="mt-0.5 break-all font-mono text-3xs text-muted-foreground/70">
                          pid {pty.pid} · {pty.cwd}
                        </p>
                      </div>
                    )
                  })
                )}
              </div>
            )
          )}

          {coreCard(
            'acpCore',
            status.acpCore,
            status.agents !== null && (
              <div className="mt-2 space-y-1.5" data-testid="runtime-agents">
                {status.agents.length === 0 ? (
                  <p className="text-xs text-muted-foreground">{t('runtime.noAgents')}</p>
                ) : (
                  status.agents.map((agentId) => (
                    <p
                      key={agentId}
                      data-runtime-agent={agentId}
                      className="rounded bg-background/40 px-2 py-1.5 text-xs text-foreground"
                    >
                      {agentLabel(agentId)}
                    </p>
                  ))
                )}
              </div>
            )
          )}
        </>
      )}

      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={() => void refresh()}
          data-testid="runtime-refresh"
          className="inline-flex h-8 items-center gap-2 rounded-md bg-secondary/50 px-3 text-sm text-foreground transition-colors duration-150 hover:bg-secondary"
        >
          <RefreshCw size={14} />
          {t('runtime.refresh')}
        </button>
        <p className="text-xs text-muted-foreground">{t('runtime.autoRefresh')}</p>
      </div>

      <ConfirmDialog
        isOpen={confirming !== null}
        title={confirming ? confirmCopy[confirming].title : ''}
        message={confirming ? confirmCopy[confirming].message : ''}
        confirmLabel={t('runtime.restart')}
        variant={confirming === 'gui' ? 'default' : 'danger'}
        onConfirm={() => {
          if (confirming) void restart(confirming)
        }}
        onCancel={() => setConfirming(null)}
      />
    </div>
  )
}
