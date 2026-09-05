import type {
  ConversationAggregateMutationOutcome,
  ConversationRecordV2,
  ExecutionTarget,
  ProjectAttachment
} from '@shared/types/conversation.types'
import type { RecoveryItemV1 } from '@shared/types/conversation-recovery.types'
import type { SessionWorkspaceV1 } from '@shared/types/session-workspace.types'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
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
    <div data-testid="mobile-connected-terminal">connected:{terminalId}</div>
  )
}))

const mobileConversation: ConversationRecordV2 = {
  schemaVersion: 2,
  conversationId: ID,
  createdAtUtc: '2026-08-15T09:45:15.123Z',
  creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
  workspaceCwd: `/visible/sessions/2026/08/15/${ID}`,
  executionTarget: { kind: 'workspace' },
  projectAttachment: null,
  lifecycleState: 'ready',
  lastSeq: 0,
  createdBy: 'se-manager'
}

const mobileProject: Project = {
  id: 'phone-project',
  name: 'Phone project',
  color: 'green',
  path: '/projects/phone',
  isGitRepo: true,
  gitBranch: 'main',
  worktrees: []
}

const mobileAttachment: ProjectAttachment = {
  schemaVersion: 1,
  projectId: mobileProject.id,
  attachedAtUtc: '2026-08-15T10:00:00.000Z',
  projectPathSnapshot: mobileProject.path,
  worktreePath: null,
  worktreeBranch: null
}

function mobileAggregateOutcome(
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

const recoveryEvidence = {
  sourcePaths: ['legacy_workspace_manifests/0/phone.json'],
  sourceSha256: ['e'.repeat(64)],
  candidateFacts: [{ candidate: 'phone-preserved' }],
  provenance: [
    {
      sourceKind: 'legacy_workspace_manifests',
      relativePath: 'legacy_workspace_manifests/0/phone.json',
      sha256: 'e'.repeat(64),
      preservedReadOnly: true as const
    }
  ]
}

const recoveryItem: RecoveryItemV1 = {
  recoveryId: 'b'.repeat(64),
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
  revision: 3,
  associationDecisions: []
}

function mobileTerminalWorkspace(): SessionWorkspaceV1 {
  return {
    schemaVersion: 1,
    conversationId: ID,
    revision: 6,
    updatedAtUtc: '2026-08-15T10:00:00.000Z',
    topology: {
      type: 'leaf',
      id: 'phone-terminal-pane',
      terminalIds: ['phone-terminal-record'],
      editorIds: [],
      activeTabId: 'term-phone-terminal-record'
    },
    activePaneId: 'phone-terminal-pane',
    resources: [
      {
        kind: 'terminal',
        terminalId: 'pty-phone-cold',
        terminalRecordId: 'phone-terminal-record',
        conversationId: ID
      }
    ],
    projectionState: { status: 'native' }
  }
}

function PhoneHarness(): React.JSX.Element {
  const [drawerOpen, setDrawerOpen] = useState(false)
  const [target, setTarget] = useState({ kind: 'workspace' as const })
  const terminal = useTerminalStore((state) => state.terminals[0])
  const closeView = useTerminalStore((state) => state.closeTerminalView)
  const reopen = useTerminalStore((state) => state.reopenTerminalView)
  const terminate = useTerminalStore((state) => state.terminateTerminalResource)

  return (
    <main data-testid="phone-flow" style={{ width: 390 }}>
      <button
        type="button"
        aria-label="Open conversation drawer"
        onClick={() => setDrawerOpen(true)}
      >
        Conversations
      </button>
      {drawerOpen ? (
        <aside aria-label="Conversation drawer">
          <button type="button" aria-label="New chat">
            New chat
          </button>
          <button
            type="button"
            aria-label="Close conversation drawer"
            onClick={() => setDrawerOpen(false)}
          >
            Close
          </button>
        </aside>
      ) : null}
      <ExecutionTargetPicker
        projects={[]}
        value={target}
        attachment={null}
        workspaceCwd={`/visible/sessions/2026/08/15/${ID}`}
        onChange={setTarget}
        onAttachmentChange={() => undefined}
      />
      {terminal ? (
        <section aria-label="Terminal lifecycle">
          <output>{terminal.viewState ?? 'visible'}</output>
          <button type="button" onClick={() => void closeView(terminal.id)}>
            Close terminal view
          </button>
          <button type="button" onClick={() => reopen(terminal.id)}>
            Reopen terminal
          </button>
          <button type="button" onClick={() => void terminate(terminal.id)}>
            Terminate terminal
          </button>
        </section>
      ) : null}
    </main>
  )
}

function MobileReadyTargetHarness(): React.JSX.Element {
  const current = useConversationStore((state) => state.summariesById[ID]) ?? mobileConversation
  const [target, setTarget] = useState(current.executionTarget)
  const [attachment, setAttachment] = useState(current.projectAttachment)
  return (
    <main style={{ width: 390 }}>
      <ExecutionTargetPicker
        projects={[mobileProject]}
        value={target}
        attachment={attachment}
        conversation={current}
        workspaceCwd={current.workspaceCwd}
        onChange={setTarget}
        onAttachmentChange={setAttachment}
        nowUtc={() => mobileAttachment.attachedAtUtc}
      />
    </main>
  )
}

async function chooseMobileTarget(option: string): Promise<void> {
  const trigger = screen.getByRole('combobox', { name: 'Additional reachable directories' })
  trigger.focus()
  fireEvent.keyDown(trigger, { key: 'ArrowDown' })
  fireEvent.click(await screen.findByRole('option', { name: option }))
}

describe('Conversation-first responsive phone matrix', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: 390 })
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
    const terminal = useTerminalStore
      .getState()
      .addTerminal('Phone shell', '', 'bash', `/visible/sessions/2026/08/15/${ID}`, [], ID)
    useTerminalStore.getState().setTerminalPtyId(terminal.id, 'pty-phone')
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
    mockConversationApi.openConversation.mockResolvedValue({
      success: true,
      data: {
        conversation: {
          schemaVersion: 2,
          conversationId: ID,
          createdAtUtc: '2026-08-15T09:45:15.123Z',
          creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
          workspaceCwd: `/visible/sessions/2026/08/15/${ID}`,
          executionTarget: { kind: 'workspace' },
          projectAttachment: null,
          lifecycleState: 'ready',
          lastSeq: 0,
          createdBy: 'se-manager'
        },
        workspace: { status: 'missing', conversationId: ID }
      }
    })
  })

  it('opens the mobile drawer and keeps New Chat enabled without a project', () => {
    render(<PhoneHarness />)
    expect(screen.queryByLabelText('Conversation drawer')).not.toBeInTheDocument()
    fireEvent.click(screen.getByLabelText('Open conversation drawer'))
    expect(screen.getByLabelText('Conversation drawer')).toBeVisible()
    expect(screen.getByLabelText('New chat')).toBeEnabled()
    expect(screen.getByLabelText('Additional reachable directories')).toHaveTextContent(
      'Conversation directory only'
    )
  })

  it('persists ready attach, retarget, workspace, and detach at phone width', async () => {
    useConversationStore.getState().replaceSummaries([mobileConversation])
    const attached = mobileAggregateOutcome(
      mobileConversation,
      'attachProject',
      mobileAttachment,
      mobileConversation.executionTarget
    )
    const projectTarget: ExecutionTarget = {
      kind: 'project_root',
      projectId: mobileProject.id,
      projectRoot: mobileProject.path
    }
    const retargeted = mobileAggregateOutcome(
      attached.conversation,
      'updateExecutionTarget',
      mobileAttachment,
      projectTarget
    )
    const workspace = mobileAggregateOutcome(
      retargeted.conversation,
      'updateExecutionTarget',
      mobileAttachment,
      { kind: 'workspace' }
    )
    const detached = mobileAggregateOutcome(workspace.conversation, 'detachProject', null, {
      kind: 'workspace'
    })
    mockConversationApi.attachProject.mockResolvedValue({ success: true, data: attached })
    mockConversationApi.updateExecutionTarget
      .mockResolvedValueOnce({ success: true, data: retargeted })
      .mockResolvedValueOnce({ success: true, data: workspace })
    mockConversationApi.detachProject.mockResolvedValue({ success: true, data: detached })

    render(<MobileReadyTargetHarness />)
    fireEvent.click(screen.getByRole('button', { name: 'Attach project context' }))
    expect(await screen.findByText(/Project context attached/)).toBeVisible()
    await chooseMobileTarget('Plus the project root')
    expect(await screen.findByText(/Reachable scope updated/)).toBeVisible()
    await chooseMobileTarget('Conversation directory only')
    await waitFor(() => expect(mockConversationApi.updateExecutionTarget).toHaveBeenCalledTimes(2))
    fireEvent.click(screen.getByRole('button', { name: 'Detach project context' }))
    expect(await screen.findByText(/Project context detached/)).toBeVisible()

    const finalRecord = useConversationStore.getState().summariesById[ID]
    expect(finalRecord).toEqual(detached.conversation)
    expect(finalRecord.workspaceCwd).toBe(mobileConversation.workspaceCwd)
    expect(finalRecord.createdAtUtc).toBe(mobileConversation.createdAtUtc)
    expect(finalRecord.creationPartition).toEqual(mobileConversation.creationPartition)
  })

  it('keeps terminal close-view, reopen, and explicit terminate distinct at phone width', async () => {
    render(<PhoneHarness />)
    fireEvent.click(screen.getByRole('button', { name: 'Close terminal view' }))
    await waitFor(() => expect(mockTerminalApi.closeView).toHaveBeenCalledWith('pty-phone'))
    expect(mockTerminalApi.terminate).not.toHaveBeenCalled()
    expect(screen.getByText('hidden')).toBeVisible()

    fireEvent.click(screen.getByRole('button', { name: 'Reopen terminal' }))
    expect(screen.getByText('visible')).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Terminate terminal' }))
    await waitFor(() => expect(mockTerminalApi.terminate).toHaveBeenCalledWith('pty-phone'))
  })

  it('reopens the same canonical Conversation after background/reconnect without terminating PTY', async () => {
    document.dispatchEvent(new Event('visibilitychange'))
    const backgroundEpoch = useConversationStore.getState().beginConversationActivation(ID)
    await expect(
      useConversationStore.getState().activateConversation(ID, backgroundEpoch)
    ).resolves.toBe(true)

    window.dispatchEvent(new Event('online'))
    const reconnectEpoch = useConversationStore.getState().beginConversationActivation(ID)
    await expect(
      useConversationStore.getState().activateConversation(ID, reconnectEpoch)
    ).resolves.toBe(true)

    expect(mockConversationApi.openConversation).toHaveBeenCalledTimes(2)
    expect(useConversationStore.getState().activeConversationId).toBe(ID)
    expect(mockTerminalApi.terminate).not.toHaveBeenCalled()
    expect(useTerminalStore.getState().terminals[0].ptyId).toBe('pty-phone')
  })

  it('preserves a denied cold terminal as a phone placeholder and retries without spawning', async () => {
    useTerminalStore.setState({ terminals: [], activeTerminalId: '', ptyIdIndex: new Map() })
    const persisted = mobileTerminalWorkspace()
    mockSessionWorkspaceApi.getWorkspace.mockResolvedValue({
      success: true,
      data: { status: 'loaded', workspace: persisted }
    })
    mockTerminalApi.resume
      .mockResolvedValueOnce({ success: false, error: 'Unauthorized', code: 'UNAUTHORIZED' })
      .mockImplementationOnce(async (request) => {
        useTerminalStore.getState().appendTranscript(request.terminalId, 'phone replay output')
        return {
          success: true,
          data: {
            terminal: {
              id: request.terminalId,
              shell: 'bash',
              cwd: `/visible/sessions/2026/08/15/${ID}`,
              pid: 91,
              cols: 80,
              rows: 24,
              latestSeq: 14,
              gap: false
            },
            claim: 'phone-memory-only-grant'
          }
        }
      })

    document.dispatchEvent(new Event('visibilitychange'))
    await expect(loadSessionWorkspace(ID)).resolves.toBe(true)
    window.dispatchEvent(new Event('online'))

    const root = useWorkspaceStore.getState().root
    if (root.type !== 'leaf') throw new Error('expected restored phone leaf')
    render(
      <MemoryRouter>
        <PaneContent pane={root} />
      </MemoryRouter>
    )

    expect(screen.getByRole('region', { name: 'Terminal is disconnected' })).toBeVisible()
    expect(screen.getByText(/saved terminal is unavailable/i)).toBeVisible()
    const retry = screen.getByRole('button', { name: 'Retry connection' })
    expect(retry).toBeEnabled()
    expect(useTerminalStore.getState().terminals[0]).toMatchObject({
      id: 'phone-terminal-record',
      ptyId: 'pty-phone-cold',
      healthStatus: 'disconnected',
      claim: undefined
    })

    fireEvent.click(retry)

    expect(await screen.findByTestId('mobile-connected-terminal')).toHaveTextContent(
      'connected:pty-phone-cold'
    )
    expect(screen.queryByRole('region', { name: 'Terminal is disconnected' })).toBeNull()
    expect(useTerminalStore.getState().peekTranscript('pty-phone-cold')).toBe('phone replay output')
    expect(useTerminalStore.getState().terminals[0]).toMatchObject({
      healthStatus: 'running',
      resumeCursor: 14,
      claim: 'phone-memory-only-grant'
    })
    expect(mockTerminalApi.resume).toHaveBeenNthCalledWith(1, {
      conversationId: ID,
      terminalId: 'pty-phone-cold',
      lastSeq: 0
    })
    expect(mockTerminalApi.resume).toHaveBeenNthCalledWith(2, {
      conversationId: ID,
      terminalId: 'pty-phone-cold',
      lastSeq: 0
    })
    expect(mockTerminalApi.spawn).not.toHaveBeenCalled()
    expect(mockTerminalApi.terminate).not.toHaveBeenCalled()
  })

  it('reveals authenticated recovery evidence from redacted phone status at 390px', async () => {
    const originalSnapshot = structuredClone(recoveryItem)
    mockConversationApi.resolveRecovery.mockImplementation(async (request) => ({
      success: true,
      data: {
        recoveryId: recoveryItem.recoveryId,
        action: request.action,
        authorization: request.action === 'inspect' ? 'read' : 'mutation',
        status: request.action === 'inspect' ? 'unresolved' : 'resolvedStartedEmpty',
        recoveryRevision: request.action === 'inspect' ? 3 : 4,
        workspaceRevision: request.action === 'startEmptyWorkspace' ? 1 : null,
        workspaceChanged: request.action === 'startEmptyWorkspace',
        ...recoveryEvidence
      }
    }))
    render(<ConversationRecoveryPanel items={[recoveryItem]} conversationId={ID} embedded />)
    expect(screen.getAllByRole('complementary', { name: 'Conversation recovery' })).toHaveLength(1)
    expect(document.querySelectorAll('[data-conversation-recovery-panel]')).toHaveLength(1)
    for (const action of recoveryItem.suggestedActions) {
      expect(document.querySelectorAll(`[data-recovery-action="${action}"]`)).toHaveLength(1)
    }
    expect(screen.queryByText(/legacy_workspace_manifests\/0\/phone.json/)).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Inspect preserved source' }))
    await waitFor(() => expect(mockConversationApi.resolveRecovery).toHaveBeenCalledTimes(1))
    expect(mockConversationApi.resolveRecovery.mock.calls[0][0]).toEqual({
      recoveryId: recoveryItem.recoveryId,
      expectedRevision: 3,
      action: 'inspect',
      payload: {}
    })
    expect(
      (await screen.findAllByText(/legacy_workspace_manifests\/0\/phone.json/)).length
    ).toBeGreaterThan(0)
    expect(screen.getAllByText(new RegExp(`sha256:${'e'.repeat(64)}`)).length).toBeGreaterThan(0)
    expect(screen.getByText('{"candidate":"phone-preserved"}')).toBeVisible()
    expect(recoveryItem).toEqual(originalSnapshot)

    mockConversationApi.resolveRecovery.mockClear()
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss preserved source' }))
    await waitFor(() => expect(mockConversationApi.resolveRecovery).toHaveBeenCalledTimes(1))
    expect(mockConversationApi.resolveRecovery.mock.calls[0][0]).toEqual({
      recoveryId: recoveryItem.recoveryId,
      expectedRevision: 3,
      action: 'dismissPreservedSource',
      idempotencyKey: expect.stringMatching(
        /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/
      ),
      payload: { reasonCode: 'deferLegacyProjection' }
    })
    expect(document.querySelectorAll('[data-conversation-recovery-panel]')).toHaveLength(1)
  })
})
