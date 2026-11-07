import type {
  McpHostAccessPolicy,
  McpHostAccessView,
  McpHostJob,
  McpSshGrant
} from '@shared/types/mcp-host.types'
import { normalizeMcpHostAccessPolicy } from '@shared/types/mcp-host.types'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { callMcpHost, getMcpHostAccess, putMcpHostAccess } from '@/lib/mcp-host-api'
import { McpCommandDenyEditor, validCommandDenyPolicy } from './McpCommandDenyEditor'
import { McpCommandDenyPreview } from './McpCommandDenyPreview'
import { HostAccessSection, hostCheckboxClass, hostInputClass } from './McpHostAccessSection'
import { McpSshAccessCard } from './McpSshAccessCard'
import { McpWorkspaceAccess } from './McpWorkspaceAccess'

const roots = (text: string): string[] => [
  ...new Set(
    text
      .split('\n')
      .map((v) => v.trim())
      .filter(Boolean)
  )
]

export function McpHostAccessPanel(): React.JSX.Element {
  const { t } = useTranslation('mcp')
  const translation = useRef(t)
  useEffect(() => {
    translation.current = t
  }, [t])
  const [view, setView] = useState<McpHostAccessView | null>(null)
  const [policy, setPolicy] = useState<McpHostAccessPolicy | null>(null)
  const [busy, setBusy] = useState(false)
  const [locked, setLocked] = useState(false)
  const [message, setMessage] = useState('')
  const [jobs, setJobs] = useState<Record<string, McpHostJob[]>>({})
  const [sshOpen, setSshOpen] = useState(true)
  const [sshQuery, setSshQuery] = useState('')
  const [expandedProfiles, setExpandedProfiles] = useState<Record<string, boolean>>({})
  const fail = useCallback((code: string) => {
    if (/FORBIDDEN|PUBLIC_TUNNEL|UNAUTHORIZED/i.test(code)) setLocked(true)
    setMessage(
      translation.current(
        /CONFLICT/i.test(code)
          ? 'hostAccess.conflict'
          : /UNAUTHORIZED/i.test(code)
            ? 'hostAccess.unauthorized'
            : /FORBIDDEN|PUBLIC_TUNNEL/i.test(code)
              ? 'hostAccess.readOnly'
              : /INVALID/i.test(code)
                ? 'commandDeny.invalidServer'
                : 'hostAccess.failed'
      )
    )
  }, [])
  const reload = useCallback(async () => {
    setBusy(true)
    try {
      const result = await getMcpHostAccess()
      if (!result.success) {
        fail(result.code)
        return
      }
      setView(result.data)
      setPolicy(normalizeMcpHostAccessPolicy(result.data.policy))
      setLocked(result.data.hostMode.toUpperCase().replaceAll('-', '_') === 'PUBLIC_TUNNEL')
      setJobs({})
    } finally {
      setBusy(false)
    }
  }, [fail])
  useEffect(() => {
    void reload()
  }, [reload])

  function grant(profileId: string, value: McpSshGrant | null): void {
    if (!policy) return
    const profiles = { ...policy.ssh.profiles }
    const fingerprints = { ...policy.profileFingerprints }
    if (value) {
      // Only NEW selection grants consent to the displayed identity. Ordinary
      // edits, bulk permissions, search and collapse never refresh existing pins.
      if (!profiles[profileId]) {
        const profile = view?.profiles.find((item) => item.profileId === profileId)
        if (profile) fingerprints[profileId] = profile.fingerprint
      }
      profiles[profileId] = value
    } else {
      delete profiles[profileId]
      delete fingerprints[profileId]
    }
    setPolicy({ ...policy, ssh: { ...policy.ssh, profiles }, profileFingerprints: fingerprints })
  }
  async function save(): Promise<void> {
    if (!policy || !view || locked) return
    if (!validCommandDenyPolicy(policy)) {
      setMessage(t('commandDeny.invalid'))
      return
    }
    setBusy(true)
    setMessage('')
    try {
      const availableProfiles = new Set(view.profiles.map((profile) => profile.profileId))
      const next = {
        ...normalizeMcpHostAccessPolicy(policy),
        ssh: {
          ...policy.ssh,
          commandDeny: policy.ssh.commandDeny ?? [],
          profiles: Object.fromEntries(
            Object.entries(policy.ssh.profiles)
              .filter(([id]) => availableProfiles.has(id))
              .map(([id, selected]) => [
                id,
                {
                  ...selected,
                  commandDeny: selected.commandDeny ?? [],
                  remoteRoots: roots(selected.remoteRoots.join('\n')),
                  localRoots: roots(selected.localRoots.join('\n'))
                }
              ])
          )
        },
        profileFingerprints: Object.fromEntries(
          Object.entries(policy.profileFingerprints).filter(
            ([id]) => availableProfiles.has(id) && !!policy.ssh.profiles[id]
          )
        )
      }
      const result = await putMcpHostAccess(view.policy.revision, next)
      if (!result.success) {
        fail(result.code)
        return
      }
      setPolicy(result.data)
      setView({ ...view, policy: result.data })
      setMessage(t('hostAccess.saved'))
    } finally {
      setBusy(false)
    }
  }
  async function run(profileId: string, tool: string, jobId?: string): Promise<void> {
    if (locked) return
    setBusy(true)
    setMessage('')
    try {
      const result = await callMcpHost({
        provider: 'ssh',
        tool,
        arguments: { profileId, ...(jobId ? { jobId } : {}) }
      })
      if (!result.success) {
        fail(result.code)
        return
      }
      if (tool === 'job_list') {
        const data = result.data as { jobs?: McpHostJob[] }
        setJobs((old) => ({
          ...old,
          [profileId]: Array.isArray(data?.jobs)
            ? data.jobs.filter((job) => job.profileId === profileId)
            : []
        }))
        setMessage(t('hostAccess.jobsRefreshed'))
      } else
        setMessage(
          t(tool === 'test_connection' ? 'hostAccess.tested' : 'hostAccess.cancelRequested')
        )
    } finally {
      setBusy(false)
    }
  }
  const disabled = busy || locked
  const canCall = (profileId: string, operation: string): boolean =>
    !!view?.policy.enabled &&
    view.policy.profileFingerprints[profileId] ===
      view.profiles.find((profile) => profile.profileId === profileId)?.fingerprint &&
    !!view.policy.ssh.profiles[profileId]?.operations.some((item) => item === operation)
  const dirty =
    !!policy &&
    !!view &&
    JSON.stringify(policy) !== JSON.stringify(normalizeMcpHostAccessPolicy(view.policy))
  const term = sshQuery.trim().toLocaleLowerCase()
  const profiles =
    view?.profiles.filter(
      (p) =>
        !term ||
        `${p.name} ${p.host} ${p.username} ${(p.tags ?? []).join(' ')}`
          .toLocaleLowerCase()
          .includes(term)
    ) ?? []
  const grantedCount = view?.profiles.filter((p) => policy?.ssh.profiles[p.profileId]).length ?? 0
  return (
    <section
      className="min-w-0 space-y-4 rounded-xl border border-border bg-secondary/10 p-3 sm:p-4"
      aria-label={t('hostAccess.title')}
    >
      <div className="flex flex-col items-start justify-between gap-3 sm:flex-row">
        <div className="min-w-0 flex-1">
          <h3 className="text-sm font-semibold">{t('hostAccess.title')}</h3>
          <p className="mt-1 max-w-2xl text-xs leading-relaxed text-muted-foreground">
            {t('hostAccess.globalWarning')}
          </p>
        </div>
        {policy && (
          <label className="flex min-h-9 shrink-0 items-center gap-2 rounded-md border border-border bg-background px-3 text-xs">
            <input
              className={hostCheckboxClass}
              type="checkbox"
              disabled={disabled}
              checked={policy.enabled}
              onChange={(e) => setPolicy({ ...policy, enabled: e.target.checked })}
            />
            {t('hostAccess.enabled')}
          </label>
        )}
      </div>
      {locked && (
        <p
          role="alert"
          className="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-xs"
        >
          {t('hostAccess.readOnly')}
        </p>
      )}
      {message && (
        <p
          role="status"
          aria-live="polite"
          className="rounded-md bg-secondary/60 px-3 py-2 text-xs"
        >
          {message}
        </p>
      )}
      {policy && view && (
        <>
          <McpWorkspaceAccess
            value={policy.workspace}
            projects={view.projects}
            disabled={disabled}
            onChange={(workspace) => setPolicy({ ...policy, workspace })}
          />
          <HostAccessSection
            title={t('hostAccess.sshProfiles')}
            open={sshOpen}
            onOpenChange={setSshOpen}
            summary={t('hostAccess.authorizedCount', {
              selected: grantedCount,
              total: view.profiles.length
            })}
          >
            <div className="space-y-3">
              <div className="flex flex-wrap gap-2">
                <input
                  type="search"
                  name="mcp-ssh-filter"
                  autoComplete="off"
                  spellCheck={false}
                  className={`${hostInputClass} flex-1 basis-48`}
                  aria-label={t('hostAccess.searchProfiles')}
                  placeholder={t('hostAccess.searchProfiles')}
                  value={sshQuery}
                  onChange={(e) => setSshQuery(e.target.value)}
                />
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={() =>
                    setExpandedProfiles(
                      Object.fromEntries(view.profiles.map((p) => [p.profileId, true]))
                    )
                  }
                >
                  {t('hostAccess.expandAll')}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  onClick={() => setExpandedProfiles({})}
                >
                  {t('hostAccess.collapseAll')}
                </Button>
              </div>
              <details className="rounded-md border border-border bg-secondary/20 px-3 py-2 text-xs">
                <summary className="cursor-pointer font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
                  {t('hostAccess.securityDetails')}
                </summary>
                <p className="pt-2 leading-relaxed text-muted-foreground">
                  {t('hostAccess.executeWarning')}
                </p>
              </details>
              <McpCommandDenyEditor
                title={t('commandDeny.global')}
                rules={policy.ssh.commandDeny ?? []}
                disabled={disabled}
                onChange={(commandDeny) =>
                  setPolicy({ ...policy, ssh: { ...policy.ssh, commandDeny } })
                }
              />
              <details className="rounded-md border p-3">
                <summary className="cursor-pointer text-xs font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
                  {t('commandDeny.preview')}
                </summary>
                <div className="mt-3">
                  <McpCommandDenyPreview
                    policy={policy}
                    profiles={view.profiles}
                    disabled={disabled}
                    dirty={dirty}
                  />
                </div>
              </details>
              {profiles.map((profile) => (
                <McpSshAccessCard
                  key={profile.profileId}
                  profile={profile}
                  selected={policy.ssh.profiles[profile.profileId]}
                  globalCommandDeny={policy.ssh.commandDeny ?? []}
                  fingerprint={policy.profileFingerprints[profile.profileId]}
                  open={expandedProfiles[profile.profileId] ?? false}
                  disabled={disabled}
                  onOpenChange={(open) =>
                    setExpandedProfiles((old) => ({ ...old, [profile.profileId]: open }))
                  }
                  onGrant={(value) => grant(profile.profileId, value)}
                  onReauthorize={() =>
                    setPolicy({
                      ...policy,
                      profileFingerprints: {
                        ...policy.profileFingerprints,
                        [profile.profileId]: profile.fingerprint
                      }
                    })
                  }
                  onRun={(operation, jobId) => void run(profile.profileId, operation, jobId)}
                  canCall={(operation) => canCall(profile.profileId, operation)}
                  jobs={jobs[profile.profileId]}
                />
              ))}
              {!profiles.length && (
                <p className="py-3 text-center text-xs text-muted-foreground">
                  {t(view.profiles.length ? 'hostAccess.noMatches' : 'hostAccess.noProfiles')}
                </p>
              )}
            </div>
          </HostAccessSection>
        </>
      )}
      <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-border bg-background p-3">
        <span
          className={`text-xs ${dirty ? 'font-medium text-primary' : 'text-muted-foreground'}`}
          role="status"
        >
          {t(dirty ? 'hostAccess.unsaved' : 'hostAccess.savedState')}
        </span>
        <div className="flex flex-wrap gap-2">
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={busy}
            onClick={() => {
              setMessage('')
              void reload()
            }}
          >
            {t('hostAccess.reload')}
          </Button>
          <Button
            type="button"
            size="sm"
            disabled={disabled || !policy}
            onClick={() => void save()}
          >
            {t(busy ? 'hostAccess.saving' : 'hostAccess.save')}
          </Button>
        </div>
      </div>
      {policy && (
        <p className="text-[11px] leading-relaxed text-muted-foreground">
          {t('hostAccess.saveHint')}
        </p>
      )}
    </section>
  )
}
