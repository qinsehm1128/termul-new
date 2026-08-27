import { Check, RotateCcw, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { cn } from '@/lib/utils'
import { countSnapshotTerminals, useLastSessionStore } from '@/stores/last-session-store'
import { useTerminalStore } from '@/stores/terminal-store'

function formatCapturedAt(iso: string | null): string | null {
  if (!iso) return null
  const parsed = new Date(iso)
  return Number.isNaN(parsed.getTime()) ? null : parsed.toLocaleString()
}

interface LastSessionNoticeProps {
  /** Reopen a terminal for this project. */
  onRestore: (projectId: string) => void
}

/**
 * What was open before, and a way to put it back.
 *
 * Terminal tabs can disappear wholesale without the app process restarting — a
 * web-content reload resets every store while the PTYs stay alive. The layout
 * is on disk, but nothing surfaced it, so recovering meant remembering which
 * projects had been open and reopening each by hand.
 *
 * Two rules make this usable during a recovery rather than only right after a
 * restart:
 *
 * * restoring one project does *not* dismiss the panel — the remaining rows are
 *   exactly the list the user is working through, and hiding it after the first
 *   click would delete the answer at the moment it is needed;
 * * a project that is live again stays on the list with a check instead of
 *   vanishing, so rows do not shift under the cursor mid-restore.
 *
 * It hides itself once every project is back, so a clean session never carries
 * a stale panel.
 *
 * A row restores *one* terminal, not the whole remembered count: reopening a
 * terminal cannot bring back what was running in it, so the count and names are
 * shown as a reminder of what else to start rather than acted on.
 */
export function LastSessionNotice({ onRestore }: LastSessionNoticeProps): React.JSX.Element | null {
  const { t } = useTranslation('terminal')
  const snapshot = useLastSessionStore((state) => state.snapshot)
  const dismissed = useLastSessionStore((state) => state.dismissed)
  const dismiss = useLastSessionStore((state) => state.dismiss)
  const terminals = useTerminalStore((state) => state.terminals)

  const liveProjectIds = new Set(
    terminals.map((terminal) => terminal.projectId).filter((id): id is string => Boolean(id))
  )
  const projects = snapshot?.projects ?? []
  const pendingCount = projects.filter((project) => !liveProjectIds.has(project.projectId)).length

  // Nothing left to restore is the panel's own exit condition; the close button
  // is for leaving early.
  if (dismissed || countSnapshotTerminals(snapshot) === 0 || pendingCount === 0) return null

  const capturedAt = formatCapturedAt(snapshot?.capturedAt ?? null)

  return (
    <aside
      // Floating rather than in flow: `main` is a `flex-col overflow-hidden`
      // column, so an inline block gets squeezed out the moment a terminal pane
      // fills it — which is precisely when this is needed.
      className="absolute right-3 top-3 z-20 w-80 rounded-lg border border-border/70 bg-card/95 p-3 text-left shadow-lg backdrop-blur-sm"
      aria-label={t('lastSession.label')}
    >
      <div className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <p className="text-xs font-medium text-foreground">{t('lastSession.title')}</p>
          {capturedAt ? (
            <p className="mt-0.5 text-2xs text-muted-foreground">
              {t('lastSession.capturedAt', { time: capturedAt })}
            </p>
          ) : null}
        </div>
        <button
          type="button"
          onClick={dismiss}
          className="shrink-0 rounded p-1 text-muted-foreground transition-colors hover:bg-secondary/60 hover:text-foreground"
          title={t('lastSession.dismiss')}
          aria-label={t('lastSession.dismiss')}
        >
          <X className="size-3" aria-hidden="true" />
        </button>
      </div>

      <ul className="mt-2 space-y-1">
        {projects.map((project) => {
          const isLive = liveProjectIds.has(project.projectId)
          return (
            <li
              key={project.projectId}
              data-session-project={project.projectId}
              data-session-restored={isLive ? 'true' : 'false'}
              className="flex items-center gap-2 rounded px-1.5 py-1 text-2xs"
            >
              <div className="min-w-0 flex-1">
                <div className="truncate text-foreground" title={project.name}>
                  {project.name}
                </div>
                <div
                  className="truncate text-muted-foreground/80"
                  title={project.terminalNames.join(', ')}
                >
                  {t('lastSession.terminalCount', { terminals: project.terminalCount })}
                  {project.terminalNames.length > 0 ? ` · ${project.terminalNames.join(', ')}` : ''}
                </div>
              </div>
              {isLive ? (
                <span
                  className="inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-1 text-green-500"
                  title={t('lastSession.restored')}
                >
                  <Check className="size-3" aria-hidden="true" />
                  <span className="sr-only">{t('lastSession.restored')}</span>
                </span>
              ) : (
                <button
                  type="button"
                  onClick={() => onRestore(project.projectId)}
                  className={cn(
                    'inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-1',
                    'text-foreground transition-colors hover:bg-secondary/60'
                  )}
                >
                  <RotateCcw className="size-3" aria-hidden="true" />
                  {t('lastSession.restore')}
                </button>
              )}
            </li>
          )
        })}
      </ul>
    </aside>
  )
}
