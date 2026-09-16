import { RefreshCw, Sparkles } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
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
  type SkillRecord,
  type SkillsCatalogRequest,
  type SkillsStatus,
  skillsApi
} from '@/lib/skills-api'
import { useProjectStore } from '@/stores/project-store'

const scopes = ['all', 'global', 'project'] as const
type ScopeFilter = (typeof scopes)[number]
type WriteAction = 'install' | 'project' | 'repair' | 'fallback' | 'collision'

export default function Skills(): React.JSX.Element {
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
  const loadedRef = useRef(false)
  const requestInFlightRef = useRef(false)

  const request = useMemo<SkillsCatalogRequest>(() => {
    if (projectId === 'all' || !projectId) return {}
    const project = projects.find((item) => item.id === projectId)
    return { projectId, projectRoot: project?.path ?? null }
  }, [projectId, projects])

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

  const runWrite = async (action: WriteAction): Promise<void> => {
    if (!selected) return
    let keepDialog = false
    try {
      if (action === 'install' || action === 'collision') {
        await skillsApi.install({
          name: selected.name,
          sourcePath,
          scope: selected.scope,
          projectId: selected.projectId,
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
      toast.success('Skills updated')
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
            <Sparkles size={16} /> Skills
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            onClick={() => void load(false)}
            disabled={loading}
            aria-label="Refresh skills"
          >
            <RefreshCw size={14} className={loading ? 'animate-spin' : undefined} />
          </Button>
        </div>
        <div className="space-y-2 border-b border-border p-3">
          <input
            className="h-8 w-full rounded-md border border-input bg-background px-2 text-sm outline-none focus-visible:ring-1 focus-visible:ring-ring"
            placeholder="Search skills"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            aria-label="Search skills"
          />
          <label className="block text-[11px] text-muted-foreground">
            Project
            <select
              className="mt-1 h-8 w-full rounded-md border border-input bg-background px-2 text-sm"
              value={projectId}
              onChange={(event) => setProjectId(event.target.value)}
              aria-label="Filter by project"
            >
              <option value="all">All registered projects</option>
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
            Provider
            <select
              className="mt-1 h-8 w-full rounded-md border border-input bg-background px-2 text-sm"
              value={provider}
              onChange={(event) => setProvider(event.target.value)}
              aria-label="Filter by provider"
            >
              {providers.map((id) => (
                <option key={id} value={id}>
                  {id}
                </option>
              ))}
            </select>
          </label>
          <div className="flex gap-1" role="tablist" aria-label="Skill scope">
            {scopes.map((value) => (
              <button
                key={value}
                type="button"
                role="tab"
                aria-selected={scope === value}
                className={`rounded px-2 py-1 text-xs capitalize ${scope === value ? 'bg-secondary text-foreground' : 'text-muted-foreground hover:bg-secondary'}`}
                onClick={() => setScope(value)}
              >
                {value}
              </button>
            ))}
          </div>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-2">
          {loading ? (
            <div className="p-6 text-center text-xs text-muted-foreground">Loading skills…</div>
          ) : null}
          {!loading && !error && skills.length === 0 ? (
            <div className="p-6 text-center text-xs text-muted-foreground">No skills found</div>
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
                  {skill.conflict ? <span className="text-amber-500">conflict</span> : null}
                  {skill.drift ? <span className="text-amber-500">drift</span> : null}
                  {skill.managed ? <span className="text-sky-500">managed</span> : null}
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
            Catalog is stale. Refresh to run a full rescan.
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
              <h2 className="text-sm font-medium">Canonical</h2>
              <div className="mt-2 space-y-1 text-xs text-muted-foreground">
                <div>digest: {selected.digest}</div>
                <div>metadata: {selected.metadataDigest}</div>
              </div>
            </section>
            <section className="mt-4 rounded-lg border border-border p-4">
              <h2 className="text-sm font-medium">Sources</h2>
              <div className="mt-3 space-y-2 text-xs text-muted-foreground">
                {selected.sources.map((source) => (
                  <div key={source.skillMdPath} className="rounded border border-border/60 p-2">
                    <div>{source.provider}</div>
                    <div className="break-all">{source.skillMdPath}</div>
                    <div>digest: {source.digest}</div>
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
              Source path for install
              <input
                className="mt-1 h-8 w-full rounded-md border border-input bg-background px-2 text-sm"
                value={sourcePath}
                onChange={(event) => setSourcePath(event.target.value)}
                placeholder="/absolute/path/to/SKILL.md"
                aria-label="Install source path"
              />
            </label>
            <div className="mt-4 flex gap-2">
              <Button
                type="button"
                disabled={!sourcePath}
                onClick={() => setPendingAction('install')}
              >
                Install
              </Button>
              <Button type="button" variant="outline" onClick={() => setPendingAction('project')}>
                Project
              </Button>
              <Button type="button" variant="outline" onClick={() => setPendingAction('repair')}>
                Repair projection
              </Button>
            </div>
          </div>
        ) : null}
        {!error && !loading && !selected ? (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
            Select a skill to inspect its sources and status.
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
                ? 'Copy projection instead of symlink?'
                : pendingAction === 'collision'
                  ? 'Overwrite unmanaged collision?'
                  : pendingAction === 'repair'
                    ? 'Repair this projection?'
                    : pendingAction === 'project'
                      ? 'Create provider projection?'
                      : 'Install this skill?'}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {pendingAction === 'fallback'
                ? 'Symlink is unavailable. Confirm to copy the canonical skill into the provider root.'
                : pendingAction === 'collision'
                  ? 'A different digest already occupies this canonical path. Confirm to replace the unmanaged file.'
                  : 'This writes only Termul-owned canonical files and recorded projections. Unmanaged files are left unchanged.'}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (pendingAction) void runWrite(pendingAction)
              }}
            >
              Confirm
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
