import type {
  McpHostAccessPolicy,
  McpHostAccessPreviewResult,
  McpHostProfile
} from '@shared/types/mcp-host.types'
import { useEffect, useId, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { previewMcpHostAccess } from '@/lib/mcp-host-api'
import { validCommandDenyPolicy } from './McpCommandDenyEditor'

interface Props {
  policy: McpHostAccessPolicy
  profiles: McpHostProfile[]
  disabled?: boolean
  dirty: boolean
}

/** Management-only preview. Commands live only in this component and the POST/IPC body. */
export function McpCommandDenyPreview({ policy, profiles, disabled, dirty }: Props) {
  const { t } = useTranslation('mcp')
  const id = useId()
  const [profileId, setProfileId] = useState('')
  const [operation, setOperation] = useState<'execute' | 'job_start'>('execute')
  const [command, setCommand] = useState('')
  const [useDraft, setUseDraft] = useState(true)
  const [pending, setPending] = useState(false)
  const [result, setResult] = useState<McpHostAccessPreviewResult | null>(null)
  const [error, setError] = useState<string | null>(null)
  const generation = useRef(0)
  const validPolicy = !useDraft || validCommandDenyPolicy(policy)
  const selectedProfileExists = profiles.some((profile) => profile.profileId === profileId)

  // Invalidate stale responses, without contacting the server or persisting the command.
  // biome-ignore lint/correctness/useExhaustiveDependencies: Every preview input must invalidate the pending/result snapshot, even though the effect only resets state.
  useEffect(() => {
    generation.current++
    setResult(null)
    setError(null)
    setPending(false)
    return () => {
      generation.current++
    }
  }, [policy, profileId, operation, command, useDraft, disabled])

  const preview = async () => {
    if (disabled || pending || !validPolicy || !selectedProfileExists || !command.trim()) return
    const current = ++generation.current
    setPending(true)
    setResult(null)
    setError(null)
    const response = await previewMcpHostAccess({
      profileId,
      operation,
      command,
      ...(useDraft ? { policy } : {})
    })
    if (generation.current !== current) return
    setPending(false)
    if (response.success) setResult(response.data)
    else if (response.code === 'REVISION_CONFLICT') setError(t('hostAccess.conflict'))
    else if (response.code === 'FORBIDDEN' || response.code === 'UNAUTHORIZED')
      setError(t('hostAccess.readOnly'))
    else setError(t('commandDeny.previewError'))
  }

  return (
    <section aria-labelledby={`${id}-title`} className="space-y-3 rounded-md border p-3">
      <h4 id={`${id}-title`} className="text-sm font-medium">
        {t('commandDeny.previewTitle')}
      </h4>
      <p className="text-xs text-muted-foreground">{t('commandDeny.previewHelp')}</p>
      <fieldset disabled={disabled || pending} className="min-w-0 space-y-2">
        <div className="flex flex-wrap gap-2">
          <label className="min-w-0 flex-1 basis-44 space-y-1 text-xs">
            <span>{t('commandDeny.previewProfile')}</span>
            <select
              className="h-9 w-full rounded-md border bg-background px-2"
              value={profileId}
              onChange={(event) => {
                setProfileId(event.target.value)
                setCommand('')
              }}
            >
              <option value="">{t('commandDeny.chooseProfile')}</option>
              {profiles.map((profile) => (
                <option key={profile.profileId} value={profile.profileId}>
                  {profile.name}
                </option>
              ))}
            </select>
          </label>
          <label className="min-w-0 flex-1 basis-44 space-y-1 text-xs">
            <span>{t('commandDeny.previewOperation')}</span>
            <select
              className="h-9 w-full rounded-md border bg-background px-2"
              value={operation}
              onChange={(event) => setOperation(event.target.value as 'execute' | 'job_start')}
            >
              <option value="execute">{t('hostAccess.operations.execute')}</option>
              <option value="job_start">{t('hostAccess.operations.job_start')}</option>
            </select>
          </label>
        </div>
        <label className="flex items-center gap-2 text-xs">
          <input
            type="checkbox"
            checked={useDraft}
            onChange={(event) => setUseDraft(event.target.checked)}
          />
          {t('commandDeny.useDraft')}
        </label>
        <p className="text-xs text-muted-foreground">
          {t(useDraft ? 'commandDeny.draftStatus' : 'commandDeny.activeStatus')}
          {dirty && useDraft ? ` · ${t('hostAccess.unsaved')}` : ''}
        </p>
        <label className="block space-y-1 text-xs">
          <span>{t('commandDeny.command')}</span>
          <textarea
            className="min-h-20 w-full resize-y rounded-md border bg-background p-2 font-mono text-xs"
            value={command}
            autoComplete="off"
            spellCheck={false}
            autoCapitalize="off"
            onChange={(event) => setCommand(event.target.value)}
          />
        </label>
        {!validPolicy && (
          <p role="alert" className="text-xs text-destructive">
            {t('commandDeny.invalid')}
          </p>
        )}
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => void preview()}
          disabled={!selectedProfileExists || !command.trim() || !validPolicy}
        >
          {t(pending ? 'commandDeny.previewing' : 'commandDeny.preview')}
        </Button>
      </fieldset>
      <div aria-live="polite" aria-busy={pending} className="space-y-1 text-xs">
        {error && (
          <p role="alert" className="text-destructive">
            {error}
          </p>
        )}
        {result && (
          <>
            <p className="font-medium">
              {t(result.outcome === 'matched' ? 'commandDeny.matched' : 'commandDeny.notMatched')}
            </p>
            <p>
              {t(result.draft ? 'commandDeny.draftResult' : 'commandDeny.activeResult', {
                revision: result.policyRevision
              })}
            </p>
            {result.blockedBy && <p>{t('commandDeny.blocked')}</p>}
            <ul className="space-y-1">
              {result.matches.map((match) => (
                <li key={`${match.source}:${match.id}`} className="break-all">
                  {t(match.source === 'global' ? 'commandDeny.global' : 'commandDeny.profile')}:{' '}
                  <code>{match.id}</code>
                </li>
              ))}
            </ul>
            <p className="text-muted-foreground">{t('commandDeny.notPermission')}</p>
          </>
        )}
      </div>
    </section>
  )
}
