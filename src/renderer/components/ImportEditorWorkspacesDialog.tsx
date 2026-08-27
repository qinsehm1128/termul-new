import type {
  EditorWorkspaceCandidate,
  EditorWorkspaceKind
} from '@shared/types/editor-workspace.types'
import { AnimatePresence, motion, useReducedMotion } from 'framer-motion'
import { FolderInput, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Checkbox } from '@/components/ui/checkbox'
import { toast } from '@/hooks/use-toast'
import { availableColors } from '@/lib/colors'
import { dialogApi } from '@/lib/dialog-api'
import { editorWorkspaceApi } from '@/lib/editor-workspace-api'
import { normalizeProjectPath } from '@/lib/editor-workspace-paths'
import { logFrontendError } from '@/lib/log-api'
import { isTauriContext } from '@/lib/tauri-runtime'
import { useProjectActions, useProjects } from '@/stores/project-store'
import type { ProjectColor } from '@/types/project'

interface ImportEditorWorkspacesDialogProps {
  isOpen: boolean
  onClose: () => void
  targetGroupId?: string | null
}

const EDITOR_LABEL_KEYS = {
  vscode: 'editorImport.editors.vscode',
  cursor: 'editorImport.editors.cursor',
  windsurf: 'editorImport.editors.windsurf',
  trae: 'editorImport.editors.trae',
  zed: 'editorImport.editors.zed'
} as const satisfies Record<EditorWorkspaceKind, string>

export function ImportEditorWorkspacesDialog({
  isOpen,
  onClose,
  targetGroupId
}: ImportEditorWorkspacesDialogProps) {
  const { t } = useTranslation('projects')
  const reducedMotion = useReducedMotion() ?? false
  const projects = useProjects()
  const { addProject, addGroup, moveProjectToGroup } = useProjectActions()
  const [candidates, setCandidates] = useState<EditorWorkspaceCandidate[]>([])
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set())
  const [workspacePath, setWorkspacePath] = useState('')
  const [loading, setLoading] = useState(false)
  const [importing, setImporting] = useState(false)

  const importedPaths = useMemo(() => {
    const paths = new Set<string>()
    for (const project of projects) {
      if (project.path) paths.add(normalizeProjectPath(project.path))
    }
    return paths
  }, [projects])

  const loadRecent = useCallback(async () => {
    setLoading(true)
    try {
      const result = await editorWorkspaceApi.list()
      if (!result.success) {
        throw new Error(result.error || t('editorImport.loadFailed'))
      }
      setCandidates(result.data.candidates)
      setSelectedIds(
        new Set(
          result.data.candidates
            .filter((candidate) => !importedPaths.has(normalizeProjectPath(candidate.path)))
            .map((candidate) => candidate.id)
        )
      )
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      void logFrontendError({
        level: 'warn',
        source: 'editor-import.list',
        message
      })
      toast({
        title: t('error'),
        description: t('editorImport.loadFailed'),
        variant: 'destructive'
      })
    } finally {
      setLoading(false)
    }
  }, [importedPaths, t])

  useEffect(() => {
    if (!isOpen) return
    void loadRecent()
  }, [isOpen, loadRecent])

  const grouped = useMemo(() => {
    const groups = new Map<EditorWorkspaceKind, EditorWorkspaceCandidate[]>()
    for (const candidate of candidates) {
      const list = groups.get(candidate.editor) ?? []
      list.push(candidate)
      groups.set(candidate.editor, list)
    }
    return groups
  }, [candidates])

  const mergeCandidates = useCallback(
    (incoming: EditorWorkspaceCandidate[]) => {
      setCandidates((current) => {
        const byId = new Map(current.map((item) => [item.id, item]))
        for (const item of incoming) byId.set(item.id, item)
        return [...byId.values()]
      })
      setSelectedIds((current) => {
        const next = new Set(current)
        for (const item of incoming) {
          if (!importedPaths.has(normalizeProjectPath(item.path))) {
            next.add(item.id)
          }
        }
        return next
      })
    },
    [importedPaths]
  )

  const handleParseWorkspace = useCallback(async () => {
    let path = workspacePath.trim()
    if (!path && isTauriContext()) {
      const picked = await dialogApi.selectFile({
        title: t('editorImport.selectWorkspaceFile'),
        filters: [{ name: 'VS Code Workspace', extensions: ['code-workspace'] }]
      })
      if (!picked.success || !picked.data) return
      path = picked.data
      setWorkspacePath(path)
    }
    if (!path) {
      toast({
        title: t('error'),
        description: t('editorImport.workspacePathRequired'),
        variant: 'destructive'
      })
      return
    }
    const result = await editorWorkspaceApi.parseFile(path)
    if (!result.success) {
      void logFrontendError({
        level: 'warn',
        source: 'editor-import.parse',
        message: result.error || 'parse failed'
      })
      toast({
        title: t('error'),
        description: result.error || t('editorImport.parseFailed'),
        variant: 'destructive'
      })
      return
    }
    if (result.data.candidates.length === 0) {
      toast({
        title: t('editorImport.emptyWorkspace'),
        description: t('editorImport.emptyWorkspaceHint')
      })
      return
    }
    mergeCandidates(result.data.candidates)
  }, [mergeCandidates, t, workspacePath])

  const toggle = useCallback((id: string, alreadyImported: boolean) => {
    if (alreadyImported) return
    setSelectedIds((current) => {
      const next = new Set(current)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }, [])

  const handleImport = useCallback(() => {
    const selected = candidates.filter((candidate) => selectedIds.has(candidate.id))
    const fresh = selected.filter(
      (candidate) => !importedPaths.has(normalizeProjectPath(candidate.path))
    )
    if (fresh.length === 0) {
      toast({
        title: t('editorImport.nothingToImport'),
        description: t('editorImport.nothingToImportHint')
      })
      return
    }

    setImporting(true)
    try {
      const fromWorkspaceFile = fresh.filter((candidate) => candidate.source === 'workspace-file')
      let groupId = targetGroupId ?? null
      if (!groupId && fromWorkspaceFile.length > 1) {
        const workspaceName =
          workspacePath
            .split(/[\\/]/)
            .pop()
            ?.replace(/\.code-workspace$/i, '') || t('editorImport.workspaceGroup')
        groupId = addGroup(workspaceName)
      }

      fresh.forEach((candidate, index) => {
        const color = availableColors[index % availableColors.length] as ProjectColor
        const project = addProject(candidate.name, color, candidate.path)
        if (groupId) {
          moveProjectToGroup(project.id, groupId)
        }
      })

      toast({
        title: t('editorImport.imported'),
        description: t('editorImport.importedCount', { count: fresh.length })
      })
      onClose()
    } finally {
      setImporting(false)
    }
  }, [
    addGroup,
    addProject,
    candidates,
    importedPaths,
    moveProjectToGroup,
    onClose,
    selectedIds,
    t,
    targetGroupId,
    workspacePath
  ])

  const selectedCount = candidates.filter(
    (candidate) =>
      selectedIds.has(candidate.id) && !importedPaths.has(normalizeProjectPath(candidate.path))
  ).length

  return (
    <AnimatePresence>
      {isOpen && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-[2px]"
          onClick={onClose}
        >
          <motion.div
            initial={{ opacity: 0, y: reducedMotion ? 0 : -6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: reducedMotion ? 0 : -6 }}
            transition={{ duration: reducedMotion ? 0 : 0.15 }}
            className="flex max-h-[90vh] w-[560px] flex-col overflow-hidden rounded-md border border-border/80 bg-card shadow-[0_18px_60px_hsl(var(--background)/0.7)]"
            onClick={(event) => event.stopPropagation()}
          >
            <div className="flex h-9 shrink-0 items-center justify-between border-b border-border/70 px-3">
              <h3 className="text-xs font-semibold tracking-[-0.01em] text-foreground">
                {t('editorImport.title')}
              </h3>
              <button
                type="button"
                onClick={onClose}
                className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
                aria-label={t('close')}
              >
                <X size={14} />
              </button>
            </div>

            <div className="flex-1 space-y-4 overflow-y-auto p-4">
              <div className="space-y-2">
                <p className="text-xs font-medium text-muted-foreground">
                  {t('editorImport.recent')}
                </p>
                {loading ? (
                  <p className="text-xs text-muted-foreground">{t('editorImport.loading')}</p>
                ) : candidates.length === 0 ? (
                  <p className="text-xs text-muted-foreground">{t('editorImport.emptyRecent')}</p>
                ) : (
                  <div className="space-y-3">
                    {[...grouped.entries()].map(([editor, items]) => (
                      <div key={editor} className="space-y-1.5">
                        <p className="text-3xs font-semibold uppercase tracking-wide text-muted-foreground">
                          {t(EDITOR_LABEL_KEYS[editor])}
                        </p>
                        {items.map((candidate) => {
                          const alreadyImported = importedPaths.has(
                            normalizeProjectPath(candidate.path)
                          )
                          return (
                            <div
                              key={candidate.id}
                              className="flex items-start gap-2 rounded-md border border-border/70 px-2.5 py-2 text-xs"
                            >
                              <Checkbox
                                checked={alreadyImported || selectedIds.has(candidate.id)}
                                disabled={alreadyImported}
                                onCheckedChange={() => toggle(candidate.id, alreadyImported)}
                                className="mt-0.5"
                                aria-label={candidate.name}
                              />
                              <span className="min-w-0 flex-1">
                                <span className="block truncate font-medium text-foreground">
                                  {candidate.name}
                                </span>
                                <span className="block truncate text-3xs text-muted-foreground">
                                  {candidate.path}
                                </span>
                                {alreadyImported ? (
                                  <span className="text-3xs text-muted-foreground">
                                    {t('editorImport.alreadyImported')}
                                  </span>
                                ) : null}
                              </span>
                            </div>
                          )
                        })}
                      </div>
                    ))}
                  </div>
                )}
              </div>

              <div className="space-y-2">
                <p className="text-xs font-medium text-muted-foreground">
                  {t('editorImport.workspaceFile')}
                </p>
                <div className="flex gap-2">
                  <input
                    type="text"
                    value={workspacePath}
                    onChange={(event) => setWorkspacePath(event.target.value)}
                    placeholder={t('editorImport.workspacePathPlaceholder')}
                    className="h-8 flex-1 rounded-md border border-input/80 bg-secondary/35 px-2.5 text-sm outline-none"
                    data-testid="editor-import-workspace-path"
                  />
                  <button
                    type="button"
                    onClick={() => void handleParseWorkspace()}
                    className="inline-flex h-8 items-center rounded-md border border-border/80 bg-secondary/50 px-3 text-xs"
                  >
                    {isTauriContext() ? t('browse') : t('editorImport.parse')}
                  </button>
                </div>
              </div>

              {!isTauriContext() && (
                <p className="rounded-md bg-secondary/35 px-3 py-2 text-xs leading-relaxed text-muted-foreground">
                  {t('webSessionNote')}
                </p>
              )}
            </div>

            <div className="flex h-10 shrink-0 items-center justify-end gap-2 border-t border-border/70 bg-secondary/20 px-4">
              <button
                type="button"
                onClick={onClose}
                className="inline-flex h-8 items-center rounded-md px-3 text-xs text-muted-foreground hover:bg-secondary/60"
              >
                {t('cancel')}
              </button>
              <button
                type="button"
                onClick={handleImport}
                disabled={importing || selectedCount === 0}
                className="inline-flex h-8 items-center gap-1.5 rounded-md bg-primary px-3 text-xs font-medium text-primary-foreground disabled:opacity-50"
                data-testid="editor-import-confirm"
              >
                <FolderInput size={13} />
                {t('editorImport.importSelected', { count: selectedCount })}
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
