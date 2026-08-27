import type {
  ConversationRecordV2,
  ExecutionTarget,
  ProjectAttachment
} from '@shared/types/conversation.types'
import { Folder, FolderGit2, Link2, PanelsTopLeft, Unlink } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from '@/components/ui/select'
import { getCurrentConversation, useConversationStore } from '@/stores/conversation-store'
import type { Project } from '@/types/project'

export interface ExecutionTargetPickerProps {
  projects: readonly Project[]
  value: ExecutionTarget
  attachment: ProjectAttachment | null
  conversation?: ConversationRecordV2 | null
  workspaceCwd?: string | null
  onChange: (target: ExecutionTarget) => void
  onAttachmentChange: (attachment: ProjectAttachment | null) => void
  nowUtc?: () => string
}

function projectRoot(project: Project | undefined): string {
  return project?.path?.trim() ?? ''
}

function activeWorktree(project: Project | undefined) {
  if (!project?.activeWorktreeId) return undefined
  return project.worktrees?.find((worktree) => worktree.id === project.activeWorktreeId)
}

export type ExecutionTargetValidationError =
  | 'projectRequired'
  | 'projectRootRequired'
  | 'worktreeBranchRequired'
  | 'projectAttachmentRequired'

export function validateExecutionTarget(
  target: ExecutionTarget
): ExecutionTargetValidationError | null {
  if (target.kind === 'workspace') return null
  if (!target.projectId.trim()) return 'projectRequired'
  if (target.kind === 'project_root') {
    return target.projectRoot.trim() ? null : 'projectRootRequired'
  }
  if (!target.worktreeBranch.trim()) return 'worktreeBranchRequired'
  return null
}

export function ExecutionTargetPicker({
  projects,
  value,
  attachment,
  conversation,
  workspaceCwd,
  onChange,
  onAttachmentChange,
  nowUtc = () => new Date().toISOString()
}: ExecutionTargetPickerProps): React.JSX.Element {
  const { t } = useTranslation('conversation')
  const selectedProjectId = value.kind === 'workspace' ? (projects[0]?.id ?? '') : value.projectId
  const selectedProject = projects.find((project) => project.id === selectedProjectId)
  const attachProject = useConversationStore((state) => state.attachProject)
  const detachProject = useConversationStore((state) => state.detachProject)
  const updateExecutionTarget = useConversationStore((state) => state.updateExecutionTarget)
  const aggregateBusy = useConversationStore((state) =>
    conversation ? Boolean(state.aggregateBusyById[conversation.conversationId]) : false
  )
  const aggregateError = useConversationStore((state) =>
    conversation ? state.errorsById[conversation.conversationId] : undefined
  )
  const [localValidationError, setLocalValidationError] =
    useState<ExecutionTargetValidationError | null>(null)
  const [savedAction, setSavedAction] = useState<'target' | 'attached' | 'detached' | null>(null)
  const validationError = localValidationError ?? validateExecutionTarget(value)
  const readyConversation = conversation?.lifecycleState === 'ready' ? conversation : null
  const attachableProjects = useMemo(
    () => projects.filter((project) => Boolean(project.path?.trim())),
    [projects]
  )

  const commitTarget = async (target: ExecutionTarget): Promise<void> => {
    setLocalValidationError(null)
    setSavedAction(null)
    onChange(target)
    if (!readyConversation) return
    const currentConversation =
      getCurrentConversation(useConversationStore.getState(), readyConversation.conversationId) ??
      readyConversation
    if (target.kind !== 'workspace' && !currentConversation.projectAttachment) {
      setLocalValidationError('projectAttachmentRequired')
      onChange(currentConversation.executionTarget)
      return
    }
    const outcome = await updateExecutionTarget(currentConversation.conversationId, target)
    if (!outcome) {
      onChange(currentConversation.executionTarget)
      return
    }
    onChange(outcome.executionTarget)
    onAttachmentChange(outcome.projectAttachment)
    setSavedAction('target')
  }

  const selectKind = (kind: ExecutionTarget['kind']): void => {
    if (kind === 'workspace') {
      void commitTarget({ kind: 'workspace' })
      return
    }
    const project = selectedProject ?? attachableProjects[0]
    if (kind === 'project_root') {
      void commitTarget({
        kind,
        projectId: project?.id ?? '',
        projectRoot: projectRoot(project)
      })
      return
    }
    const selectedWorktree = activeWorktree(project)
    void commitTarget({
      kind,
      projectId: project?.id ?? '',
      worktreePath: selectedWorktree?.path ?? '',
      worktreeBranch: selectedWorktree?.branch ?? project?.gitBranch ?? ''
    })
  }

  const selectProject = (projectId: string): void => {
    const project = projects.find((candidate) => candidate.id === projectId)
    if (!project || value.kind === 'workspace') return
    if (value.kind === 'project_root') {
      void commitTarget({ kind: value.kind, projectId, projectRoot: projectRoot(project) })
      return
    }
    const selectedWorktree = activeWorktree(project)
    void commitTarget({
      kind: value.kind,
      projectId,
      worktreePath: selectedWorktree?.path ?? '',
      worktreeBranch: selectedWorktree?.branch ?? project.gitBranch ?? ''
    })
  }

  const toggleAttachment = async (): Promise<void> => {
    setLocalValidationError(null)
    setSavedAction(null)
    const currentConversation = readyConversation
      ? (getCurrentConversation(
          useConversationStore.getState(),
          readyConversation.conversationId
        ) ?? readyConversation)
      : null
    const currentAttachment = currentConversation?.projectAttachment ?? attachment
    const currentTarget = currentConversation?.executionTarget ?? value
    if (currentAttachment) {
      if (currentConversation && currentTarget.kind !== 'workspace') {
        setLocalValidationError('projectAttachmentRequired')
        return
      }
      onAttachmentChange(null)
      if (!currentConversation) return
      const outcome = await detachProject(currentConversation.conversationId)
      if (!outcome) {
        onAttachmentChange(currentAttachment)
        return
      }
      onAttachmentChange(outcome.projectAttachment)
      onChange(outcome.executionTarget)
      setSavedAction('detached')
      return
    }
    const project = selectedProject ?? attachableProjects[0]
    const root = projectRoot(project)
    if (!project || !root) return
    const selectedWorktree = value.kind === 'worktree' ? activeWorktree(project) : undefined
    const nextAttachment: ProjectAttachment = {
      schemaVersion: 1,
      projectId: project.id,
      attachedAtUtc: nowUtc(),
      projectPathSnapshot: root,
      worktreePath: selectedWorktree?.path ?? null,
      worktreeBranch: selectedWorktree?.branch ?? null
    }
    onAttachmentChange(nextAttachment)
    if (!currentConversation) return
    const outcome = await attachProject(currentConversation.conversationId, nextAttachment)
    if (!outcome) {
      onAttachmentChange(null)
      return
    }
    onAttachmentChange(outcome.projectAttachment)
    onChange(outcome.executionTarget)
    setSavedAction('attached')
  }

  return (
    <section
      className="rounded-xl border border-border/60 bg-card/60 p-3"
      aria-label={t('target.title')}
    >
      <div className="grid gap-3 sm:grid-cols-2">
        <div className="space-y-1.5">
          <Label htmlFor="execution-target-kind">{t('target.label')}</Label>
          <Select
            value={value.kind}
            disabled={aggregateBusy}
            onValueChange={(kind) => selectKind(kind as ExecutionTarget['kind'])}
          >
            <SelectTrigger id="execution-target-kind" aria-label={t('target.label')}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="workspace">
                <span className="flex items-center gap-2">
                  <PanelsTopLeft className="size-4" aria-hidden="true" />
                  {t('target.workspace')}
                </span>
              </SelectItem>
              <SelectItem
                value="project_root"
                disabled={
                  attachableProjects.length === 0 || Boolean(readyConversation && !attachment)
                }
              >
                <span className="flex items-center gap-2">
                  <Folder className="size-4" aria-hidden="true" />
                  {t('target.projectRoot')}
                </span>
              </SelectItem>
              <SelectItem
                value="worktree"
                disabled={
                  attachableProjects.length === 0 || Boolean(readyConversation && !attachment)
                }
              >
                <span className="flex items-center gap-2">
                  <FolderGit2 className="size-4" aria-hidden="true" />
                  {t('target.worktree')}
                </span>
              </SelectItem>
            </SelectContent>
          </Select>
        </div>

        <div className="space-y-1.5">
          <Label htmlFor="execution-target-project">{t('target.project')}</Label>
          <Select
            value={selectedProjectId}
            onValueChange={selectProject}
            disabled={
              aggregateBusy || value.kind === 'workspace' || attachableProjects.length === 0
            }
          >
            <SelectTrigger id="execution-target-project" aria-label={t('target.project')}>
              <SelectValue placeholder={t('target.noProject')} />
            </SelectTrigger>
            <SelectContent>
              {attachableProjects.map((project) => (
                <SelectItem key={project.id} value={project.id}>
                  {project.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </div>

      <div className="mt-3 flex flex-col gap-2 text-xs text-muted-foreground sm:flex-row sm:items-center sm:justify-between">
        <div className="min-w-0">
          <div data-testid="workspace-identity-unchanged" data-unchanged="true">
            {t('target.workspaceUnchanged')}
          </div>
          <code className="block truncate font-mono" title={workspaceCwd ?? undefined}>
            {workspaceCwd || t('target.newWorkspace')}
          </code>
          {conversation ? (
            <dl
              className="mt-2 grid gap-x-3 gap-y-1 sm:grid-cols-[auto_minmax(0,1fr)]"
              data-testid="conversation-identity-details"
            >
              <dt>{t('identity.conversationId')}</dt>
              <dd className="truncate font-mono" title={conversation.conversationId}>
                {conversation.conversationId}
              </dd>
              <dt>{t('identity.createdAtUtc')}</dt>
              <dd className="truncate font-mono" title={conversation.createdAtUtc}>
                {conversation.createdAtUtc}
              </dd>
              <dt>{t('identity.creationPartition')}</dt>
              <dd className="truncate font-mono" title={conversation.creationPartition.path}>
                {conversation.creationPartition.path}
              </dd>
              <dt>{t('identity.workspaceCwd')}</dt>
              <dd className="truncate font-mono" title={conversation.workspaceCwd}>
                {conversation.workspaceCwd}
              </dd>
            </dl>
          ) : null}
          {value.kind === 'project_root' ? (
            <code className="block truncate font-mono">
              {value.projectRoot || t('target.invalid')}
            </code>
          ) : null}
          {value.kind === 'worktree' ? (
            <code className="block truncate font-mono">
              {value.worktreePath || t('target.newWorktree')} ·{' '}
              {value.worktreeBranch || t('target.invalid')}
            </code>
          ) : null}
          {validationError ? (
            <p role="alert" className="mt-1 text-destructive">
              {t(`target.errors.${validationError}` as const)}
            </p>
          ) : null}
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="min-h-10 shrink-0 gap-2"
          disabled={
            aggregateBusy ||
            (!attachment && attachableProjects.length === 0) ||
            Boolean(attachment && readyConversation && value.kind !== 'workspace')
          }
          onClick={() => void toggleAttachment()}
        >
          {attachment ? <Unlink className="size-4" /> : <Link2 className="size-4" />}
          {attachment ? t('attachment.detach') : t('attachment.attach')}
        </Button>
      </div>

      <output className="mt-2 block text-xs text-muted-foreground" aria-live="polite">
        {aggregateBusy
          ? t('mutation.saving')
          : aggregateError
            ? t(`mutation.errors.${aggregateError.code}` as const, {
                defaultValue: aggregateError.message
              })
            : savedAction
              ? t(`mutation.saved.${savedAction}` as const)
              : attachment
                ? t('attachment.attached', { projectId: attachment.projectId })
                : t('attachment.none')}
      </output>
    </section>
  )
}
