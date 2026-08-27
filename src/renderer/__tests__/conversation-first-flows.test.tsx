import type {
  ConversationAggregateMutationOutcome,
  ConversationRecordV2,
  ExecutionTarget,
  ProjectAttachment
} from '@shared/types/conversation.types'
import type { RecoveryItemV1 } from '@shared/types/conversation-recovery.types'
import type { SessionWorkspaceV1 } from '@shared/types/session-workspace.types'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { useState } from 'react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ConversationRecoveryPanel } from '@/components/conversation/ConversationRecoveryPanel'
import { ExecutionTargetPicker } from '@/components/conversation/ExecutionTargetPicker'
import { PaneContent } from '@/components/workspace/PaneContent'
import { loadSessionWorkspace } from '@/hooks/use-session-workspace-sync'
import { useConversationStore } from '@/stores/conversation-store'
import { useProjectStore } from '@/stores/project-store'
import { useSessionWorkspaceSyncStore } from '@/stores/session-workspace-sync-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { useWorkspaceStore } from '@/stores/workspace-store'
import type { Project } from '@/types/project'

if (!HTMLElement.prototype.hasPointerCapture) {
  HTMLElement.prototype.hasPointerCapture = () => false
  HTMLElement.prototype.setPointerCapture = () => undefined
  HTMLElement.prototype.releasePointerCapture = () => undefined
}

if (!HTMLElement.prototype.scrollIntoView) {
  HTMLElement.prototype.scrollIntoView = () => undefined
}

const ID = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
const { mockConversationApi, mockTerminalApi, mockSessionWorkspaceApi } = vi.hoisted(() => ({
  mockConversationApi: {
    listConversations: vi.fn(),
    openConversation: vi.fn(),
    resolveRecovery: vi.fn(),
    attachProject: vi.fn(),
    detachProject: vi.fn(),
    updateExecutionTarget: vi.fn()
  },
  mockTerminalApi: {
    resume: vi.fn(),
    spawn: vi.fn(),
    closeView: vi.fn(),
    terminate: vi.fn()
  },
  mockSessionWorkspaceApi: {
    getWorkspace: vi.fn(),
    writeWorkspace: vi.fn(),
    resolveRecovery: vi.fn()
  }
}))

vi.mock('@/lib/conversation-api', () => ({ conversationApi: mockConversationApi }))
vi.mock('@/lib/terminal-api', () => ({ terminalApi: mockTerminalApi }))
vi.mock('@/lib/session-workspace-api', () => ({ sessionWorkspaceApi: mockSessionWorkspaceApi }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn() }))
vi.mock('@/components/terminal/ConnectedTerminal', () => ({
  ConnectedTerminal: ({ terminalId }: { terminalId?: string }) => (
    <div data-testid="connected-terminal">connected:{terminalId}</div>
  )
}))

const conversation: ConversationRecordV2 = {
  schemaVersion: 2,
  conversationId: ID,
  createdAtUtc: '2026-08-15T09:45:15.123Z',
  creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
  workspaceCwd: `/visible/sessions/2026/08/15/${ID}`,
  executionTarget: { kind: 'workspace' },
  projectAttachment: null,
  lifecycleState: 'ready',
  lastSeq: 0,
  createdBy: 'termul'
}

const project: Project = {
  id: 'project-1',
  name: 'Termul',
  color: 'blue',
  path: '/projects/termul',
  isGitRepo: true,
  gitBranch: 'main',
  activeWorktreeId: 'worktree-1',
  worktrees: [
    {
      id: 'worktree-1',
      name: 'Conversation repair',
      path: '/projects/termul-worktree',
      branch: 'feature/conversation-repair',
      createdAt: '2026-08-15T09:00:00.000Z'
    }
  ]
}

const projectAttachment: ProjectAttachment = {
  schemaVersion: 1,
  projectId: project.id,
  attachedAtUtc: '2026-08-15T10:00:00.000Z',
  projectPathSnapshot: project.path,
  worktreePath: null,
  worktreeBranch: null
}

function aggregateOutcome(
  current: ConversationRecordV2,
  action: ConversationAggregateMutationOutcome['action'],
  attachment: ProjectAttachment | null,
  executionTarget: ExecutionTarget
): ConversationAggregateMutationOutcome {
  const next = {
    ...current,
    projectAttachment: attachment,
    executionTarget,
    lastSeq: current.lastSeq + 1
  }
  const identity = {
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
    revision: next.lastSeq,
    identityBefore: identity,
    identityAfter: identity,
    projectAttachment: attachment,
    executionTarget,
    conversation: next
  }
}

function coldTerminalWorkspace(): SessionWorkspaceV1 {
  return {
    schemaVersion: 1,
    conversationId: ID,
    revision: 9,
    updatedAtUtc: '2026-08-15T10:00:00.000Z',
    topology: {
      type: 'leaf',
      id: 'cold-terminal-pane',
      terminalIds: ['cold-terminal-record'],
      editorIds: [],
      activeTabId: 'term-cold-terminal-record'
    },
    activePaneId: 'cold-terminal-pane',
    resources: [
      {
        kind: 'terminal',
        terminalId: 'pty-cold-live',
        terminalRecordId: 'cold-terminal-record',
        conversationId: ID
      }
    ],
    projectionState: { status: 'native' }
  }
}

const recoveryEvidence = {
  sourcePaths: ['legacy_workspace_manifests/0/shared.json'],
  sourceSha256: ['e'.repeat(64)],
  candidateFacts: [{ candidate: 'preserved' }],
  provenance: [
    {
      sourceKind: 'legacy_workspace_manifests',
      relativePath: 'legacy_workspace_manifests/0/shared.json',
      sha256: 'e'.repeat(64),
      preservedReadOnly: true as const
    }
  ]
}

const recoveryItem: RecoveryItemV1 = {
  recoveryId: 'a'.repeat(64),
  kind: 'ambiguous_workspace_manifest',
  severity: 'warning',
  sourcePaths: [],
  conversationIds: [ID],
  sourceSha256: [],
  candidateFacts: [],
  provenance: [],
  status: 'unresolved',
  suggestedActions: [
    'inspect',
    'associateConversation',
    'startEmptyWorkspace',
    'dismissPreservedSource'
  ],
  revision: 1,
  associationDecisions: []
}

function ConflictView(): React.JSX.Element {
  const conflict = useSessionWorkspaceSyncStore((state) => state.getConflict(ID))
  return conflict ? (
    <div role="alert">
      Workspace conflict at revision {conflict.currentRevision}; canonical Conversation remains {ID}
    </div>
  ) : (
    <div role="status">Workspace ready</div>
  )
}

function TargetHarness({ projects = [] }: { projects?: readonly Project[] }): React.JSX.Element {
  const current = useConversationStore((state) => state.summariesById[ID]) ?? conversation
  const [target, setTarget] = useState(current.executionTarget)
  const [attachment, setAttachment] = useState(current.projectAttachment)
  return (
    <ExecutionTargetPicker
      projects={projects}
      value={target}
      attachment={attachment}
      conversation={current}
      workspaceCwd={current.workspaceCwd}
      onChange={setTarget}
      onAttachmentChange={setAttachment}
      nowUtc={() => projectAttachment.attachedAtUtc}
    />
  )
}

function LifecycleStatus(): React.JSX.Element {
  const current = useConversationStore((state) => state.summariesById[ID])
  const active = useConversationStore((state) => state.activeConversationId)
  return (
    <output aria-label="Conversation lifecycle status">
      {current?.lifecycleState ?? 'deleted'}:{active ?? 'none'}
    </output>
  )
}

async function chooseTarget(option: string): Promise<void> {
  const trigger = screen.getByRole('combobox', { name: 'Execution target' })
  trigger.focus()
  fireEvent.keyDown(trigger, { key: 'ArrowDown' })
  fireEvent.click(await screen.findByRole('option', { name: option }))
}

describe('Conversation-first desktop/browser flow matrix', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    useConversationStore.getState().reset()
    useSessionWorkspaceSyncStore.setState({
      activeConversationId: null,
      basedRevisionByConversation: {},
      conflictsByConversation: {},
      recoveryByConversation: {},
      loadOutcomeByConversation: {},
      restoreInProgressByConversation: {}
    })
    useTerminalStore.setState({ terminals: [], activeTerminalId: '', ptyIdIndex: new Map() })
    useWorkspaceStore.getState().resetLayout()
    useProjectStore.setState({ activeProjectId: '' })
    mockTerminalApi.resume.mockResolvedValue({
      success: false,
      error: 'Unauthorized',
      code: 'UNAUTHORIZED'
    })
    mockTerminalApi.spawn.mockResolvedValue({
      success: false,
      error: 'not expected',
      code: 'SPAWN_FAILED'
    })
    mockTerminalApi.closeView.mockResolvedValue({ success: true, data: undefined })
    mockTerminalApi.terminate.mockResolvedValue({ success: true, data: undefined })
    mockSessionWorkspaceApi.getWorkspace.mockResolvedValue({
      success: true,
      data: { status: 'missing', conversationId: ID }
    })
  })

  it('supports zero-project New Chat target selection without changing canonical identity', () => {
    render(<TargetHarness />)
    expect(screen.getByLabelText('Execution target')).toHaveTextContent('Conversation workspace')
    expect(screen.getByTestId('workspace-identity-unchanged')).toHaveAttribute(
      'data-unchanged',
      'true'
    )
    expect(screen.getByText(ID)).toBeVisible()
    expect(screen.getByText('2026/08/15')).toBeVisible()
    expect(screen.getAllByText(conversation.workspaceCwd).length).toBeGreaterThan(0)
    expect(screen.getByRole('button', { name: 'Attach project context' })).toBeDisabled()
  })

  it('attaches, retargets, returns to workspace, and detaches without changing identity', async () => {
    useConversationStore.getState().replaceSummaries([conversation])
    const attached = aggregateOutcome(
      conversation,
      'attachProject',
      projectAttachment,
      conversation.executionTarget
    )
    const projectTarget: ExecutionTarget = {
      kind: 'project_root',
      projectId: project.id,
      projectRoot: project.path
    }
    const retargeted = aggregateOutcome(
      attached.conversation,
      'updateExecutionTarget',
      projectAttachment,
      projectTarget
    )
    const workspace = aggregateOutcome(
      retargeted.conversation,
      'updateExecutionTarget',
      projectAttachment,
      { kind: 'workspace' }
    )
    const detached = aggregateOutcome(workspace.conversation, 'detachProject', null, {
      kind: 'workspace'
    })
    mockConversationApi.attachProject.mockResolvedValue({ success: true, data: attached })
    mockConversationApi.updateExecutionTarget
      .mockResolvedValueOnce({ success: true, data: retargeted })
      .mockResolvedValueOnce({ success: true, data: workspace })
    mockConversationApi.detachProject.mockResolvedValue({ success: true, data: detached })

    render(<TargetHarness projects={[project]} />)
    fireEvent.click(screen.getByRole('button', { name: 'Attach project context' }))
    expect(await screen.findByText(/Project context attached/)).toBeVisible()

    await chooseTarget('Project root')
    expect(await screen.findByText(/Execution target updated/)).toBeVisible()
    await chooseTarget('Conversation workspace')
    await waitFor(() => expect(mockConversationApi.updateExecutionTarget).toHaveBeenCalledTimes(2))
    fireEvent.click(screen.getByRole('button', { name: 'Detach project context' }))
    expect(await screen.findByText(/Project context detached/)).toBeVisible()

    const finalRecord = useConversationStore.getState().summariesById[ID]
    expect(finalRecord).toEqual(detached.conversation)
    expect(finalRecord.conversationId).toBe(conversation.conversationId)
    expect(finalRecord.createdAtUtc).toBe(conversation.createdAtUtc)
    expect(finalRecord.creationPartition).toEqual(conversation.creationPartition)
    expect(finalRecord.workspaceCwd).toBe(conversation.workspaceCwd)
  })

  it('opens and reopens the canonical Conversation route through the activation authority', async () => {
    mockConversationApi.openConversation.mockResolvedValue({
      success: true,
      data: { conversation, workspace: { status: 'missing', conversationId: ID } }
    })
    const firstEpoch = useConversationStore.getState().beginConversationActivation(ID)
    await expect(
      useConversationStore.getState().activateConversation(ID, firstEpoch)
    ).resolves.toBe(true)
    expect(useConversationStore.getState().detailsById[ID]?.conversation.conversationId).toBe(ID)
    expect(useConversationStore.getState().activeConversationId).toBe(ID)

    useConversationStore.getState().setActiveConversationId(null)
    const reopenEpoch = useConversationStore.getState().beginConversationActivation(ID)
    await expect(
      useConversationStore.getState().activateConversation(ID, reopenEpoch)
    ).resolves.toBe(true)
    expect(useConversationStore.getState().activeConversationId).toBe(ID)
    expect(mockConversationApi.openConversation).toHaveBeenCalledTimes(2)
  })

  it('cold-loads a persisted terminal tab and replay without spawning a replacement', async () => {
    const persisted = coldTerminalWorkspace()
    useConversationStore.getState().replaceSummaries([conversation])
    useConversationStore.getState().setActiveConversationId(ID)
    mockSessionWorkspaceApi.getWorkspace.mockResolvedValue({
      success: true,
      data: { status: 'loaded', workspace: persisted }
    })
    mockTerminalApi.resume.mockImplementation(async (request) => {
      const hydrated = useTerminalStore.getState().findTerminalByPtyId(request.terminalId)
      expect(hydrated).toMatchObject({
        id: 'cold-terminal-record',
        healthStatus: 'disconnected',
        conversationId: ID
      })
      useTerminalStore.getState().appendTranscript(request.terminalId, 'replayed cold output')
      return {
        success: true,
        data: {
          terminal: {
            id: request.terminalId,
            shell: 'bash',
            cwd: conversation.workspaceCwd,
            pid: 73,
            cols: 100,
            rows: 30,
            latestSeq: 21,
            gap: false
          },
          claim: 'renderer-memory-only'
        }
      }
    })

    await expect(loadSessionWorkspace(ID)).resolves.toBe(true)

    const root = useWorkspaceStore.getState().root
    if (root.type !== 'leaf') throw new Error('expected restored leaf')
    render(
      <MemoryRouter>
        <PaneContent pane={root} />
      </MemoryRouter>
    )
    expect(await screen.findByTestId('connected-terminal')).toHaveTextContent(
      'connected:pty-cold-live'
    )
    expect(root.tabs).toEqual([
      {
        type: 'terminal',
        id: 'term-cold-terminal-record',
        terminalId: 'cold-terminal-record'
      }
    ])
    expect(useTerminalStore.getState().peekTranscript('pty-cold-live')).toBe('replayed cold output')
    expect(useTerminalStore.getState().terminals[0]).toMatchObject({
      id: 'cold-terminal-record',
      ptyId: 'pty-cold-live',
      healthStatus: 'running',
      resumeCursor: 21,
      claim: 'renderer-memory-only'
    })
    expect(mockTerminalApi.resume).toHaveBeenCalledWith({
      conversationId: ID,
      terminalId: 'pty-cold-live',
      lastSeq: 0
    })
    expect(mockTerminalApi.spawn).not.toHaveBeenCalled()
    expect(mockTerminalApi.terminate).not.toHaveBeenCalled()
    expect(JSON.stringify(persisted)).not.toMatch(/claim|token|terminalOutput/i)
  })

  it('surfaces independent workspace conflicts without replacing Conversation identity', () => {
    render(<ConflictView />)
    act(() => {
      useSessionWorkspaceSyncStore.getState().setActiveConversationId(ID)
      useSessionWorkspaceSyncStore.getState().setConflict(ID, {
        conversationId: ID,
        currentRevision: 7,
        currentUpdatedAtUtc: '2026-08-15T10:00:00.000Z',
        currentUpdateIdentity: 'browser-b'
      })
    })
    expect(screen.getByRole('alert')).toHaveTextContent('revision 7')
    expect(screen.getByRole('alert')).toHaveTextContent(ID)
    expect(useSessionWorkspaceSyncStore.getState().activeConversationId).toBe(ID)
  })

  it('reveals authenticated recovery evidence from redacted browser status before mutation choices', async () => {
    const originalSnapshot = structuredClone(recoveryItem)
    mockConversationApi.resolveRecovery.mockImplementation(async (request) => ({
      success: true,
      data: {
        recoveryId: request.recoveryId,
        action: request.action,
        authorization: request.action === 'inspect' ? 'read' : 'mutation',
        status: request.action === 'inspect' ? 'unresolved' : 'resolvedAssociated',
        recoveryRevision: request.action === 'inspect' ? 1 : 2,
        workspaceRevision: null,
        workspaceChanged: false,
        ...recoveryEvidence
      }
    }))
    render(<ConversationRecoveryPanel items={[recoveryItem]} conversationId={ID} embedded />)

    expect(screen.getAllByRole('complementary', { name: 'Conversation recovery' })).toHaveLength(1)
    expect(document.querySelectorAll('[data-conversation-recovery-panel]')).toHaveLength(1)
    for (const action of recoveryItem.suggestedActions) {
      expect(document.querySelectorAll(`[data-recovery-action="${action}"]`)).toHaveLength(1)
    }
    expect(screen.queryByText(/legacy_workspace_manifests\/0\/shared.json/)).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Inspect preserved source' }))
    await waitFor(() => expect(mockConversationApi.resolveRecovery).toHaveBeenCalledTimes(1))
    expect(mockConversationApi.resolveRecovery.mock.calls[0][0]).toEqual({
      recoveryId: recoveryItem.recoveryId,
      expectedRevision: 1,
      action: 'inspect',
      payload: {}
    })
    expect(
      (await screen.findAllByText(/legacy_workspace_manifests\/0\/shared.json/)).length
    ).toBeGreaterThan(0)
    expect(screen.getAllByText(new RegExp(`sha256:${'e'.repeat(64)}`)).length).toBeGreaterThan(0)
    expect(screen.getByText('{"candidate":"preserved"}')).toBeVisible()
    expect(recoveryItem).toEqual(originalSnapshot)

    mockConversationApi.resolveRecovery.mockClear()
    fireEvent.click(screen.getByRole('button', { name: 'Associate conversation' }))
    await waitFor(() => expect(mockConversationApi.resolveRecovery).toHaveBeenCalledTimes(1))
    expect(mockConversationApi.resolveRecovery.mock.calls[0][0]).toEqual({
      recoveryId: recoveryItem.recoveryId,
      expectedRevision: 1,
      action: 'associateConversation',
      idempotencyKey: expect.stringMatching(
        /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/
      ),
      payload: { conversationId: ID }
    })
    expect(document.querySelectorAll('[data-conversation-recovery-panel]')).toHaveLength(1)
  })

  it.each([
    'FORBIDDEN',
    'UNAUTHORIZED',
    'CONVERSATION_RECOVERY_REQUIRED'
  ])('shows stable adapter error %s through an accessible alert', async (code) => {
    mockConversationApi.resolveRecovery.mockResolvedValue({
      success: false,
      code,
      error: code
    })
    render(<ConversationRecoveryPanel items={[recoveryItem]} conversationId={ID} embedded />)
    fireEvent.click(screen.getByRole('button', { name: 'Inspect preserved source' }))
    const alert = await screen.findByRole('alert')
    expect(alert).toHaveAttribute('data-error-code', code)
  })

  it('commits lifecycle outcomes in Conversation authority without terminating live PTYs', () => {
    const terminal = useTerminalStore
      .getState()
      .addTerminal('Lifecycle shell', '', 'bash', conversation.workspaceCwd, [], ID)
    useTerminalStore.getState().setTerminalPtyId(terminal.id, 'pty-lifecycle-live')
    useConversationStore.getState().replaceSummaries([conversation])
    useConversationStore.getState().setActiveConversationId(ID)
    render(<LifecycleStatus />)

    act(() => {
      useConversationStore.getState().applyLifecycleOutcome({
        status: 'updated',
        action: 'suspendBinding',
        conversationId: ID,
        previousRevision: 0,
        revision: 1,
        workspaceCwd: conversation.workspaceCwd,
        lifecycleState: 'ready',
        currentBinding: null
      })
    })
    expect(screen.getByLabelText('Conversation lifecycle status')).toHaveTextContent(`ready:${ID}`)
    expect(mockTerminalApi.terminate).not.toHaveBeenCalled()
    expect(useTerminalStore.getState().terminals[0].ptyId).toBe('pty-lifecycle-live')

    act(() => {
      useConversationStore.getState().applyLifecycleOutcome({
        status: 'updated',
        action: 'deleteConversation',
        conversationId: ID,
        previousRevision: 1,
        revision: 2,
        workspaceCwd: conversation.workspaceCwd,
        lifecycleState: 'deleted',
        currentBinding: null
      })
    })
    expect(screen.getByLabelText('Conversation lifecycle status')).toHaveTextContent('deleted:none')
    expect(mockTerminalApi.terminate).not.toHaveBeenCalled()
    expect(useTerminalStore.getState().terminals[0].ptyId).toBe('pty-lifecycle-live')
  })

  it('keeps Chat state intact while terminal close-view/reopen/terminate stay distinct', async () => {
    const terminal = useTerminalStore
      .getState()
      .addTerminal('Conversation shell', '', 'bash', conversation.workspaceCwd, [], ID)
    useTerminalStore.getState().setTerminalPtyId(terminal.id, 'pty-live')
    useConversationStore.getState().replaceSummaries([conversation])
    useConversationStore.getState().setActiveConversationId(ID)

    await expect(useTerminalStore.getState().closeTerminalView(terminal.id)).resolves.toBe(true)
    expect(mockTerminalApi.closeView).toHaveBeenCalledWith('pty-live')
    expect(mockTerminalApi.terminate).not.toHaveBeenCalled()
    expect(useTerminalStore.getState().terminals[0].viewState).toBe('hidden')
    expect(useConversationStore.getState().summariesById[ID]).toEqual(conversation)

    useTerminalStore.getState().reopenTerminalView(terminal.id)
    expect(useTerminalStore.getState().terminals[0].viewState).toBe('visible')
    await expect(useTerminalStore.getState().terminateTerminalResource(terminal.id)).resolves.toBe(
      true
    )
    expect(mockTerminalApi.terminate).toHaveBeenCalledWith('pty-live')
    expect(useConversationStore.getState().summariesById[ID]).toEqual(conversation)
  })
})
