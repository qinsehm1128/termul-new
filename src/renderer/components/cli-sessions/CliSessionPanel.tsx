import type { CliSessionAgentId, DiscoveredCliSession } from '@shared/types/cli-session.types'
import { CLI_SESSION_AGENT_IDS, CLI_SESSION_AGENT_LABELS } from '@shared/types/cli-session.types'
import { ChevronDown, RefreshCw, SlidersHorizontal } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import { AgentIcon } from '@/components/agents/AgentIcon'
import { CliSessionResumeDialog } from '@/components/cli-sessions/CliSessionResumeDialog'
import {
  ListEmptyState,
  ListLoadingState,
  ListPanelHeader,
  ListRow,
  ListRowMeta,
  ListScopeSwitch,
  pathBasename
} from '@/components/lists'
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'
import { useRuntimeTranslation } from '@/i18n/use-runtime-translation'
import { launchAgentResumeInPane } from '@/lib/agent-launch'
import { getBuiltInAgent } from '@/lib/agents/agent-registry'
import { cliSessionApi } from '@/lib/api'
import { loadCliResumeDefaults } from '@/lib/cli-resume-defaults'
import {
  type CliSessionGroup,
  type CliSessionSort,
  filterCliSessions,
  groupCliSessions,
  sortCliSessions
} from '@/lib/cli-session-list'
import { buildCliSessionScopePaths, type CliSessionScopeMode } from '@/lib/cli-session-scope'
import { cliSessionListTitle } from '@/lib/cli-session-title'
import { formatRelativeTime } from '@/lib/git-time'
import { logFrontendError } from '@/lib/log-api'
import { cn } from '@/lib/utils'
import { getDefaultCwdForProject } from '@/lib/worktree-context'
import { useCliSessionPanelVisible } from '@/stores/cli-session-panel-store'
import { useActiveProject } from '@/stores/project-store'
import { useActiveTerminal } from '@/stores/terminal-store'
import { useWorkspaceStore } from '@/stores/workspace-store'

const HYDRATE_BATCH = 6
const SCAN_TIMEOUT_MS = 12_000
const ALL_AGENTS = new Set<CliSessionAgentId>(CLI_SESSION_AGENT_IDS)

interface CliSessionPanelProps {
  className?: string
  /** Ignore the persisted panel store (mobile sheet). */
  forceVisible?: boolean
}

export function CliSessionPanel({
  className,
  forceVisible = false
}: CliSessionPanelProps): React.JSX.Element | null {
  const t = useRuntimeTranslation('shell')
  const storeVisible = useCliSessionPanelVisible()
  const isVisible = forceVisible || storeVisible
  const activeProject = useActiveProject()
  const activeTerminal = useActiveTerminal()
  const [scopeMode, setScopeMode] = useState<CliSessionScopeMode>('project')
  const [visibleAgents, setVisibleAgents] = useState<Set<CliSessionAgentId>>(ALL_AGENTS)
  const [sort, setSort] = useState<CliSessionSort>('updated')
  const [group, setGroup] = useState<CliSessionGroup>('agent')
  const [hideEmpty, setHideEmpty] = useState(false)
  const [search, setSearch] = useState('')
  const [sessions, setSessions] = useState<DiscoveredCliSession[]>([])
  const [issues, setIssues] = useState<number>(0)
  const [loading, setLoading] = useState(false)
  const [expandedId, setExpandedId] = useState<string | null>(null)
  const [resumeSession, setResumeSession] = useState<DiscoveredCliSession | null>(null)
  const [resumeBusy, setResumeBusy] = useState(false)
  const [defaultsByAgent, setDefaultsByAgent] = useState<
    Partial<Record<CliSessionAgentId, string>>
  >({})
  const requestIdRef = useRef(0)

  const directoryPath =
    activeTerminal?.cwd ?? (activeProject ? getDefaultCwdForProject(activeProject.id) : null)
  const scopePaths = useMemo(
    () =>
      buildCliSessionScopePaths({
        mode: scopeMode,
        directoryPath,
        projectPath: activeProject?.path,
        worktreePaths: activeProject?.worktrees?.map((worktree) => worktree.path)
      }),
    [scopeMode, directoryPath, activeProject?.path, activeProject?.worktrees]
  )

  const hydrateSessions = useCallback(
    async (scanned: DiscoveredCliSession[], requestId: number): Promise<void> => {
      const pending = scanned.filter((session) => !session.resumable)
      for (let index = 0; index < pending.length; index += HYDRATE_BATCH) {
        if (requestIdRef.current !== requestId) return
        const chunk = pending.slice(index, index + HYDRATE_BATCH)
        try {
          const result = await cliSessionApi.resolveSessions({
            files: chunk.map((session) => ({
              agentId: session.agentId,
              filePath: session.filePath
            }))
          })
          if (requestIdRef.current !== requestId) return
          setSessions((current) =>
            current.map((existing) => {
              const next = result.sessions.find(
                (session) =>
                  session.agentId === existing.agentId && session.filePath === existing.filePath
              )
              if (!next) return existing
              return {
                ...existing,
                ...next,
                id: existing.id,
                cwd: existing.cwd ?? next.cwd
              }
            })
          )
        } catch (error) {
          if (requestIdRef.current !== requestId) return
          const message = error instanceof Error ? error.message : String(error)
          void logFrontendError({
            source: 'CliSessionPanel',
            message: `cli session hydrate failed: ${message}`
          })
        }
      }
    },
    []
  )

  const refresh = useCallback(async (): Promise<void> => {
    const requestId = requestIdRef.current + 1
    requestIdRef.current = requestId
    if (scopeMode !== 'all' && (!scopePaths || scopePaths.length === 0)) {
      setSessions([])
      setIssues(0)
      setLoading(false)
      return
    }
    setLoading(true)
    try {
      const result = await Promise.race([
        cliSessionApi.listSessions({
          scopePaths,
          limit: 80
        }),
        new Promise<never>((_, reject) => {
          window.setTimeout(
            () => reject(new Error(t('cliSessions.scanFailed', 'CLI session scan failed'))),
            SCAN_TIMEOUT_MS
          )
        })
      ])
      if (requestIdRef.current !== requestId) return
      setSessions(result.sessions)
      setIssues(result.issues.length)
      setLoading(false)
      if (result.issues.length > 0) {
        void logFrontendError({
          level: 'warn',
          source: 'CliSessionPanel',
          message: `cli session scan issues=${result.issues.length}`
        })
      }
      void hydrateSessions(result.sessions, requestId)
    } catch (error) {
      if (requestIdRef.current !== requestId) return
      const message = error instanceof Error ? error.message : String(error)
      toast.error(message)
      void logFrontendError({
        source: 'CliSessionPanel',
        message
      })
      setLoading(false)
    }
  }, [hydrateSessions, scopeMode, scopePaths, t])

  useEffect(() => {
    if (!isVisible) return
    void loadCliResumeDefaults().then((defaults) => {
      setDefaultsByAgent(defaults.extraArgsByAgentId)
    })
    const handle = window.setTimeout(() => {
      void refresh()
    }, 200)
    return () => {
      window.clearTimeout(handle)
    }
  }, [isVisible, refresh])

  const filtered = useMemo(
    () => sortCliSessions(filterCliSessions(sessions, search, hideEmpty, visibleAgents), sort),
    [hideEmpty, search, sessions, sort, visibleAgents]
  )
  const grouped = useMemo(() => groupCliSessions(filtered, group), [filtered, group])

  const handleResume = async (onceExtraArgs: string): Promise<void> => {
    if (!resumeSession || !activeProject) return
    const def = getBuiltInAgent(resumeSession.agentId)
    const paneId = useWorkspaceStore.getState().activePaneId
    if (!def || !paneId) {
      toast.error(t('cliSessions.noPane', 'No active pane or agent definition'))
      return
    }
    setResumeBusy(true)
    try {
      const cwd = resumeSession.cwd || directoryPath || activeProject.path || ''
      const result = await launchAgentResumeInPane(
        paneId,
        activeProject.id,
        cwd,
        def,
        resumeSession,
        defaultsByAgent[resumeSession.agentId] ?? '',
        onceExtraArgs
      )
      if (!result.success) {
        toast.error(result.error || t('cliSessions.resumeFailed', 'Failed to resume session'))
        void logFrontendError({
          source: 'CliSessionPanel',
          message: `resume failed agent=${resumeSession.agentId} session=${resumeSession.sessionId}`
        })
        return
      }
      setResumeSession(null)
    } finally {
      setResumeBusy(false)
    }
  }

  if (!isVisible) return null

  const emptyCopy =
    scopeMode !== 'all' && (!scopePaths || scopePaths.length === 0)
      ? t(
          'cliSessions.emptyNoPath',
          'Open a project or terminal to list sessions for the current path.'
        )
      : t('cliSessions.empty', 'No CLI sessions found for this scope.')

  return (
    <div
      className={
        className ??
        'relative flex h-full w-full min-w-0 flex-col overflow-hidden bg-sidebar text-foreground'
      }
      data-testid="cli-session-panel"
    >
      <ListPanelHeader
        title={t('cliSessions.title', 'CLI Sessions')}
        shown={filtered.length}
        total={sessions.length}
        countLabel={t('cliSessions.shownCount', '{{shown}} shown · {{total}} total', {
          shown: filtered.length,
          total: sessions.length
        })}
        search={search}
        onSearchChange={setSearch}
        searchLabel={t('cliSessions.search', 'Search CLI sessions')}
        clearSearchLabel={t('cliSessions.clearSearch', 'Clear CLI session search')}
        actions={
          <>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                  aria-label={t('cliSessions.viewMenu', 'View')}
                  title={t('cliSessions.viewMenu', 'View')}
                >
                  <SlidersHorizontal size={14} />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-56">
                <DropdownMenuLabel>{t('cliSessions.viewMenu', 'View')}</DropdownMenuLabel>
                <DropdownMenuRadioGroup
                  value={sort}
                  onValueChange={(value) => setSort(value as CliSessionSort)}
                >
                  <DropdownMenuRadioItem value="updated">
                    {t('cliSessions.sortUpdated', 'Sort by updated')}
                  </DropdownMenuRadioItem>
                  <DropdownMenuRadioItem value="created">
                    {t('cliSessions.sortCreated', 'Sort by created')}
                  </DropdownMenuRadioItem>
                </DropdownMenuRadioGroup>
                <DropdownMenuSeparator />
                <DropdownMenuRadioGroup
                  value={group}
                  onValueChange={(value) => setGroup(value as CliSessionGroup)}
                >
                  <DropdownMenuRadioItem value="agent">
                    {t('cliSessions.groupByAgent', 'Group by agent')}
                  </DropdownMenuRadioItem>
                  <DropdownMenuRadioItem value="folder">
                    {t('cliSessions.groupByFolder', 'Group by folder')}
                  </DropdownMenuRadioItem>
                </DropdownMenuRadioGroup>
                <DropdownMenuSeparator />
                <DropdownMenuCheckboxItem
                  checked={hideEmpty}
                  onCheckedChange={(checked) => setHideEmpty(checked === true)}
                >
                  {t('cliSessions.hideEmpty', 'Hide empty sessions')}
                </DropdownMenuCheckboxItem>
                <DropdownMenuSeparator />
                <DropdownMenuLabel>{t('cliSessions.agent', 'Agent')}</DropdownMenuLabel>
                {CLI_SESSION_AGENT_IDS.map((agentId) => (
                  <DropdownMenuCheckboxItem
                    key={agentId}
                    checked={visibleAgents.has(agentId)}
                    onCheckedChange={(checked) => {
                      setVisibleAgents((current) => {
                        const next = new Set(current)
                        if (checked) next.add(agentId)
                        else next.delete(agentId)
                        return next
                      })
                    }}
                  >
                    {CLI_SESSION_AGENT_LABELS[agentId]}
                  </DropdownMenuCheckboxItem>
                ))}
              </DropdownMenuContent>
            </DropdownMenu>
            <button
              type="button"
              onClick={() => void refresh()}
              disabled={loading}
              className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-40"
              title={t('cliSessions.refresh', 'Refresh')}
              aria-label={t('cliSessions.refreshAria', 'Refresh CLI sessions')}
            >
              <RefreshCw size={14} className={loading ? 'animate-spin' : undefined} />
            </button>
          </>
        }
      >
        <ListScopeSwitch
          value={scopeMode}
          onChange={setScopeMode}
          ariaLabel={t('cliSessions.scopeAria', 'CLI session scope')}
          options={[
            {
              value: 'directory',
              label: t('cliSessions.scopeDirectory', 'This directory')
            },
            {
              value: 'project',
              label: t('cliSessions.scopeProject', 'This project')
            },
            {
              value: 'all',
              label: t('cliSessions.scopeAll', 'All sessions')
            }
          ]}
        />
      </ListPanelHeader>

      <div className="min-h-0 flex-1 overflow-y-auto">
        {loading ? (
          <ListLoadingState
            density="comfortable"
            label={t('cliSessions.loading', 'Scanning session files…')}
          />
        ) : grouped.length === 0 ? (
          <ListEmptyState message={emptyCopy} />
        ) : (
          grouped.map((section) => (
            <section key={section.key} className="border-b border-sidebar-border/60">
              <h3 className="flex items-center justify-between px-3 py-2 text-2xs text-muted-foreground">
                <span>
                  {group === 'agent'
                    ? (CLI_SESSION_AGENT_LABELS[section.key as CliSessionAgentId] ?? section.label)
                    : section.label}
                </span>
                <span className="tabular-nums">{section.sessions.length}</span>
              </h3>
              {section.sessions.map((session) => {
                const title = cliSessionListTitle(
                  session,
                  t('cliSessions.loadingId', 'Reading session title…'),
                  t('cliSessions.untitled', 'Untitled session')
                )
                const folder = pathBasename(session.cwd) || session.cwd
                const preview = folder || t('cliSessions.unknownCwd', 'Unknown cwd')
                const expanded = expandedId === session.id
                return (
                  <ListRow
                    key={session.id}
                    density="comfortable"
                    disabled={!session.resumable}
                    title={
                      <span className="flex min-w-0 items-center gap-1.5">
                        <AgentIcon agentId={session.agentId} className="size-3.5" />
                        <span className="truncate">{title}</span>
                      </span>
                    }
                    titleAttr={title}
                    preview={preview}
                    meta={
                      <ListRowMeta
                        items={[
                          CLI_SESSION_AGENT_LABELS[session.agentId],
                          session.updatedAt || session.createdAt
                            ? formatRelativeTime(session.updatedAt ?? session.createdAt ?? '')
                            : null,
                          session.messageCount > 0
                            ? t('cliSessions.messageCount', '{{count}} messages', {
                                count: session.messageCount
                              })
                            : null,
                          session.resumable
                            ? null
                            : t('cliSessions.notResumable', 'Not ready to resume')
                        ]}
                      />
                    }
                    trailing={
                      <>
                        <button
                          type="button"
                          className="h-7 rounded-md px-2 text-2xs text-foreground hover:bg-sidebar-accent"
                          disabled={!session.resumable}
                          onClick={(event) => {
                            event.stopPropagation()
                            setResumeSession(session)
                          }}
                        >
                          {t('cliSessions.resumeRow', 'Resume')}
                        </button>
                        <button
                          type="button"
                          className="inline-flex size-7 items-center justify-center rounded-md hover:bg-sidebar-accent hover:text-foreground"
                          aria-expanded={expanded}
                          aria-label={t('cliSessions.expandDetails', 'Show session details')}
                          onClick={(event) => {
                            event.stopPropagation()
                            setExpandedId((current) => (current === session.id ? null : session.id))
                          }}
                        >
                          <ChevronDown
                            className={cn(
                              'size-3.5 transition-transform',
                              expanded && 'rotate-180'
                            )}
                          />
                        </button>
                      </>
                    }
                    details={
                      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-2 gap-y-1">
                        <dt>{t('cliSessions.detailsCwd', 'Directory')}</dt>
                        <dd className="truncate" title={session.cwd ?? undefined}>
                          {session.cwd ?? t('cliSessions.unknownCwd', 'Unknown cwd')}
                        </dd>
                        <dt>{t('cliSessions.detailsId', 'Session id')}</dt>
                        <dd className="truncate" title={session.sessionId || undefined}>
                          {session.sessionId || '—'}
                        </dd>
                        <dt>{t('cliSessions.detailsLog', 'Log file')}</dt>
                        <dd className="truncate" title={session.filePath}>
                          {session.filePath}
                        </dd>
                      </dl>
                    }
                    expanded={expanded}
                    onClick={() =>
                      setExpandedId((current) => (current === session.id ? null : session.id))
                    }
                  />
                )
              })}
            </section>
          ))
        )}
      </div>

      {issues > 0 ? (
        <div className="border-t border-border px-3 py-2 text-2xs text-muted-foreground">
          {issues === 1
            ? t('cliSessions.issueCount_one', '{{count}} scan issue (see logs)', { count: issues })
            : t('cliSessions.issueCount_other', '{{count}} scan issues (see logs)', {
                count: issues
              })}
        </div>
      ) : null}

      <CliSessionResumeDialog
        session={resumeSession}
        defaultExtraArgs={resumeSession ? (defaultsByAgent[resumeSession.agentId] ?? '') : ''}
        busy={resumeBusy}
        onOpenChange={(open) => {
          if (!open) setResumeSession(null)
        }}
        onConfirm={(onceExtraArgs) => {
          void handleResume(onceExtraArgs)
        }}
      />
    </div>
  )
}
