import type { DiscoveredCliSession } from '@shared/types/cli-session.types'
import { CLI_SESSION_AGENT_LABELS } from '@shared/types/cli-session.types'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { useRuntimeTranslation } from '@/i18n/use-runtime-translation'
import { cliSessionListTitle } from '@/lib/cli-session-title'

interface CliSessionResumeDialogProps {
  session: DiscoveredCliSession | null
  defaultExtraArgs: string
  busy?: boolean
  onOpenChange: (open: boolean) => void
  onConfirm: (onceExtraArgs: string) => void
}

export function CliSessionResumeDialog({
  session,
  defaultExtraArgs,
  busy = false,
  onOpenChange,
  onConfirm
}: CliSessionResumeDialogProps): React.JSX.Element {
  const t = useRuntimeTranslation('shell')
  const [onceExtraArgs, setOnceExtraArgs] = useState('')

  return (
    <Dialog
      open={session !== null}
      onOpenChange={(open) => {
        if (!open) setOnceExtraArgs('')
        onOpenChange(open)
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t('cliSessions.resumeTitle', 'Resume CLI session')}</DialogTitle>
          <DialogDescription>
            {session
              ? `${CLI_SESSION_AGENT_LABELS[session.agentId]} · ${cliSessionListTitle(session, session.sessionId, t('cliSessions.untitled', 'Untitled session'))}`
              : t(
                  'cliSessions.resumeDescription',
                  'Resume a scanned CLI agent session in a new terminal.'
                )}
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-3">
          <p className="text-xs text-muted-foreground">
            {t(
              'cliSessions.resumeHint',
              'Default extra args are inserted before the resume flag. One-time args apply only to this launch.'
            )}
          </p>
          <div className="space-y-1.5">
            <Label htmlFor="cli-resume-default-args">
              {t('cliSessions.defaultExtraArgs', 'Default extra args')}
            </Label>
            <Input id="cli-resume-default-args" value={defaultExtraArgs} readOnly />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="cli-resume-once-args">
              {t('cliSessions.onceExtraArgs', 'One-time extra args')}
            </Label>
            <Input
              id="cli-resume-once-args"
              value={onceExtraArgs}
              onChange={(event) => setOnceExtraArgs(event.target.value)}
              placeholder={t('cliSessions.onceExtraArgsPlaceholder', '--yolo')}
            />
          </div>
        </div>
        <DialogFooter>
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)} disabled={busy}>
            {t('cliSessions.cancel', 'Cancel')}
          </Button>
          <Button
            type="button"
            disabled={!session || busy || !session.resumable}
            onClick={() => onConfirm(onceExtraArgs)}
          >
            {t('cliSessions.resume', 'Resume')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
