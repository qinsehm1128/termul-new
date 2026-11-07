import {
  emptyMcpSshGrant,
  MCP_SSH_OPERATIONS,
  type McpHostJob,
  type McpHostProfile,
  type McpSshGrant,
  type McpSshOperation
} from '@shared/types/mcp-host.types'
import { ChevronDown } from 'lucide-react'
import { useId, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { hostCheckboxClass } from './McpHostAccessSection'

const READ_OPERATIONS: McpSshOperation[] = [
  'profile_list',
  'profile_get',
  'list_files',
  'read_file',
  'job_status',
  'job_list'
]
const RISK_OPERATIONS: McpSshOperation[] = [
  'execute',
  'job_start',
  'write_file',
  'upload',
  'download',
  'job_cancel'
]
const GROUPS = [
  { title: 'readPermissions' as const, operations: READ_OPERATIONS, risk: false },
  {
    title: 'connectionPermissions' as const,
    operations: ['test_connection'] as McpSshOperation[],
    risk: false
  },
  { title: 'riskPermissions' as const, operations: RISK_OPERATIONS, risk: true }
]

export function McpSshAccessCard({
  profile,
  selected,
  fingerprint,
  disabled,
  open,
  onOpenChange,
  onGrant,
  onReauthorize,
  onRun,
  canCall,
  jobs
}: {
  profile: McpHostProfile
  selected?: McpSshGrant
  fingerprint?: string
  disabled: boolean
  open: boolean
  onOpenChange: (open: boolean) => void
  onGrant: (grant: McpSshGrant | null) => void
  onReauthorize: () => void
  onRun: (operation: McpSshOperation, jobId?: string) => void
  canCall: (operation: McpSshOperation) => boolean
  jobs?: McpHostJob[]
}): React.JSX.Element {
  const { t } = useTranslation('mcp')
  const id = useId()
  const [confirmAll, setConfirmAll] = useState(false)
  const stale = !!selected && fingerprint !== profile.fingerprint
  return (
    <article
      className="min-w-0 overflow-hidden rounded-lg border border-border bg-background"
      aria-label={profile.name}
    >
      <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-2 px-3 py-2.5">
        <button
          type="button"
          className="flex min-w-0 flex-1 basis-48 items-center gap-2 rounded py-1 text-left hover:text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          aria-label={t('hostAccess.configureProfile', { name: profile.name })}
          aria-expanded={open}
          aria-controls={id}
          onClick={() => {
            setConfirmAll(false)
            onOpenChange(!open)
          }}
        >
          <ChevronDown
            aria-hidden="true"
            className={`size-4 shrink-0 text-muted-foreground ${open ? '' : '-rotate-90'}`}
          />
          <span className="min-w-0">
            <span className="block break-words text-sm font-medium">{profile.name}</span>
            <span className="block break-all text-[11px] text-muted-foreground">
              {profile.username}@{profile.host}:{profile.port} · {profile.authMethod}
            </span>
          </span>
        </button>
        <span
          className={`rounded px-2 py-1 text-[11px] tabular-nums ${stale ? 'bg-destructive/10 text-destructive' : 'bg-secondary text-muted-foreground'}`}
        >
          {stale
            ? t('hostAccess.identityChangedShort')
            : selected
              ? t('hostAccess.permissionCount', {
                  count: selected.operations.length,
                  total: MCP_SSH_OPERATIONS.length
                })
              : t('hostAccess.notGranted')}
        </span>
        <label className="flex min-h-8 shrink-0 items-center gap-2 text-xs">
          <input
            type="checkbox"
            className={hostCheckboxClass}
            disabled={disabled}
            checked={!!selected}
            onChange={(e) => {
              setConfirmAll(false)
              onGrant(
                e.target.checked
                  ? { ...emptyMcpSshGrant(), operations: [...MCP_SSH_OPERATIONS] }
                  : null
              )
              if (e.target.checked) onOpenChange(true)
            }}
          />
          {t('hostAccess.grantProfile')}
        </label>
      </div>
      <div id={id} hidden={!open} className="space-y-3 border-t border-border p-3">
        {!selected && (
          <p className="text-xs text-muted-foreground">{t('hostAccess.grantToConfigure')}</p>
        )}
        {selected && (
          <fieldset disabled={disabled} className="min-w-0 space-y-3">
            {stale && (
              <div className="space-y-2 rounded-md border border-destructive/30 bg-destructive/5 p-3">
                <p className="text-xs leading-relaxed text-destructive">
                  {t('hostAccess.fingerprintChanged')}
                </p>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={disabled}
                  onClick={onReauthorize}
                >
                  {t('hostAccess.reauthorizeIdentity')}
                </Button>
              </div>
            )}
            <div className="flex flex-wrap gap-2">
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={disabled}
                onClick={() => {
                  setConfirmAll(false)
                  onGrant({
                    ...selected,
                    operations: [...READ_OPERATIONS]
                  })
                }}
              >
                {t('hostAccess.selectReadOnly')}
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={disabled}
                onClick={() => setConfirmAll(true)}
              >
                {t('hostAccess.selectAllPermissions')}
              </Button>
              <Button
                type="button"
                size="sm"
                variant="ghost"
                disabled={disabled || !selected.operations.length}
                onClick={() => {
                  setConfirmAll(false)
                  onGrant({ ...selected, operations: [] })
                }}
              >
                {t('hostAccess.clearPermissions')}
              </Button>
            </div>
            {confirmAll && (
              <div
                role="alert"
                className="space-y-2 rounded-md border border-destructive/30 bg-destructive/5 p-3"
              >
                <p className="text-xs leading-relaxed">{t('hostAccess.confirmAllWarning')}</p>
                <div className="flex flex-wrap gap-2">
                  <Button
                    type="button"
                    size="sm"
                    variant="destructive"
                    disabled={disabled}
                    onClick={() => {
                      onGrant({ ...selected, operations: [...MCP_SSH_OPERATIONS] })
                      setConfirmAll(false)
                    }}
                  >
                    {t('hostAccess.confirmAllPermissions')}
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    variant="ghost"
                    onClick={() => setConfirmAll(false)}
                  >
                    {t('hostAccess.cancelSelection')}
                  </Button>
                </div>
              </div>
            )}
            {GROUPS.map((group) => (
              <div
                key={group.title}
                className={
                  group.risk
                    ? 'rounded-md border border-destructive/20 bg-destructive/5 p-3'
                    : 'space-y-2'
                }
              >
                <h5
                  className={`mb-2 text-xs font-medium ${group.risk ? 'text-destructive' : 'text-muted-foreground'}`}
                >
                  {t(`hostAccess.${group.title}`)}
                </h5>
                <div className="grid grid-cols-1 gap-x-4 gap-y-1 sm:grid-cols-2 lg:grid-cols-3">
                  {group.operations.map((operation) => (
                    <label key={operation} className="flex min-h-8 items-center gap-2 text-xs">
                      <input
                        type="checkbox"
                        className={hostCheckboxClass}
                        checked={selected.operations.includes(operation)}
                        onChange={(e) =>
                          onGrant({
                            ...selected,
                            operations: e.target.checked
                              ? [...selected.operations, operation]
                              : selected.operations.filter((op) => op !== operation)
                          })
                        }
                      />
                      <span>
                        {t(`hostAccess.operations.${operation}`)}
                        {group.risk ? ` — ${t('hostAccess.highRisk')}` : ''}
                      </span>
                    </label>
                  ))}
                </div>
              </div>
            ))}
            <div className="grid min-w-0 gap-3 md:grid-cols-2">
              <label className="min-w-0 text-xs text-muted-foreground">
                {t('hostAccess.remoteRoots')}
                <textarea
                  className="mt-1.5 block w-full rounded-md border border-input bg-background p-2 font-mono text-xs text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  name={`remote-roots-${profile.profileId}`}
                  autoComplete="off"
                  spellCheck={false}
                  rows={2}
                  value={selected.remoteRoots.join('\n')}
                  onChange={(e) =>
                    onGrant({ ...selected, remoteRoots: e.target.value.split('\n') })
                  }
                />
              </label>
              <label className="min-w-0 text-xs text-muted-foreground">
                {t('hostAccess.localRoots')}
                <textarea
                  className="mt-1.5 block w-full rounded-md border border-input bg-background p-2 font-mono text-xs text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  name={`local-roots-${profile.profileId}`}
                  autoComplete="off"
                  spellCheck={false}
                  rows={2}
                  value={selected.localRoots.join('\n')}
                  onChange={(e) => onGrant({ ...selected, localRoots: e.target.value.split('\n') })}
                />
              </label>
            </div>
            <p className="text-xs leading-relaxed text-muted-foreground">
              {t('hostAccess.localWarning')}
            </p>
            <label className="flex items-start gap-2 text-xs leading-relaxed text-muted-foreground">
              <input
                type="checkbox"
                className={`${hostCheckboxClass} mt-0.5`}
                checked={selected.posixJobs}
                onChange={(e) => onGrant({ ...selected, posixJobs: e.target.checked })}
              />
              {t('hostAccess.posixJobs')}
            </label>
          </fieldset>
        )}
        <div className="flex flex-wrap gap-2 border-t border-border pt-3">
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={disabled || !canCall('test_connection')}
            onClick={() => onRun('test_connection')}
          >
            {t('hostAccess.test')}
          </Button>
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={disabled || !canCall('job_list')}
            onClick={() => onRun('job_list')}
          >
            {t('hostAccess.refreshJobs')}
          </Button>
        </div>
        {jobs && (
          <ul className="space-y-1 text-xs" aria-label={t('hostAccess.jobs')}>
            {jobs.length === 0 && <li>{t('hostAccess.noJobs')}</li>}
            {jobs.map((job) => (
              <li key={job.jobId} className="flex min-w-0 items-center justify-between gap-2">
                <span className="min-w-0 break-all">
                  {job.jobId} · {job.state}
                </span>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={disabled || job.finished || !canCall('job_cancel')}
                  onClick={() => onRun('job_cancel', job.jobId)}
                >
                  {t('hostAccess.cancelJob')}
                </Button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </article>
  )
}
