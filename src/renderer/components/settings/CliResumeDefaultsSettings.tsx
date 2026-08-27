import {
  CLI_SESSION_AGENT_IDS,
  CLI_SESSION_AGENT_LABELS,
  type CliSessionAgentId
} from '@shared/types/cli-session.types'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { useRuntimeTranslation } from '@/i18n/use-runtime-translation'
import { loadCliResumeDefaults, saveCliResumeDefaults } from '@/lib/cli-resume-defaults'
import { logFrontendError } from '@/lib/log-api'

export function CliResumeDefaultsSettings(): React.JSX.Element {
  const t = useRuntimeTranslation('settings')
  const [extraArgsByAgentId, setExtraArgsByAgentId] = useState<
    Partial<Record<CliSessionAgentId, string>>
  >({})
  const [loaded, setLoaded] = useState(false)

  useEffect(() => {
    let cancelled = false
    void loadCliResumeDefaults()
      .then((defaults) => {
        if (!cancelled) {
          setExtraArgsByAgentId(defaults.extraArgsByAgentId)
          setLoaded(true)
        }
      })
      .catch((error) => {
        void logFrontendError({
          source: 'CliResumeDefaultsSettings',
          message: error instanceof Error ? error.message : String(error)
        })
      })
    return () => {
      cancelled = true
    }
  }, [])

  const persist = (agentId: CliSessionAgentId, value: string): void => {
    const next = { ...extraArgsByAgentId, [agentId]: value }
    setExtraArgsByAgentId(next)
    void saveCliResumeDefaults(next).catch((error) => {
      toast.error(
        error instanceof Error
          ? error.message
          : t('aiAgents.cliResumeSaveFailed', 'Failed to save resume defaults')
      )
      void logFrontendError({
        source: 'CliResumeDefaultsSettings',
        message: error instanceof Error ? error.message : String(error)
      })
    })
  }

  return (
    <div className="space-y-3">
      <div>
        <h3 className="text-sm font-medium text-secondary-foreground">
          {t('aiAgents.cliResumeTitle', 'CLI resume extra args')}
        </h3>
        <p className="text-xs text-muted-foreground mt-1">
          {t(
            'aiAgents.cliResumeDescription',
            'Prefixed before the resume flag when launching a scanned session. Edit here or override once in the resume dialog.'
          )}
        </p>
      </div>
      <div className="space-y-2">
        {CLI_SESSION_AGENT_IDS.map((agentId) => (
          <div key={agentId} className="space-y-1">
            <Label htmlFor={`cli-resume-${agentId}`}>{CLI_SESSION_AGENT_LABELS[agentId]}</Label>
            <Input
              id={`cli-resume-${agentId}`}
              value={extraArgsByAgentId[agentId] ?? ''}
              disabled={!loaded}
              onChange={(event) => persist(agentId, event.target.value)}
              placeholder={t('aiAgents.cliResumePlaceholder', 'none')}
            />
          </div>
        ))}
      </div>
    </div>
  )
}
