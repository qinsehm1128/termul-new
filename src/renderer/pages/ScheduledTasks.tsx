import type { ScheduledTaskRecordV1 } from '@shared/types/scheduled-task.types'
import { CalendarClock, CirclePlay, Pause, Play, RefreshCw } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
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
import { cn } from '@/lib/utils'
import { useScheduledTaskStore, useSelectedScheduledTask } from '@/stores/scheduled-task-store'

function scheduleLabel(task: ScheduledTaskRecordV1, intervalLabel: string): string {
  if (task.schedule.kind === 'cron') {
    return `${task.schedule.expression} · ${task.schedule.timezone}`
  }
  if (task.schedule.kind === 'interval') {
    return intervalLabel
  }
  return new Date(task.schedule.at).toLocaleString()
}

function statusTone(status: ScheduledTaskRecordV1['status']): string {
  if (status === 'active') return 'bg-emerald-500/10 text-emerald-500'
  if (status === 'paused') return 'bg-amber-500/10 text-amber-500'
  return 'bg-sky-500/10 text-sky-500'
}

export default function ScheduledTasks(): React.JSX.Element {
  const { t } = useTranslation('common')
  const navigate = useNavigate()
  const task = useSelectedScheduledTask()
  const {
    tasks,
    runs,
    audit,
    loading,
    mutating,
    error,
    load,
    select,
    activate,
    pause,
    resume,
    runNow
  } = useScheduledTaskStore()
  const [activationCandidate, setActivationCandidate] = useState<ScheduledTaskRecordV1 | null>(null)
  const taskScheduleLabel = (entry: ScheduledTaskRecordV1): string =>
    scheduleLabel(
      entry,
      t('scheduledTasks.everySeconds', {
        count: entry.schedule.kind === 'interval' ? entry.schedule.everySeconds : 0
      })
    )

  useEffect(() => {
    void load()
  }, [load])

  return (
    <div className="flex h-full min-h-0 bg-background">
      <aside className="flex w-80 min-w-64 flex-col border-r border-border">
        <div className="flex h-12 items-center justify-between border-b border-border px-4">
          <div className="flex items-center gap-2 text-sm font-semibold">
            <CalendarClock size={16} />
            {t('scheduledTasks.title')}
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            disabled={loading}
            onClick={() => void load()}
            aria-label={t('scheduledTasks.refresh')}
          >
            <RefreshCw size={14} className={loading ? 'animate-spin' : undefined} />
          </Button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-2">
          {tasks.length === 0 && !loading ? (
            <div className="p-6 text-center text-xs text-muted-foreground">
              {t('scheduledTasks.empty')}
            </div>
          ) : null}
          {tasks.map((entry) => (
            <button
              key={entry.taskId}
              type="button"
              onClick={() => void select(entry.taskId)}
              className={cn(
                'mb-1 w-full rounded-md px-3 py-2 text-left transition-colors hover:bg-secondary',
                task?.taskId === entry.taskId && 'bg-secondary'
              )}
            >
              <div className="flex items-center justify-between gap-2">
                <span className="truncate text-sm font-medium">{entry.name}</span>
                <span className={cn('rounded px-1.5 py-0.5 text-[10px]', statusTone(entry.status))}>
                  {t(`scheduledTasks.status.${entry.status}`)}
                </span>
              </div>
              <div className="mt-1 truncate font-mono text-[11px] text-muted-foreground">
                {taskScheduleLabel(entry)}
              </div>
            </button>
          ))}
        </div>
      </aside>

      <main className="min-w-0 flex-1 overflow-y-auto p-6">
        {error ? (
          <div role="alert" className="mb-4 rounded-md border border-destructive/40 p-3 text-sm">
            {error}
          </div>
        ) : null}
        {!task ? (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
            {t('scheduledTasks.selectTask')}
          </div>
        ) : (
          <div className="mx-auto max-w-4xl space-y-6">
            <header className="flex flex-wrap items-start justify-between gap-4">
              <div>
                <h1 className="text-xl font-semibold">{task.name}</h1>
                <p className="mt-1 font-mono text-xs text-muted-foreground">
                  {taskScheduleLabel(task)}
                </p>
              </div>
              <div className="flex gap-2">
                {task.status === 'draft' ? (
                  <Button
                    type="button"
                    disabled={mutating}
                    onClick={() => setActivationCandidate(task)}
                  >
                    <Play size={14} />
                    {t('scheduledTasks.activate')}
                  </Button>
                ) : null}
                {task.status === 'active' ? (
                  <>
                    <Button
                      type="button"
                      variant="outline"
                      disabled={mutating}
                      onClick={() => void pause(task)}
                    >
                      <Pause size={14} />
                      {t('scheduledTasks.pause')}
                    </Button>
                    <Button
                      type="button"
                      disabled={mutating}
                      onClick={() => void runNow(task.taskId)}
                    >
                      <CirclePlay size={14} />
                      {t('scheduledTasks.runNow')}
                    </Button>
                  </>
                ) : null}
                {task.status === 'paused' ? (
                  <Button type="button" disabled={mutating} onClick={() => void resume(task)}>
                    <Play size={14} />
                    {t('scheduledTasks.resume')}
                  </Button>
                ) : null}
              </div>
            </header>

            <section className="rounded-lg border border-border p-4">
              <h2 className="mb-2 text-sm font-semibold">{t('scheduledTasks.prompt')}</h2>
              <pre className="whitespace-pre-wrap font-sans text-sm text-muted-foreground">
                {task.prompt}
              </pre>
              <div className="mt-4 grid gap-2 text-xs text-muted-foreground sm:grid-cols-2">
                <div>{t('scheduledTasks.agent', { agent: task.agentConfigId })}</div>
                <div>{t('scheduledTasks.revision', { revision: task.revision })}</div>
                <div>
                  {task.projectId
                    ? t('scheduledTasks.project', { project: task.projectId })
                    : t('scheduledTasks.noProject')}
                </div>
                <div className="truncate">
                  {t('scheduledTasks.workspace', { workspace: task.workspaceCwd })}
                </div>
                <div className="truncate sm:col-span-2">
                  {t('scheduledTasks.hash', { hash: task.draftHash })}
                </div>
              </div>
            </section>

            <section>
              <h2 className="mb-2 text-sm font-semibold">{t('scheduledTasks.history')}</h2>
              <div className="space-y-2">
                {runs.length === 0 ? (
                  <p className="text-xs text-muted-foreground">{t('scheduledTasks.noRuns')}</p>
                ) : null}
                {runs.map((run) => (
                  <button
                    key={run.runId}
                    type="button"
                    disabled={!run.conversationId}
                    onClick={() => run.conversationId && navigate(`/c/${run.conversationId}`)}
                    className="flex w-full items-center justify-between rounded-md border border-border p-3 text-left hover:bg-secondary disabled:hover:bg-transparent"
                  >
                    <span className="text-sm">{t(`scheduledTasks.runStatus.${run.status}`)}</span>
                    <span className="text-xs text-muted-foreground">
                      {new Date(run.queuedAt).toLocaleString()}
                    </span>
                  </button>
                ))}
              </div>
            </section>

            <section>
              <h2 className="mb-2 text-sm font-semibold">{t('scheduledTasks.audit')}</h2>
              <div className="space-y-1 text-xs text-muted-foreground">
                {audit.map((entry) => (
                  <div key={entry.eventId} className="flex justify-between rounded px-2 py-1">
                    <span>
                      {entry.action} · {entry.actor}
                    </span>
                    <span>{new Date(entry.createdAt).toLocaleString()}</span>
                  </div>
                ))}
              </div>
            </section>
          </div>
        )}
      </main>

      <AlertDialog
        open={Boolean(activationCandidate)}
        onOpenChange={(open) => !open && setActivationCandidate(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t('scheduledTasks.confirmTitle')}</AlertDialogTitle>
            <AlertDialogDescription>
              {activationCandidate
                ? t('scheduledTasks.confirmDescription', {
                    name: activationCandidate.name,
                    schedule: taskScheduleLabel(activationCandidate),
                    revision: activationCandidate.revision
                  })
                : ''}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t('actions.cancel')}</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (activationCandidate) void activate(activationCandidate)
                setActivationCandidate(null)
              }}
            >
              {t('scheduledTasks.activate')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
