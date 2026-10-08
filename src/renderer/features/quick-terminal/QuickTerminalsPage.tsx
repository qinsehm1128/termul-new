import type { QuickTerminalRecord, QuickTerminalTarget } from '@shared/types/quick-terminal.types'
import { FolderPlus, MoreHorizontal, Plus, Terminal as TerminalIcon } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useParams } from 'react-router-dom'
import { toast } from 'sonner'
import {
  ListEmptyState,
  ListPanelHeader,
  ListRow,
  ListRowMeta,
  ListRowStatus
} from '@/components/lists'
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
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from '@/components/ui/dialog'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { formatRelativeTime } from '@/lib/git-time'
import { isTauriContext } from '@/lib/tauri-runtime'
import { isTerminalRunning } from '@/lib/terminal-board'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import type { Project } from '@/types/project'
import { QuickTerminalView } from './QuickTerminalView'
import { quickTerminalApi } from './quick-terminal-api'
import { failureMessage, quickTerminalName, useQuickTerminalStore } from './quick-terminal-store'

/**
 * A project's quick terminal starts where its work happens: the active
 * worktree when there is one, else the project root. Remote clients cannot
 * name worktrees, so they always use the root.
 */
export function projectQuickTerminalTarget(
  project: Project,
  allowWorktree: boolean
): QuickTerminalTarget | null {
  if (!project.path) return null
  const worktree = allowWorktree
    ? project.worktrees?.find((candidate) => candidate.id === project.activeWorktreeId)
    : undefined
  return worktree
    ? {
        kind: 'worktree',
        projectId: project.id,
        worktreePath: worktree.path,
        worktreeBranch: worktree.branch
      }
    : { kind: 'project_root', projectId: project.id, projectRoot: project.path }
}

function targetLabel(
  record: QuickTerminalRecord,
  t: (key: string, options?: Record<string, string>) => string
): string {
  switch (record.target.kind) {
    case 'workspace':
      return t('targetWorkspace')
    case 'project_root':
      return t('targetProject')
    case 'worktree':
      return t('targetWorktree', { branch: record.target.worktreeBranch })
  }
}

export default function QuickTerminalsPage(): React.JSX.Element {
  const { t } = useTranslation('quickTerminal')
  const translate = (key: string, options?: Record<string, string>): string =>
    String(t(key as never, options as never))
  const navigate = useNavigate()
  const { quickTerminalId } = useParams<{ quickTerminalId: string }>()
  const records = useQuickTerminalStore((state) => state.records)
  const loaded = useQuickTerminalStore((state) => state.loaded)
  const loadError = useQuickTerminalStore((state) => state.loadError)
  const load = useQuickTerminalStore((state) => state.load)
  const createQuickTerminal = useQuickTerminalStore((state) => state.create)
  const rename = useQuickTerminalStore((state) => state.rename)
  const remove = useQuickTerminalStore((state) => state.remove)
  const closeShell = useQuickTerminalStore((state) => state.close)
  const projects = useProjectStore((state) => state.projects)
  // Joined so the selector returns a stable value; a fresh array would rerender every store update.
  const runningKey = useTerminalStore((state) =>
    state.terminals
      .filter((terminal) => terminal.quickTerminalId && isTerminalRunning(terminal))
      .map((terminal) => terminal.quickTerminalId)
      .join('\n')
  )
  const running = useMemo(() => new Set(runningKey.split('\n')), [runningKey])
  const [query, setQuery] = useState('')
  const [renaming, setRenaming] = useState<QuickTerminalRecord | null>(null)
  const [renameValue, setRenameValue] = useState('')
  const [deleting, setDeleting] = useState<QuickTerminalRecord | null>(null)

  useEffect(() => {
    void load()
    return quickTerminalApi.onChanged(() => void load())
  }, [load])

  const openProjects = useMemo(
    () => projects.filter((project) => !project.isArchived && project.path),
    [projects]
  )
  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase()
    if (!needle) return records
    return records.filter((record) =>
      `${quickTerminalName(record)} ${record.cwd}`.toLowerCase().includes(needle)
    )
  }, [records, query])
  const selected = records.find((record) => record.id === quickTerminalId)

  const createIn = async (target: QuickTerminalTarget): Promise<void> => {
    const result = await createQuickTerminal(target)
    if (result.success && result.data) navigate(`/quick-terminals/${result.data.id}`)
    else toast.error(failureMessage(result, t('createFailed')))
  }

  const submitRename = async (): Promise<void> => {
    if (!renaming) return
    const result = await rename(renaming.id, renameValue.trim() || null)
    if (!result.success) toast.error(failureMessage(result, t('renameFailed')))
    setRenaming(null)
  }

  const closeRecord = async (id: string): Promise<void> => {
    const result = await closeShell(id)
    if (!result.success) toast.error(failureMessage(result, t('closeFailed')))
  }

  const confirmDelete = async (): Promise<void> => {
    if (!deleting) return
    const target = deleting
    setDeleting(null)
    const result = await remove(target.id)
    if (!result.success) {
      toast.error(failureMessage(result, t('deleteFailed')))
      return
    }
    if (target.id === quickTerminalId) navigate('/quick-terminals')
  }

  const newMenu = (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button size="icon" variant="ghost" className="size-6" aria-label={t('newMenu')}>
          <Plus className="size-3.5" aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-64">
        <DropdownMenuItem onSelect={() => void createIn({ kind: 'workspace' })}>
          <FolderPlus className="mr-2 size-4" aria-hidden="true" />
          <div className="min-w-0">
            <div>{t('newWorkspace')}</div>
            <div className="text-2xs text-muted-foreground">{t('newWorkspaceHint')}</div>
          </div>
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuLabel className="text-2xs text-muted-foreground">
          {t('projectsSection')}
        </DropdownMenuLabel>
        {openProjects.length === 0 ? (
          <DropdownMenuItem disabled>{t('noProjects')}</DropdownMenuItem>
        ) : (
          openProjects.map((project) => {
            const target = projectQuickTerminalTarget(project, isTauriContext())
            return target ? (
              <DropdownMenuItem key={project.id} onSelect={() => void createIn(target)}>
                <TerminalIcon className="mr-2 size-4" aria-hidden="true" />
                <span className="truncate">{project.name}</span>
              </DropdownMenuItem>
            ) : null
          })
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  )

  return (
    <div className="flex h-full min-h-0 bg-background">
      <aside className="flex w-64 shrink-0 flex-col border-r border-sidebar-border/70 bg-sidebar">
        <ListPanelHeader
          title={t('title')}
          countLabel={
            records.length > 0
              ? t('count', { shown: String(visible.length), total: String(records.length) })
              : undefined
          }
          search={query}
          onSearchChange={setQuery}
          searchLabel={t('search')}
          searchPlaceholder={t('searchPlaceholder')}
          clearSearchLabel={t('clearSearch')}
          actions={newMenu}
        />
        <div className="min-h-0 flex-1 overflow-y-auto py-1">
          {loadError ? (
            <p role="alert" className="px-3 py-2 text-xs text-destructive">
              {t('loadFailed')}
            </p>
          ) : null}
          {loaded && records.length === 0 && !loadError ? (
            <ListEmptyState title={t('empty')} message={t('emptyHint')} />
          ) : null}
          {records.length > 0 && visible.length === 0 ? (
            <ListEmptyState message={t('noMatches')} />
          ) : null}
          {visible.map((record) => (
            <ListRow
              key={record.id}
              title={
                <span className="flex min-w-0 items-center gap-1.5">
                  <span className="min-w-0 truncate">{quickTerminalName(record)}</span>
                  {running.has(record.id) ? (
                    <ListRowStatus status="working" label={t('running')} />
                  ) : null}
                </span>
              }
              titleAttr={record.cwd}
              active={record.id === quickTerminalId}
              onClick={() => navigate(`/quick-terminals/${record.id}`)}
              meta={
                <ListRowMeta
                  items={[targetLabel(record, translate), formatRelativeTime(record.updatedAtUtc)]}
                />
              }
              trailing={
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button
                      size="icon"
                      variant="ghost"
                      className="size-6 opacity-0 group-hover/list-row:opacity-100 focus-visible:opacity-100"
                      aria-label={t('actions')}
                    >
                      <MoreHorizontal className="size-3.5" aria-hidden="true" />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end">
                    <DropdownMenuItem
                      onSelect={() => {
                        setRenameValue(record.title ?? '')
                        setRenaming(record)
                      }}
                    >
                      {t('rename')}
                    </DropdownMenuItem>
                    {record.terminalId ? (
                      <DropdownMenuItem onSelect={() => void closeRecord(record.id)}>
                        {t('close')}
                      </DropdownMenuItem>
                    ) : null}
                    <DropdownMenuItem
                      className="text-destructive"
                      onSelect={() => setDeleting(record)}
                    >
                      {t('delete')}
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              }
            />
          ))}
        </div>
      </aside>

      <main className="min-w-0 flex-1">
        {selected ? (
          <QuickTerminalView key={selected.id} record={selected} />
        ) : (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
            {t('selectHint')}
          </div>
        )}
      </main>

      <Dialog open={renaming !== null} onOpenChange={(open) => !open && setRenaming(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('renameTitle')}</DialogTitle>
          </DialogHeader>
          <Input
            autoFocus
            value={renameValue}
            placeholder={t('renamePlaceholder')}
            onChange={(event) => setRenameValue(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') void submitRename()
            }}
          />
          <DialogFooter>
            <Button variant="outline" onClick={() => setRenaming(null)}>
              {t('cancel')}
            </Button>
            <Button onClick={() => void submitRename()}>{t('save')}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <AlertDialog open={deleting !== null} onOpenChange={(open) => !open && setDeleting(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t('deleteTitle')}</AlertDialogTitle>
            <AlertDialogDescription>{t('deleteBody')}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t('cancel')}</AlertDialogCancel>
            <AlertDialogAction onClick={() => void confirmDelete()}>
              {t('delete')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
