import { RefreshCw, Sparkles } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
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
import {
  type SkillInstallMode,
  type SkillInstallSource,
  type SkillRecord,
  type SkillScope,
  type SkillsCatalogRequest,
  type SkillsInstallPlan,
  type SkillsStatus,
  skillsApi
} from '@/lib/skills-api'
import { useProjectStore } from '@/stores/project-store'

const scopes = ['all', 'global', 'project'] as const
type ScopeFilter = (typeof scopes)[number]
type WriteAction = 'install' | 'project' | 'repair' | 'fallback' | 'collision'

export default function Skills(): React.JSX.Element {
  const { t } = useTranslation('common')
  const projects = useProjectStore((state) => state.projects)
  const activeProjectId = useProjectStore((state) => state.activeProjectId)
  const [status, setStatus] = useState<SkillsStatus | null>(null)
  const [scope, setScope] = useState<ScopeFilter>('all')
  const [provider, setProvider] = useState('all')
  const [projectId, setProjectId] = useState(activeProjectId || 'all')
  const [query, setQuery] = useState('')
  const [selected, setSelected] = useState<SkillRecord | null>(null)
  const [preview, setPreview] = useState<string | null>(null)
  const [sourcePath, setSourcePath] = useState('')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [pendingAction, setPendingAction] = useState<WriteAction | null>(null)
  const [confirmToken, setConfirmToken] = useState<string | null>(null)
  const [remoteType, setRemoteType] = useState<'github' | 'npm' | 'url'>('github')
  const [remoteSource, setRemoteSource] = useState('')
  const [installTarget, setInstallTarget] = useState('global')
  const [localInstallTarget, setLocalInstallTarget] = useState('global')
  const [remoteMode, setRemoteMode] = useState<SkillInstallMode>('installAndProject')
  const [remotePlan, setRemotePlan] = useState<SkillsInstallPlan | null>(null)
  const [remoteJobId, setRemoteJobId] = useState<string | null>(null)
  const [remotePhase, setRemotePhase] = useState<string | null>(null)
  const [remoteDigestConfirmed, setRemoteDigestConfirmed] = useState(false)
  const [remoteBusy, setRemoteBusy] = useState(false)
  const [remoteError, setRemoteError] = useState<string | null>(null)
  const loadedRef = useRef(false)
  const requestInFlightRef = useRef(false)

  const request = useMemo<SkillsCatalogRequest>(() => {
    if (projectId === 'all' || !projectId) return {}
    const project = projects.find((item) => item.id === projectId)
    return { projectId, projectRoot: project?.path ?? null }
  }, [projectId, projects])

  const installableProjects = useMemo(
    () => projects.filter((project) => !project.isArchived && Boolean(project.path)),
    [projects]
  )

  const scopeFromTarget = (target: string): SkillScope | null => {
    if (target === 'global') return { type: 'global' }
    const project = installableProjects.find((item) => item.id === target)
    if (!project) return null
    return { type: 'project', projectId: project.id }
  }

  const load = useCallback(
    async (sync = false): Promise<void> => {
      if (requestInFlightRef.current) return
      requestInFlightRef.current = true
      if (!loadedRef.current) setLoading(true)
      setError(null)
      try {
        const next = sync ? await skillsApi.refresh(request) : await skillsApi.status(request)
        setStatus(next)
        loadedRef.current = true
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause))
      } finally {
        requestInFlightRef.current = false
        setLoading(false)
      }
    },
    [request]
  )

  useEffect(() => {
    void load(false)
  }, [load])

  useEffect(() => {
    return skillsApi.onCatalogChanged(() => {
      void load(false)
    })
  }, [load])

  const providers = useMemo(() => {
    const ids = new Set<string>()
    for (const skill of status?.catalog.skills ?? []) {
      for (const source of skill.sources ?? []) {
        if (source.provider) ids.add(source.provider)
      }
    }
    return ['all', ...Array.from(ids).sort()]
  }, [status])

  const skills = useMemo(() => {
    const rows = status?.catalog.skills ?? []
    const normalized = query.trim().toLowerCase()
    return rows.filter((skill) => {
      const matchesScope = scope === 'all' || skill.scope === scope
      const matchesProvider =
        provider === 'all' || skill.sources.some((source) => source.provider === provider)
      const matchesQuery =
        !normalized || `${skill.name} ${skill.description}`.toLowerCase().includes(normalized)
      return matchesScope && matchesProvider && matchesQuery
    })
  }, [provider, query, scope, status])

  useEffect(() => {
    return (
      skillsApi.onOperationProgress?.((status) => {
        if (status.jobId === remoteJobId) setRemotePhase(status.phase)
      }) ?? (() => undefined)
    )
  }, [remoteJobId])

  useEffect(() => {
    if (!selected) {
      setPreview(null)
      return
    }
    const projectRoot =
      selected.scope === 'project'
        ? projects.find((project) => project.id === selected.projectId)?.path
        : undefined
    void skillsApi
      .readSkill(selected.name, projectRoot)
      .then((content) => setPreview(content.body))
      .catch(() => setPreview(null))
  }, [projects, selected])

  useEffect(() => {
    if (!selected) {
      setLocalInstallTarget('global')
      return
    }
    if (selected.scope === 'project' && selected.projectId) {
      setLocalInstallTarget(selected.projectId)
      return
    }
    setLocalInstallTarget('global')
  }, [selected])

  const waitForRemoteJob = async (
    jobId: string,
    terminalPhase: 'preview_ready' | 'completed'
  ): Promise<unknown> => {
    for (let attempt = 0; attempt < 1200; attempt += 1) {
      const status = await skillsApi.operationStatus(jobId)
      setRemotePhase(status.phase)
      if (status.phase === terminalPhase) {
        if (status.result == null) throw new Error('SKILLS_OPERATION_RESULT_MISSING')
        return status.result
      }
      if (status.phase === 'failed' || status.phase === 'cancelled') {
        throw new Error(status.errorCode ?? status.phase)
      }
      await new Promise((resolve) => window.setTimeout(resolve, 250))
    }
    throw new Error('SKILLS_OPERATION_TIMEOUT')
  }

  const previewRemote = async (): Promise<void> => {
    if (!remoteSource.trim()) return
    setRemoteBusy(true)
    setRemoteError(null)
    setRemotePlan(null)
    try {
      const source: SkillInstallSource =
        remoteType === 'github'
          ? { type: 'github', repositoryOrUrl: remoteSource.trim() }
          : remoteType === 'npm'
            ? { type: 'npm', package: remoteSource.trim() }
            : { type: 'url', url: remoteSource.trim() }
      const scope = scopeFromTarget(installTarget)
      if (!scope) {
        setRemoteError(t('skills.chooseInstallTarget'))
        return
      }
      const start = await skillsApi.preview({
        source,
        scope,
        mode: remoteMode,
        providerIds: provider === 'all' ? [] : [provider]
      })
      setRemoteJobId(start.jobId)
      const result = (await waitForRemoteJob(start.jobId, 'preview_ready')) as SkillsInstallPlan
      setRemotePlan(result)
      setRemoteDigestConfirmed(!result.requiresDigestConfirmation)
    } catch (cause) {
      setRemoteError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setRemoteBusy(false)
    }
  }

  const installRemote = async (): Promise<void> => {
    if (!remotePlan || (remotePlan.requiresDigestConfirmation && !remoteDigestConfirmed)) return
    setRemoteBusy(true)
    setRemoteError(null)
    try {
      const start = await skillsApi.installPreview({
        previewId: remotePlan.previewId,
        confirmDigest: remoteDigestConfirmed
      })
      setRemoteJobId(start.jobId)
      await waitForRemoteJob(start.jobId, 'completed')
      toast.success(t('skills.updated'))
      setRemotePlan(null)
      setRemoteJobId(null)
      setRemotePhase(null)
      setRemoteSource('')
      await load(false)
    } catch (cause) {
      setRemoteError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setRemoteBusy(false)
    }
  }

  const runWrite = async (action: WriteAction): Promise<void> => {
    if (!selected) return
    let keepDialog = false
    try {
      if (action === 'install' || action === 'collision') {
        const localScope = scopeFromTarget(localInstallTarget)
        if (!localScope) {
          toast.error(t('skills.chooseInstallTarget'))
          return
        }
        await skillsApi.install({
          name: selected.name,
          sourcePath,
          scope: localScope.type,
          projectId: localScope.type === 'project' ? localScope.projectId : null,
          projectRoot:
            localScope.type === 'project'
              ? (installableProjects.find((project) => project.id === localScope.projectId)?.path ??
                null)
              : null,
          confirmToken: action === 'collision' ? confirmToken : null,
          fallback: status?.fallbackPolicy ?? 'ask'
        })
        setConfirmToken(null)
      } else if (action === 'project' || action === 'fallback') {
        await skillsApi.project({
          name: selected.name,
          projectId: selected.projectId,
          confirmFallback: action === 'fallback',
          fallback: action === 'fallback' ? 'copy' : (status?.fallbackPolicy ?? 'ask')
        })
      } else {
        await skillsApi.repair({
          name: selected.name,
          projectId: selected.projectId
        })
      }
      toast.success(t('skills.updated'))
      await load(false)
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause)
      if (message.includes('PROJECTION_FALLBACK_CONFIRMATION_REQUIRED')) {
        keepDialog = true
        setPendingAction('fallback')
        return
      }
      const token = message.match(/confirmToken=(\S+)/)?.[1]
      if (message.includes('UNMANAGED_COLLISION') && token) {
        keepDialog = true
        setConfirmToken(token)
        setPendingAction('collision')
        return
      }
      setError(message)
      toast.error(message)
    } finally {
      if (!keepDialog) setPendingAction(null)
    }
  }

  return (
    <div className="flex h-full min-h-0 bg-background">
      <aside className="flex w-80 min-w-64 flex-col border-r border-border">
        <div className="flex h-12 items-center justify-between border-b border-border px-4">
          <div className="flex items-center gap-2 text-sm font-semibold">
            <Sparkles size={16} /> {t('skills.title')}
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            onClick={() => void load(false)}
            disabled={loading}
            aria-label={t('skills.refresh')}
          >
            <RefreshCw size={14} className={loading ? 'animate-spin' : undefined} />
          </Button>
        </div>
        <div className="space-y-2 border-b border-border p-3">
          <input
            className="h-8 w-full rounded-md border border-input bg-background px-2 text-sm outline-none focus-visible:ring-1 focus-visible:ring-ring"
            placeholder={t('skills.search')}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            aria-label={t('skills.search')}
          />
          <label className="block text-[11px] text-muted-foreground">
            {t('skills.project')}
            <select
              className="mt-1 h-8 w-full rounded-md border border-input bg-background px-2 text-sm"
              value={projectId}
              onChange={(event) => setProjectId(event.target.value)}
              aria-label={t('skills.project')}
            >
              <option value="all">{t('skills.allProjects')}</option>
              {projects
                .filter((project) => !project.isArchived)
                .map((project) => (
                  <option key={project.id} value={project.id}>
                    {project.name}
                  </option>
                ))}
            </select>
          </label>
          <label className="block text-[11px] text-muted-foreground">
            {t('skills.provider')}
            <select
              className="mt-1 h-8 w-full rounded-md border border-input bg-background px-2 text-sm"
              value={provider}
              onChange={(event) => setProvider(event.target.value)}
              aria-label={t('skills.provider')}
            >
              {providers.map((id) => (
                <option key={id} value={id}>
                  {id === 'all' ? t('skills.allProviders') : id}
                </option>
              ))}
            </select>
          </label>
          <div className="flex gap-1" role="tablist" aria-label={t('skills.scope')}>
            {scopes.map((value) => (
              <button
                key={value}
                type="button"
                role="tab"
                aria-label={value}
                aria-selected={scope === value}
                className={`rounded px-2 py-1 text-xs capitalize ${scope === value ? 'bg-secondary text-foreground' : 'text-muted-foreground hover:bg-secondary'}`}
                onClick={() => setScope(value)}
              >
                {value === 'all'
                  ? t('skills.all')
                  : value === 'project'
                    ? t('skills.projectScope')
                    : t('skills.global')}
              </button>
            ))}
          </div>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-2">
          {loading ? (
            <div className="p-6 text-center text-xs text-muted-foreground">
              {t('skills.loading')}
            </div>
          ) : null}
          {!loading && !error && skills.length === 0 ? (
            <div className="p-6 text-center text-xs text-muted-foreground">{t('skills.empty')}</div>
          ) : null}
          {skills.map((skill) => (
            <button
              key={`${skill.scope}:${skill.projectId ?? 'global'}:${skill.name}`}
              type="button"
              onClick={() => setSelected(skill)}
              className={`mb-1 w-full rounded-md px-3 py-2 text-left hover:bg-secondary ${selected?.name === skill.name && selected.scope === skill.scope ? 'bg-secondary' : ''}`}
            >
              <div className="flex items-center justify-between gap-2 text-sm">
                <span className="truncate font-medium">{skill.name}</span>
                <span className="flex gap-1 text-[10px]">
                  {skill.conflict ? (
                    <span className="text-amber-500">{t('skills.badges.conflict')}</span>
                  ) : null}
                  {skill.drift ? (
                    <span className="text-amber-500">{t('skills.badges.drift')}</span>
                  ) : null}
                  {skill.managed ? (
                    <span className="text-sky-500">{t('skills.badges.managed')}</span>
                  ) : null}
                </span>
              </div>
              <div className="mt-1 text-xs text-muted-foreground">
                {skill.scope} · {skill.status}
              </div>
            </button>
          ))}
        </div>
      </aside>
      <main className="min-w-0 flex-1 overflow-y-auto p-6">
        {status?.stale ? (
          <div
            role="status"
            className="mb-4 rounded-md border border-amber-500/40 p-3 text-sm text-amber-700"
          >
            {t('skills.catalogStale')}
          </div>
        ) : null}
        {status?.catalog.diagnostics.length ? (
          <div
            role="status"
            className="mb-4 rounded-md border border-border p-3 text-xs text-muted-foreground"
          >
            {status.catalog.diagnostics.join('\n')}
          </div>
        ) : null}
        {error ? (
          <div
            role="alert"
            className="rounded-md border border-destructive/40 p-4 text-sm text-destructive"
          >
            {error}
          </div>
        ) : null}
        {!error ? (
          <section className="mx-auto mb-6 max-w-3xl rounded-lg border border-border p-4">
            <h2 className="text-sm font-medium">{t('skills.remoteTitle')}</h2>
            <div className="mt-3 grid gap-2 sm:grid-cols-2">
              <label className="text-xs text-muted-foreground">
                {t('skills.sourceType')}
                <select
                  className="mt-1 h-8 w-full rounded-md border border-input bg-background px-2 text-sm"
                  value={remoteType}
                  onChange={(event) => setRemoteType(event.target.value as typeof remoteType)}
                >
                  <option value="github">{t('skills.github')}</option>
                  <option value="npm">{t('skills.npm')}</option>
                  <option value="url">{t('skills.url')}</option>
                </select>
              </label>
              <label className="text-xs text-muted-foreground">
                {t('skills.source')}
                <input
                  className="mt-1 h-8 w-full rounded-md border border-input bg-background px-2 text-sm"
                  value={remoteSource}
                  onChange={(event) => setRemoteSource(event.target.value)}
                  placeholder={
                    remoteType === 'github'
                      ? 'owner/repository'
                      : remoteType === 'npm'
                        ? '@scope/package'
                        : 'https://…'
                  }
                />
              </label>
              <label className="text-xs text-muted-foreground">
                {t('skills.installTarget')}
                <select
                  className="mt-1 h-8 w-full rounded-md border border-input bg-background px-2 text-sm"
                  value={installTarget}
                  onChange={(event) => setInstallTarget(event.target.value)}
                  aria-label={t('skills.installTarget')}
                >
                  <option value="global">{t('skills.scopeGlobal')}</option>
                  {installableProjects.map((project) => (
                    <option key={project.id} value={project.id}>
                      {project.name}
                    </option>
                  ))}
                </select>
                {installableProjects.length === 0 ? (
                  <span className="mt-1 block">{t('skills.noInstallableProject')}</span>
                ) : null}
              </label>
              <label className="text-xs text-muted-foreground">
                {t('skills.install')}
                <select
                  className="mt-1 h-8 w-full rounded-md border border-input bg-background px-2 text-sm"
                  value={remoteMode}
                  onChange={(event) => setRemoteMode(event.target.value as SkillInstallMode)}
                >
                  <option value="installAndProject">{t('skills.installAndProject')}</option>
                  <option value="installOnly">{t('skills.installOnly')}</option>
                  <option value="projectionOnly">{t('skills.projectionOnly')}</option>
                </select>
              </label>
            </div>
            <div className="mt-3 flex gap-2">
              <Button
                type="button"
                onClick={() => void previewRemote()}
                disabled={remoteBusy || !remoteSource.trim() || !scopeFromTarget(installTarget)}
              >
                {remoteBusy ? t('skills.loading') : t('skills.previewSource')}
              </Button>
              {remoteJobId ? (
                <Button
                  type="button"
                  variant="outline"
                  onClick={() =>
                    void skillsApi.cancelOperation(remoteJobId).catch((cause) => {
                      setRemoteError(cause instanceof Error ? cause.message : String(cause))
                    })
                  }
                  disabled={!remoteBusy}
                >
                  {t('skills.cancel')}
                </Button>
              ) : null}
              {remotePlan ? (
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => void installRemote()}
                  disabled={
                    remoteBusy ||
                    Boolean(remotePlan.requiresDigestConfirmation && !remoteDigestConfirmed)
                  }
                >
                  {t('skills.installPreview')}
                </Button>
              ) : null}
            </div>
            {remotePhase ? (
              <div className="mt-3 text-xs text-muted-foreground" role="status">
                {remotePhase}
              </div>
            ) : null}
            {remoteError ? (
              <div className="mt-3 text-xs text-destructive" role="alert">
                {remoteError}
              </div>
            ) : null}
            {remotePlan ? (
              <div className="mt-3 rounded border border-border/60 p-3 text-xs">
                <div className="font-medium">
                  {t('skills.previewReady')}: {remotePlan.name}
                </div>
                <div className="mt-1 break-all text-muted-foreground">
                  {t('skills.previewDigest')}: {remotePlan.actualSha256}
                </div>
                {remotePlan.requiresDigestConfirmation ? (
                  <label className="mt-2 flex items-center gap-2">
                    <input
                      type="checkbox"
                      checked={remoteDigestConfirmed}
                      onChange={(event) => setRemoteDigestConfirmed(event.target.checked)}
                    />
                    {t('skills.confirmDigest')}
                  </label>
                ) : null}
              </div>
            ) : null}
          </section>
        ) : null}
        {!error && selected ? (
          <div className="mx-auto max-w-3xl">
            <div className="mb-6 flex items-start justify-between">
              <div>
                <h1 className="text-xl font-semibold">{selected.name}</h1>
                <p className="mt-1 text-sm text-muted-foreground">
                  {selected.description || 'No description'}
                </p>
              </div>
              <span className="rounded bg-secondary px-2 py-1 text-xs">{selected.scope}</span>
            </div>
            <section className="rounded-lg border border-border p-4">
              <h2 className="text-sm font-medium">{t('skills.canonical')}</h2>
              <div className="mt-2 space-y-1 text-xs text-muted-foreground">
                <div>
                  {t('skills.digest')}: {selected.digest}
                </div>
                <div>
                  {t('skills.metadata')}: {selected.metadataDigest}
                </div>
              </div>
            </section>
            <section className="mt-4 rounded-lg border border-border p-4">
              <h2 className="text-sm font-medium">{t('skills.sources')}</h2>
              <div className="mt-3 space-y-2 text-xs text-muted-foreground">
                {selected.sources.map((source) => (
                  <div key={source.skillMdPath} className="rounded border border-border/60 p-2">
                    <div>{source.provider}</div>
                    <div className="break-all">{source.skillMdPath}</div>
                    <div>
                      {t('skills.digest')}: {source.digest}
                    </div>
                  </div>
                ))}
              </div>
            </section>
            {preview ? (
              <section className="mt-4 rounded-lg border border-border p-4">
                <h2 className="text-sm font-medium">Markdown</h2>
                <pre className="mt-3 max-h-80 overflow-auto whitespace-pre-wrap text-xs">
                  {preview}
                </pre>
              </section>
            ) : null}
            <label className="mt-4 block text-xs text-muted-foreground">
              {t('skills.installTarget')}
              <select
                className="mt-1 h-8 w-full rounded-md border border-input bg-background px-2 text-sm"
                value={localInstallTarget}
                onChange={(event) => setLocalInstallTarget(event.target.value)}
                aria-label={t('skills.localInstallTarget')}
              >
                <option value="global">{t('skills.scopeGlobal')}</option>
                {installableProjects.map((project) => (
                  <option key={project.id} value={project.id}>
                    {project.name}
                  </option>
                ))}
              </select>
            </label>
            <label className="mt-4 block text-xs text-muted-foreground">
              {t('skills.sourcePath')}
              <input
                className="mt-1 h-8 w-full rounded-md border border-input bg-background px-2 text-sm"
                value={sourcePath}
                onChange={(event) => setSourcePath(event.target.value)}
                placeholder={t('skills.sourcePathPlaceholder')}
                aria-label={t('skills.sourcePath')}
              />
            </label>
            <div className="mt-4 flex gap-2">
              <Button
                type="button"
                disabled={!sourcePath || !scopeFromTarget(localInstallTarget)}
                onClick={() => setPendingAction('install')}
              >
                {t('skills.install')}
              </Button>
              <Button type="button" variant="outline" onClick={() => setPendingAction('project')}>
                {t('skills.projectAction')}
              </Button>
              <Button type="button" variant="outline" onClick={() => setPendingAction('repair')}>
                {t('skills.repair')}
              </Button>
            </div>
          </div>
        ) : null}
        {!error && !loading && !selected ? (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
            {t('skills.selectPrompt')}
          </div>
        ) : null}
      </main>
      <AlertDialog
        open={pendingAction !== null}
        onOpenChange={(open) => {
          if (!open) setPendingAction(null)
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {pendingAction === 'fallback'
                ? t('skills.confirm.fallbackTitle')
                : pendingAction === 'collision'
                  ? t('skills.confirm.collisionTitle')
                  : pendingAction === 'repair'
                    ? t('skills.confirm.repairTitle')
                    : pendingAction === 'project'
                      ? t('skills.confirm.projectTitle')
                      : t('skills.confirm.installTitle')}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {pendingAction === 'fallback'
                ? t('skills.confirm.fallbackDescription')
                : pendingAction === 'collision'
                  ? t('skills.confirm.collisionDescription')
                  : t('skills.confirm.writeDescription')}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t('skills.confirm.cancel')}</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (pendingAction) void runWrite(pendingAction)
              }}
            >
              {t('skills.confirm.confirm')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
