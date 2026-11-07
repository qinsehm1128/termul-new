import {
  emptyMcpSshGrant,
  MCP_SSH_OPERATIONS,
  type McpHostAccessPolicy,
  type McpHostAccessView,
  type McpHostJob,
  type McpSshGrant
} from '@shared/types/mcp-host.types'
import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { callMcpHost, getMcpHostAccess, putMcpHostAccess } from '@/lib/mcp-host-api'

const HIGH_RISK = new Set(['write_file', 'upload', 'download', 'job_cancel'])
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
  const [view, setView] = useState<McpHostAccessView | null>(null)
  const [policy, setPolicy] = useState<McpHostAccessPolicy | null>(null)
  const [busy, setBusy] = useState(false)
  const [locked, setLocked] = useState(false)
  const [message, setMessage] = useState('')
  const [jobs, setJobs] = useState<Record<string, McpHostJob[]>>({})
  const fail = useCallback(
    (code: string) => {
      if (/FORBIDDEN|PUBLIC_TUNNEL|UNAUTHORIZED/i.test(code)) setLocked(true)
      setMessage(
        t(
          /CONFLICT/i.test(code)
            ? 'hostAccess.conflict'
            : /UNAUTHORIZED/i.test(code)
              ? 'hostAccess.unauthorized'
              : /FORBIDDEN|PUBLIC_TUNNEL/i.test(code)
                ? 'hostAccess.readOnly'
                : 'hostAccess.failed'
        )
      )
    },
    [t]
  )
  const reload = useCallback(async () => {
    setBusy(true)
    try {
      const result = await getMcpHostAccess()
      if (!result.success) {
        fail(result.code)
        return
      }
      setView(result.data)
      setPolicy(result.data.policy)
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
      // Only a newly selected profile grants consent to its displayed identity.
      // Editing roots/operations or unrelated settings must preserve old pins.
      if (!profiles[profileId]) {
        const profile = view?.profiles.find((item) => item.profileId === profileId)
        if (profile) fingerprints[profileId] = profile.fingerprint
      }
      profiles[profileId] = value
    } else {
      delete profiles[profileId]
      delete fingerprints[profileId]
    }
    setPolicy({ ...policy, ssh: { profiles }, profileFingerprints: fingerprints })
  }
  async function save(): Promise<void> {
    if (!policy || !view || locked) return
    setBusy(true)
    setMessage('')
    try {
      const availableProfiles = new Set(view.profiles.map((profile) => profile.profileId))
      const next = {
        ...policy,
        ssh: {
          profiles: Object.fromEntries(
            Object.entries(policy.ssh.profiles)
              .filter(([id]) => availableProfiles.has(id))
              .map(([id, selected]) => [
                id,
                {
                  ...selected,
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
  return (
    <section
      className="space-y-3 rounded-lg border border-border bg-secondary/20 p-4"
      aria-label={t('hostAccess.title')}
    >
      <h3 className="text-sm font-medium">{t('hostAccess.title')}</h3>
      <p className="text-xs text-muted-foreground">{t('hostAccess.globalWarning')}</p>
      {locked && (
        <p role="alert" className="text-xs text-destructive">
          {t('hostAccess.readOnly')}
        </p>
      )}
      {message && (
        <p role="status" className="text-xs">
          {message}
        </p>
      )}
      <Button
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
      {policy && view && (
        <>
          <fieldset disabled={disabled} className="space-y-3">
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={policy.enabled}
                onChange={(e) => setPolicy({ ...policy, enabled: e.target.checked })}
              />
              {t('hostAccess.enabled')}
            </label>
            <fieldset className="space-y-2 rounded border border-border p-3">
              <legend className="text-sm">{t('hostAccess.workspace')}</legend>
              <label className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={policy.workspace.enabled}
                  onChange={(e) =>
                    setPolicy({
                      ...policy,
                      workspace: { ...policy.workspace, enabled: e.target.checked }
                    })
                  }
                />
                {t('hostAccess.workspaceEnabled')}
              </label>
              <label className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={policy.workspace.projectIds === null}
                  onChange={(e) =>
                    setPolicy({
                      ...policy,
                      workspace: { ...policy.workspace, projectIds: e.target.checked ? null : [] }
                    })
                  }
                />
                {t('hostAccess.allProjects')}
              </label>
              {policy.workspace.projectIds !== null &&
                view.projects.map((project) => (
                  <label key={project.id} className="flex items-center gap-2 text-sm">
                    <input
                      type="checkbox"
                      checked={policy.workspace.projectIds?.includes(project.id) ?? false}
                      onChange={(e) =>
                        setPolicy({
                          ...policy,
                          workspace: {
                            ...policy.workspace,
                            projectIds: e.target.checked
                              ? [...(policy.workspace.projectIds ?? []), project.id]
                              : (policy.workspace.projectIds?.filter((id) => id !== project.id) ??
                                [])
                          }
                        })
                      }
                    />
                    {project.name}
                  </label>
                ))}
              <label className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={policy.workspace.includePaths}
                  onChange={(e) =>
                    setPolicy({
                      ...policy,
                      workspace: { ...policy.workspace, includePaths: e.target.checked }
                    })
                  }
                />
                {t('hostAccess.includePaths')}
              </label>
            </fieldset>
            <p className="text-xs text-destructive">{t('hostAccess.executeWarning')}</p>
            <p className="text-xs text-muted-foreground">{t('hostAccess.localWarning')}</p>
            {view.profiles.length === 0 && <p className="text-xs">{t('hostAccess.noProfiles')}</p>}
            {view.profiles.map((profile) => {
              const selected = policy.ssh.profiles[profile.profileId]
              return (
                <fieldset
                  key={profile.profileId}
                  className="space-y-2 rounded border border-border p-3"
                >
                  <legend className="text-sm">{profile.name}</legend>
                  <p className="break-all text-xs text-muted-foreground">
                    {profile.username}@{profile.host}:{profile.port} · {profile.authMethod}
                  </p>
                  <label className="flex items-center gap-2 text-sm">
                    <input
                      type="checkbox"
                      checked={!!selected}
                      onChange={(e) =>
                        grant(profile.profileId, e.target.checked ? emptyMcpSshGrant() : null)
                      }
                    />
                    {t('hostAccess.grantProfile')}
                  </label>
                  {selected && (
                    <>
                      {policy.profileFingerprints[profile.profileId] !== profile.fingerprint && (
                        <div className="space-y-2">
                          <p className="text-xs text-destructive">
                            {t('hostAccess.fingerprintChanged')}
                          </p>
                          <Button
                            size="sm"
                            variant="outline"
                            disabled={disabled}
                            onClick={() =>
                              setPolicy({
                                ...policy,
                                profileFingerprints: {
                                  ...policy.profileFingerprints,
                                  [profile.profileId]: profile.fingerprint
                                }
                              })
                            }
                          >
                            {t('hostAccess.reauthorizeIdentity')}
                          </Button>
                        </div>
                      )}
                      <div className="grid gap-2 sm:grid-cols-2">
                        {MCP_SSH_OPERATIONS.map((operation) => (
                          <label key={operation} className="flex items-center gap-2 text-xs">
                            <input
                              type="checkbox"
                              checked={selected.operations.includes(operation)}
                              onChange={(e) =>
                                grant(profile.profileId, {
                                  ...selected,
                                  operations: e.target.checked
                                    ? [...selected.operations, operation]
                                    : selected.operations.filter((op) => op !== operation)
                                })
                              }
                            />
                            <span>
                              {t(`hostAccess.operations.${operation}`)}
                              {HIGH_RISK.has(operation) ? ` — ${t('hostAccess.highRisk')}` : ''}
                            </span>
                          </label>
                        ))}
                      </div>
                      <label className="block text-xs">
                        {t('hostAccess.remoteRoots')}
                        <textarea
                          className="mt-1 block w-full rounded border border-border bg-background p-2"
                          rows={2}
                          value={selected.remoteRoots.join('\n')}
                          onChange={(e) =>
                            grant(profile.profileId, {
                              ...selected,
                              remoteRoots: e.target.value.split('\n')
                            })
                          }
                        />
                      </label>
                      <label className="block text-xs">
                        {t('hostAccess.localRoots')}
                        <textarea
                          className="mt-1 block w-full rounded border border-border bg-background p-2"
                          rows={2}
                          value={selected.localRoots.join('\n')}
                          onChange={(e) =>
                            grant(profile.profileId, {
                              ...selected,
                              localRoots: e.target.value.split('\n')
                            })
                          }
                        />
                      </label>
                      <label className="flex items-center gap-2 text-xs">
                        <input
                          type="checkbox"
                          checked={selected.posixJobs}
                          onChange={(e) =>
                            grant(profile.profileId, { ...selected, posixJobs: e.target.checked })
                          }
                        />
                        {t('hostAccess.posixJobs')}
                      </label>
                    </>
                  )}
                  <div className="flex gap-2">
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={disabled || !canCall(profile.profileId, 'test_connection')}
                      onClick={() => void run(profile.profileId, 'test_connection')}
                    >
                      {t('hostAccess.test')}
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={disabled || !canCall(profile.profileId, 'job_list')}
                      onClick={() => void run(profile.profileId, 'job_list')}
                    >
                      {t('hostAccess.refreshJobs')}
                    </Button>
                  </div>
                  {jobs[profile.profileId] && (
                    <ul className="space-y-1 text-xs" aria-label={t('hostAccess.jobs')}>
                      {jobs[profile.profileId].length === 0 && <li>{t('hostAccess.noJobs')}</li>}
                      {jobs[profile.profileId].map((job) => (
                        <li key={job.jobId} className="flex items-center justify-between gap-2">
                          <span>
                            {job.jobId} · {job.state}
                          </span>
                          <Button
                            size="sm"
                            variant="outline"
                            disabled={
                              disabled || job.finished || !canCall(profile.profileId, 'job_cancel')
                            }
                            onClick={() => void run(profile.profileId, 'job_cancel', job.jobId)}
                          >
                            {t('hostAccess.cancelJob')}
                          </Button>
                        </li>
                      ))}
                    </ul>
                  )}
                </fieldset>
              )
            })}
          </fieldset>
          <p className="text-xs text-muted-foreground">{t('hostAccess.saveHint')}</p>
          <Button size="sm" disabled={disabled} onClick={() => void save()}>
            {t('hostAccess.save')}
          </Button>
        </>
      )}
    </section>
  )
}
