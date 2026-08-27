import type {
  ConversationAggregateMutationAction,
  ConversationAggregateMutationOutcome,
  ConversationRecordV2,
  ExecutionTarget,
  ProjectAttachment
} from '@shared/types/conversation.types'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { useState } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { conversationApi } from '@/lib/conversation-api'
import { useConversationStore } from '@/stores/conversation-store'
import type { Project } from '@/types/project'
import { ExecutionTargetPicker, validateExecutionTarget } from './ExecutionTargetPicker'

vi.mock('@/lib/conversation-api', () => ({
  conversationApi: {
    listConversations: vi.fn(),
    openConversation: vi.fn(),
    attachProject: vi.fn(),
    detachProject: vi.fn(),
    updateExecutionTarget: vi.fn()
  }
}))

vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn() }))

if (!HTMLElement.prototype.hasPointerCapture) {
  HTMLElement.prototype.hasPointerCapture = () => false
  HTMLElement.prototype.setPointerCapture = () => undefined
  HTMLElement.prototype.releasePointerCapture = () => undefined
}

if (!HTMLElement.prototype.scrollIntoView) {
  HTMLElement.prototype.scrollIntoView = () => undefined
}

const identity: ConversationRecordV2 = {
  schemaVersion: 2,
  conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
  createdAtUtc: '2026-08-15T09:45:15.123Z',
  creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
  workspaceCwd: '/visible/sessions/2026/08/15/conversation',
  executionTarget: { kind: 'workspace' },
  projectAttachment: null,
  lifecycleState: 'initializing_agent',
  lastSeq: 4,
  createdBy: 'termul'
}

const readyConversation: ConversationRecordV2 = {
  ...identity,
  lifecycleState: 'ready'
}

const attachment: ProjectAttachment = {
  schemaVersion: 1,
  projectId: 'p1',
  attachedAtUtc: '2026-08-15T10:00:00.000Z',
  projectPathSnapshot: '/projects/termul',
  worktreePath: null,
  worktreeBranch: null
}

const projects: Project[] = [
  {
    id: 'p1',
    name: 'Termul',
    color: 'blue',
    path: '/projects/termul',
    isGitRepo: true,
    gitBranch: 'main',
    activeWorktreeId: 'w1',
    worktrees: [
      {
        id: 'w1',
        name: 'feature',
        path: '/projects/termul-worktree',
        branch: 'feature/conversations',
        createdAt: '2026-08-15T09:00:00.000Z'
      }
    ]
  }
]

function aggregateOutcome(
  current: ConversationRecordV2,
  action: ConversationAggregateMutationAction,
  projectAttachment: ProjectAttachment | null,
  executionTarget: ExecutionTarget
): ConversationAggregateMutationOutcome {
  const conversation = {
    ...current,
    projectAttachment,
    executionTarget,
    lastSeq: current.lastSeq + 1
  }
  const immutableIdentity = {
    conversationId: current.conversationId,
    createdAtUtc: current.createdAtUtc,
    creationPartition: current.creationPartition,
    workspaceCwd: current.workspaceCwd
  }
  return {
    status: 'updated',
    action,
    conversationId: current.conversationId,
    previousRevision: current.lastSeq,
    revision: conversation.lastSeq,
    identityBefore: immutableIdentity,
    identityAfter: immutableIdentity,
    projectAttachment,
    executionTarget,
    conversation
  }
}

function Harness({ ready = false }: { ready?: boolean }): React.JSX.Element {
  const conversation = ready ? readyConversation : identity
  const [target, setTarget] = useState<ExecutionTarget>(conversation.executionTarget)
  const [projectAttachment, setProjectAttachment] = useState<ProjectAttachment | null>(
    conversation.projectAttachment
  )
  return (
    <ExecutionTargetPicker
      projects={projects}
      value={target}
      attachment={projectAttachment}
      conversation={conversation}
      workspaceCwd={conversation.workspaceCwd}
      onChange={setTarget}
      onAttachmentChange={setProjectAttachment}
      nowUtc={() => '2026-08-15T10:00:00.000Z'}
    />
  )
}

async function choose(label: string, option: string): Promise<void> {
  const trigger = screen.getByRole('combobox', { name: label })
  trigger.focus()
  fireEvent.keyDown(trigger, { key: 'ArrowDown' })
  fireEvent.click(await screen.findByRole('option', { name: option }))
}

beforeEach(() => {
  vi.clearAllMocks()
  useConversationStore.getState().reset()
})

describe('ExecutionTargetPicker', () => {
  it('defaults to the independent workspace and marks identity as unchanged', () => {
    render(<Harness />)

    expect(screen.getByRole('combobox', { name: 'Execution target' })).toHaveTextContent(
      'Conversation workspace'
    )
    expect(screen.getByTestId('workspace-identity-unchanged')).toHaveAttribute(
      'data-unchanged',
      'true'
    )
    expect(screen.getAllByText(identity.workspaceCwd).length).toBeGreaterThan(0)
    expect(screen.getByText(identity.conversationId)).toBeInTheDocument()
    expect(screen.getByText(identity.createdAtUtc)).toBeInTheDocument()
    expect(screen.getByText(identity.creationPartition.path)).toBeInTheDocument()
    expect(screen.getByText('No project attachment')).toBeInTheDocument()
  })

  it('selects an explicit project root without changing the visible workspace cwd', async () => {
    render(<Harness />)

    await choose('Execution target', 'Project root')

    expect(screen.getByText('/projects/termul')).toBeInTheDocument()
    expect(screen.getAllByText(identity.workspaceCwd).length).toBeGreaterThan(0)
    expect(screen.getByText(identity.conversationId)).toBeInTheDocument()
    expect(screen.getByText(identity.createdAtUtc)).toBeInTheDocument()
    expect(screen.getByText(identity.creationPartition.path)).toBeInTheDocument()
    expect(screen.getByTestId('workspace-identity-unchanged')).toHaveAttribute(
      'data-unchanged',
      'true'
    )
  })

  it('selects an explicit existing worktree and keeps attachment separate', async () => {
    render(<Harness />)

    await choose('Execution target', 'Worktree')
    expect(
      screen.getByText(/\/projects\/termul-worktree · feature\/conversations/)
    ).toBeInTheDocument()
    expect(screen.getByText('No project attachment')).toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Attach project context' }))
    expect(screen.getByText('Attached project: p1')).toBeInTheDocument()
    expect(screen.getByText(identity.conversationId)).toBeInTheDocument()
    expect(screen.getByText(identity.createdAtUtc)).toBeInTheDocument()
    expect(screen.getByText(identity.creationPartition.path)).toBeInTheDocument()
    expect(screen.getAllByText(identity.workspaceCwd).length).toBeGreaterThan(0)
    fireEvent.click(screen.getByRole('button', { name: 'Detach project context' }))
    expect(screen.getByText('No project attachment')).toBeInTheDocument()
    expect(screen.getByText(identity.conversationId)).toBeInTheDocument()
  })

  it('persists ready attach, retarget, workspace, and detach with unchanged identity', async () => {
    useConversationStore.getState().replaceSummaries([readyConversation])
    const attachedOutcome = aggregateOutcome(
      readyConversation,
      'attachProject',
      attachment,
      readyConversation.executionTarget
    )
    const projectTarget: ExecutionTarget = {
      kind: 'project_root',
      projectId: 'p1',
      projectRoot: '/projects/termul'
    }
    const targetOutcome = aggregateOutcome(
      attachedOutcome.conversation,
      'updateExecutionTarget',
      attachment,
      projectTarget
    )
    const workspaceOutcome = aggregateOutcome(
      targetOutcome.conversation,
      'updateExecutionTarget',
      attachment,
      { kind: 'workspace' }
    )
    const detachedOutcome = aggregateOutcome(workspaceOutcome.conversation, 'detachProject', null, {
      kind: 'workspace'
    })
    vi.mocked(conversationApi.attachProject).mockResolvedValue({
      success: true,
      data: attachedOutcome
    })
    vi.mocked(conversationApi.updateExecutionTarget)
      .mockResolvedValueOnce({ success: true, data: targetOutcome })
      .mockResolvedValueOnce({ success: true, data: workspaceOutcome })
    vi.mocked(conversationApi.detachProject).mockResolvedValue({
      success: true,
      data: detachedOutcome
    })

    render(<Harness ready />)
    fireEvent.click(screen.getByRole('button', { name: 'Attach project context' }))
    expect(await screen.findByText(/Project context attached/)).toBeInTheDocument()

    await choose('Execution target', 'Project root')
    expect(await screen.findByText(/Execution target updated/)).toBeInTheDocument()
    expect(screen.getByText('/projects/termul')).toBeInTheDocument()

    await choose('Execution target', 'Conversation workspace')
    await waitFor(() => expect(conversationApi.updateExecutionTarget).toHaveBeenCalledTimes(2))
    fireEvent.click(screen.getByRole('button', { name: 'Detach project context' }))
    expect(await screen.findByText(/Project context detached/)).toBeInTheDocument()

    expect(conversationApi.attachProject).toHaveBeenCalledWith(
      readyConversation.conversationId,
      4,
      attachment
    )
    expect(conversationApi.updateExecutionTarget).toHaveBeenNthCalledWith(
      1,
      readyConversation.conversationId,
      5,
      projectTarget
    )
    expect(conversationApi.updateExecutionTarget).toHaveBeenNthCalledWith(
      2,
      readyConversation.conversationId,
      6,
      { kind: 'workspace' }
    )
    expect(conversationApi.detachProject).toHaveBeenCalledWith(readyConversation.conversationId, 7)
    expect(screen.getByText(identity.conversationId)).toBeInTheDocument()
    expect(screen.getByText(identity.createdAtUtc)).toBeInTheDocument()
    expect(screen.getByText(identity.creationPartition.path)).toBeInTheDocument()
    expect(screen.getAllByText(identity.workspaceCwd).length).toBeGreaterThan(0)
  })

  it('surfaces a stable stale-revision conflict and restores the attachment projection', async () => {
    useConversationStore.getState().replaceSummaries([readyConversation])
    vi.mocked(conversationApi.attachProject).mockResolvedValue({
      success: false,
      code: 'CONVERSATION_CONFLICT',
      error: 'stale revision'
    })

    render(<Harness ready />)
    fireEvent.click(screen.getByRole('button', { name: 'Attach project context' }))

    expect(
      await screen.findByText('This Conversation changed elsewhere. Reopen it and try again.')
    ).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Attach project context' })).toBeInTheDocument()
    expect(screen.getAllByText(identity.workspaceCwd).length).toBeGreaterThan(0)
  })

  it('validates project and worktree selections explicitly', () => {
    expect(validateExecutionTarget({ kind: 'workspace' })).toBeNull()
    expect(
      validateExecutionTarget({ kind: 'project_root', projectId: 'p1', projectRoot: '' })
    ).toBe('projectRootRequired')
    expect(
      validateExecutionTarget({
        kind: 'worktree',
        projectId: 'p1',
        worktreePath: '',
        worktreeBranch: ''
      })
    ).toBe('worktreeBranchRequired')
  })
})
