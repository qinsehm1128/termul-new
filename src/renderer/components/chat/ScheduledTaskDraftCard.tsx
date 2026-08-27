import type { ScheduledTaskRecordV1 } from '@shared/types/scheduled-task.types'
import { CalendarClock, ChevronRight } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { Button } from '@/components/ui/button'

interface ScheduledTaskDraftCardProps {
  task: ScheduledTaskRecordV1
}

function scheduleLabel(task: ScheduledTaskRecordV1, intervalLabel: string): string {
  if (task.schedule.kind === 'cron') {
    return `${task.schedule.expression} · ${task.schedule.timezone}`
  }
  if (task.schedule.kind === 'interval') return intervalLabel
  return new Date(task.schedule.at).toLocaleString()
}

export function ScheduledTaskDraftCard({ task }: ScheduledTaskDraftCardProps): React.JSX.Element {
  const { t } = useTranslation('chat')
  const navigate = useNavigate()
  const label = scheduleLabel(
    task,
    t('scheduledTaskDraft.everySeconds', {
      count: task.schedule.kind === 'interval' ? task.schedule.everySeconds : 0
    })
  )
  return (
    <section className="mx-3 mb-2 rounded-lg border border-sky-500/30 bg-sky-500/5 p-3">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2 text-sm font-semibold">
            <CalendarClock size={15} className="text-sky-500" />
            {t('scheduledTaskDraft.title')}
          </div>
          <div className="mt-1 truncate text-sm">{task.name}</div>
          <div className="mt-1 font-mono text-[11px] text-muted-foreground">{label}</div>
          <div className="mt-2 text-[11px] text-muted-foreground">
            {t('scheduledTaskDraft.revisionHash', {
              revision: task.revision,
              hash: task.draftHash.slice(0, 12)
            })}
          </div>
        </div>
        <Button type="button" size="sm" onClick={() => navigate('/scheduled-tasks')}>
          {t('scheduledTaskDraft.review')}
          <ChevronRight size={14} />
        </Button>
      </div>
      <p className="mt-2 text-xs text-muted-foreground">{t('scheduledTaskDraft.notActive')}</p>
    </section>
  )
}
