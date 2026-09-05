import type {
  ConversationAggregateMutationAction,
  ConversationAggregateMutationOutcome,
  ConversationRecordV2,
  ExecutionTarget,
  ProjectAttachment
} from '@shared/types/conversation.types'
import type { ConversationLifecycleOutcome } from '@shared/types/conversation-lifecycle.types'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  AGENT_SKILLS_CHANGED_EVENT,
  type AgentSkillsChangedDetail
} from '@/lib/agent-skills-events'
import { conversationApi } from '@/lib/conversation-api'
import { setRouterNavigate } from '@/lib/router-navigate'
import { useAcpStore } from '@/stores/acp-store'
import { selectVisibleConversations, useConversationStore } from '@/stores/conversation-store'
import { useWorkspaceStore } from '@/stores/workspace-store'

const {
  loadSessionWorkspaceMock,
  openHistorySessionMock,
  loadSessionIndexMock,
  addAgentChatTabMock,
  startChatMock
} = vi.hoisted(() => ({
  loadSessionWorkspaceMock: vi.fn(),
  openHistorySessionMock: vi.fn(),
  loadSessionIndexMock: vi.fn(),
  addAgentChatTabMock: vi.fn(),
  startChatMock: vi.fn()
}))

vi.mock('@/lib/conversation-api', () => ({
  conversationApi: {
    listConversations: vi.fn(),
    openConversation: vi.fn(),
    getCurrentBinding: vi.fn(),
    attachProject: vi.fn(),
    detachProject: vi.fn(),
    updateExecutionTarget: vi.fn()
  }
}))

vi.mock('@/hooks/use-session-workspace-sync', () => ({
  loadSessionWorkspace: loadSessionWorkspaceMock
}))

vi.mock('@/hooks/use-editor-persistence', () => ({
  persistState: vi.fn(),
  restoreProjectWorkspace: vi.fn().mockResolvedValue(false)
}))

vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn() }))

const projectlessId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
const attachedId = '028f7a1c-1b4d-7c8a-9f01-0123456789ab'
const rustCanonicalNonVariantId = '038f7a1c-1b4d-1c8a-1f01-0123456789ab'
const navigateMock = vi.fn()

function summary(
  conversationId: string,
  workspaceCwd: string,
  projectId: string | null
): ConversationRecordV2 {
  return {
    schemaVersion: 2,
    conversationId,
    createdAtUtc:
      conversationId === projectlessId ? '2026-08-15T10:00:00.000Z' : '2026-08-15T09:00:00.000Z',
    creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
    workspaceCwd,
    executionTarget: { kind: 'workspace' },
    projectAttachment: projectId
      ? {
          schemaVersion: 1,
          projectId,
          attachedAtUtc: '2026-08-15T09:00:00.000Z',
          projectPathSnapshot: '/projects/attached',
          worktreePath: null,
          worktreeBranch: null
        }
      : null,
    lifecycleState: 'ready',
    lastSeq: 4,
    createdBy: 'se-manager'
  }
}

const projectless = summary(projectlessId, '/conversations/projectless', null)
const attached = summary(attachedId, '/conversations/attached', 'project-1')
const attachment: ProjectAttachment = {
  schemaVersion: 1,
  projectId: 'project-1',
  attachedAtUtc: '2026-08-15T10:15:00.000Z',
  projectPathSnapshot: '/projects/attached',
  worktreePath: null,
  worktreeBranch: null
}

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
    revision: conversation.lastSeq,
    identityBefore: identity,
    identityAfter: identity,
    projectAttachment,
    executionTarget,
    conversation
  }
}

function lifecycleOutcome(
  conversation: ConversationRecordV2,
  revision: number,
  action: 'detachBinding' | 'rebindDetachedBinding' | 'suspendBinding' | 'replaceBinding',
  bindingState: 'active' | 'detached' | 'suspended' = 'active'
): ConversationLifecycleOutcome {
  return {
    status: 'updated',
    action,
    conversationId: conversation.conversationId,
    previousRevision: revision - 1,
    revision,
    workspaceCwd: conversation.workspaceCwd,
    lifecycleState: 'ready',
    currentBinding: {
      schemaVersion: 1,
      bindingId: `binding-${conversation.conversationId}`,
      agentSessionId: `session-${conversation.conversationId}`,
      runtimeAgentId: 'agent-1',
      stableAgentNamespace: 'config:test',
      executionCwd: conversation.workspaceCwd,
      boundAtUtc: '2026-08-15T10:30:00.000Z',
      state: bindingState
    }
  }
}

function deleteOutcome(
  conversation: ConversationRecordV2,
  revision: number
): ConversationLifecycleOutcome {
  return {
    status: 'updated',
    action: 'deleteConversation',
    conversationId: conversation.conversationId,
    previousRevision: revision - 1,
    revision,
    workspaceCwd: conversation.workspaceCwd,
    lifecycleState: 'deleted',
    currentBinding: null
  }
}

function deferred<T>(): {
  promise: Promise<T>
  resolve: (value: T) => void
} {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

beforeEach(() => {
  vi.clearAllMocks()
  useConversationStore.getState().reset()
  loadSessionWorkspaceMock.mockResolvedValue(true)
  openHistorySessionMock.mockResolvedValue(undefined)
  loadSessionIndexMock.mockResolvedValue(undefined)
  startChatMock.mockResolvedValue('opaque/live')
  vi.mocked(conversationApi.getCurrentBinding).mockResolvedValue({
    success: true,
    data: { conversationId: projectlessId, binding: null }
  })
  useAcpStore.setState({
    sessions: {},
    activeSessionId: null,
    sessionIndex: [],
    agentConfigs: [],
    openHistorySession: openHistorySessionMock,
    loadSessionIndex: loadSessionIndexMock,
    startChat: startChatMock
  })
  useWorkspaceStore.setState({ addAgentChatTab: addAgentChatTabMock })
  setRouterNavigate(navigateMock)
  window.location.hash = ''
})

afterEach(() => {
  setRouterNavigate(null)
})

describe('ConversationStore canonical authority', () => {
  it('loads project-less and attached summaries with zero selected projects', async () => {
    vi.mocked(conversationApi.listConversations).mockResolvedValue({
      success: true,
      data: [attached, projectless]
    })

    await expect(useConversationStore.getState().loadConversations()).resolves.toBe(true)
    const state = useConversationStore.getState()
    expect(state.conversationIds).toEqual([projectlessId, attachedId])
    expect(state.summariesById[projectlessId].projectAttachment).toBeNull()
    expect(state.summariesById[attachedId].projectAttachment?.projectId).toBe('project-1')
  })

  it('does not rematerialize deleted Conversations after a list reload', async () => {
    vi.mocked(conversationApi.listConversations).mockResolvedValue({
      success: true,
      data: [{ ...projectless, lifecycleState: 'deleted' }]
    })

    await expect(useConversationStore.getState().loadConversations()).resolves.toBe(true)
    expect(useConversationStore.getState().conversationIds).toEqual([])
    expect(useConversationStore.getState().summariesById[projectlessId]).toBeUndefined()
  })

  it('opens canonical Conversation details without claiming route activation', async () => {
    vi.mocked(conversationApi.openConversation).mockResolvedValue({
      success: true,
      data: {
        conversation: projectless,
        workspace: { status: 'missing', conversationId: projectlessId }
      }
    })

    await expect(
      useConversationStore.getState().openConversation(projectlessId)
    ).resolves.toMatchObject({ conversation: { conversationId: projectlessId } })
    expect(useConversationStore.getState().activeConversationId).toBeNull()
    expect(useConversationStore.getState().detailsById[projectlessId]?.conversation).toEqual(
      projectless
    )
    expect(useAcpStore.getState().activeSessionId).toBeNull()

    useAcpStore.setState({ activeSessionId: 'opaque-runtime-session' })
    expect(useConversationStore.getState().activeConversationId).toBeNull()
    expect(useConversationStore.getState().conversationIds).toEqual([projectlessId])
  })

  it('keeps a listed title when open returns an untitled record', async () => {
    const titled = {
      ...projectless,
      title: 'bi查询demo',
      titleSource: 'derived_first_message' as const
    }
    useConversationStore.getState().replaceSummaries([titled])
    vi.mocked(conversationApi.openConversation).mockResolvedValue({
      success: true,
      data: {
        conversation: { ...projectless, title: null, titleSource: null, lastSeq: 5 },
        workspace: { status: 'missing', conversationId: projectlessId }
      }
    })

    await useConversationStore.getState().openConversation(projectlessId)
    expect(useConversationStore.getState().summariesById[projectlessId]).toMatchObject({
      title: 'bi查询demo',
      titleSource: 'derived_first_message',
      lastSeq: 5
    })
  })

  it('reopens history when the session index binds the Conversation', async () => {
    vi.mocked(conversationApi.openConversation).mockResolvedValue({
      success: true,
      data: {
        conversation: projectless,
        workspace: { status: 'missing', conversationId: projectlessId }
      }
    })
    useAcpStore.setState({
      sessions: {},
      sessionIndex: [
        {
          id: 'opaque/history',
          conversationId: projectlessId,
          agentId: 'agent-1',
          title: 'bi查询demo',
          cwd: projectless.workspaceCwd,
          projectId: '',
          createdAt: 1,
          lastActivityAt: 2,
          messageCount: 4,
          status: 'closed'
        }
      ]
    })

    const epoch = useConversationStore.getState().beginConversationActivation(projectlessId)
    await expect(
      useConversationStore.getState().activateConversation(projectlessId, epoch)
    ).resolves.toBe(true)
    expect(loadSessionIndexMock).toHaveBeenCalled()
    expect(openHistorySessionMock).toHaveBeenCalledWith('opaque/history')
    expect(useAcpStore.getState().activeSessionId).toBe('opaque/history')
    expect(addAgentChatTabMock).toHaveBeenCalledWith(projectlessId, undefined, false)
  })

  it('reopens history from the host binding when the local index is empty', async () => {
    vi.mocked(conversationApi.openConversation).mockResolvedValue({
      success: true,
      data: {
        conversation: projectless,
        workspace: { status: 'missing', conversationId: projectlessId }
      }
    })
    vi.mocked(conversationApi.getCurrentBinding).mockResolvedValue({
      success: true,
      data: {
        conversationId: projectlessId,
        binding: {
          schemaVersion: 1,
          bindingId: '33333333-3333-4333-8333-333333333333',
          agentSessionId: 'opaque/host',
          runtimeAgentId: 'runtime-cursor',
          stableAgentNamespace: 'config:acp-registry:cursor',
          executionCwd: projectless.workspaceCwd,
          boundAtUtc: '2026-08-15T10:30:00.000Z',
          state: 'active'
        }
      }
    })

    const epoch = useConversationStore.getState().beginConversationActivation(projectlessId)
    await expect(
      useConversationStore.getState().activateConversation(projectlessId, epoch)
    ).resolves.toBe(true)
    expect(openHistorySessionMock).toHaveBeenCalledWith('opaque/host')
    expect(useAcpStore.getState().activeSessionId).toBe('opaque/host')
    expect(addAgentChatTabMock).toHaveBeenCalledWith(projectlessId, undefined, false)
  })

  it('shows the Conversation before background ACP reconnect completes', async () => {
    vi.mocked(conversationApi.openConversation).mockResolvedValue({
      success: true,
      data: {
        conversation: projectless,
        workspace: { status: 'missing', conversationId: projectlessId }
      }
    })
    useAcpStore.setState({
      sessions: {},
      sessionIndex: [
        {
          id: 'opaque/history',
          conversationId: projectlessId,
          agentId: 'agent-1',
          title: 'bi查询demo',
          cwd: projectless.workspaceCwd,
          projectId: '',
          createdAt: 1,
          lastActivityAt: 2,
          messageCount: 4,
          status: 'closed'
        }
      ]
    })
    const reconnect = deferred<void>()
    openHistorySessionMock.mockReturnValueOnce(reconnect.promise)
    const changedRoots: string[] = []
    const onSkillsChanged = (event: Event): void => {
      changedRoots.push((event as CustomEvent<AgentSkillsChangedDetail>).detail.root)
    }
    window.addEventListener(AGENT_SKILLS_CHANGED_EVENT, onSkillsChanged)

    const epoch = useConversationStore.getState().beginConversationActivation(projectlessId)
    await expect(
      useConversationStore.getState().activateConversation(projectlessId, epoch)
    ).resolves.toBe(true)
    window.removeEventListener(AGENT_SKILLS_CHANGED_EVENT, onSkillsChanged)

    expect(useConversationStore.getState().openingById[projectlessId]).toBe(false)
    expect(changedRoots).toContain(projectless.workspaceCwd)
    expect(useAcpStore.getState().activeSessionId).toBe('opaque/history')
    expect(addAgentChatTabMock).toHaveBeenCalledWith(projectlessId, undefined, false)
    expect(openHistorySessionMock).toHaveBeenCalledWith('opaque/history')

    reconnect.resolve(undefined)
    await reconnect.promise
  })

  it('keeps the same ACP session id when history reopen stays closed', async () => {
    vi.mocked(conversationApi.openConversation).mockResolvedValue({
      success: true,
      data: {
        conversation: projectless,
        workspace: { status: 'missing', conversationId: projectlessId }
      }
    })
    useAcpStore.setState({
      sessions: {},
      sessionIndex: [
        {
          id: 'opaque/history',
          conversationId: projectlessId,
          agentId: 'agent-1',
          agentConfigId: 'pi',
          title: 'bi查询demo',
          cwd: projectless.workspaceCwd,
          projectId: '',
          createdAt: 1,
          lastActivityAt: 2,
          messageCount: 4,
          status: 'closed'
        }
      ]
    })

    const epoch = useConversationStore.getState().beginConversationActivation(projectlessId)
    await expect(
      useConversationStore.getState().activateConversation(projectlessId, epoch)
    ).resolves.toBe(true)
    expect(openHistorySessionMock).toHaveBeenCalledWith('opaque/history')
    expect(startChatMock).not.toHaveBeenCalled()
    expect(useAcpStore.getState().activeSessionId).toBe('opaque/history')
  })

  it('opens a chat tab even when the Conversation has no agent binding', async () => {
    vi.mocked(conversationApi.openConversation).mockResolvedValue({
      success: true,
      data: {
        conversation: projectless,
        workspace: { status: 'missing', conversationId: projectlessId }
      }
    })

    const epoch = useConversationStore.getState().beginConversationActivation(projectlessId)
    await expect(
      useConversationStore.getState().activateConversation(projectlessId, epoch)
    ).resolves.toBe(true)
    expect(useAcpStore.getState().activeSessionId).toBeNull()
    expect(addAgentChatTabMock).toHaveBeenCalledWith(projectlessId, undefined, false)
  })

  it('rejects an opaque ACP session id without using it as a store key', async () => {
    await expect(
      useConversationStore.getState().openConversation('opaque/acp-session')
    ).resolves.toBeNull()
    expect(conversationApi.openConversation).not.toHaveBeenCalled()
    expect(useConversationStore.getState().conversationIds).toEqual([])
    expect(useConversationStore.getState().activeConversationId).toBeNull()
  })

  it('uses the shared Rust-compatible parser for canonical non-variant UUID spellings', async () => {
    const migrated = summary(rustCanonicalNonVariantId, '/conversations/migrated', null)
    vi.mocked(conversationApi.openConversation).mockResolvedValue({
      success: true,
      data: {
        conversation: migrated,
        workspace: { status: 'missing', conversationId: rustCanonicalNonVariantId }
      }
    })

    await expect(
      useConversationStore.getState().openConversation(rustCanonicalNonVariantId)
    ).resolves.toMatchObject({ conversation: { conversationId: rustCanonicalNonVariantId } })
    expect(conversationApi.openConversation).toHaveBeenCalledWith(rustCanonicalNonVariantId)
  })

  it('applies lifecycle outcomes by revision and ignores stale or duplicate outcomes', () => {
    useConversationStore.getState().replaceSummaries([projectless])
    useConversationStore.setState({
      detailsById: {
        [projectlessId]: {
          conversation: projectless,
          workspace: { status: 'missing', conversationId: projectlessId }
        }
      }
    })

    const newest = lifecycleOutcome(projectless, 6, 'suspendBinding', 'suspended')
    const stale = lifecycleOutcome(projectless, 5, 'detachBinding', 'detached')

    expect(useConversationStore.getState().applyLifecycleOutcome(newest)).toBe(true)
    expect(useConversationStore.getState().applyLifecycleOutcome(stale)).toBe(false)
    expect(useConversationStore.getState().applyLifecycleOutcome(newest)).toBe(false)

    const state = useConversationStore.getState()
    expect(state.summariesById[projectlessId]).toMatchObject({
      lifecycleState: 'ready',
      lastSeq: 6
    })
    expect(state.detailsById[projectlessId]?.conversation.lastSeq).toBe(6)
    expect(state.conversationIds).toEqual([projectlessId])
  })

  it('deletes active and non-active Conversation state without removing terminal tabs', () => {
    useConversationStore.getState().replaceSummaries([projectless, attached])
    useConversationStore.setState({
      activeConversationId: projectlessId,
      detailsById: {
        [projectlessId]: {
          conversation: projectless,
          workspace: { status: 'missing', conversationId: projectlessId }
        },
        [attachedId]: {
          conversation: attached,
          workspace: { status: 'missing', conversationId: attachedId }
        }
      },
      openingById: { [projectlessId]: true, [attachedId]: true },
      aggregateBusyById: { [projectlessId]: true, [attachedId]: true },
      errorsById: {
        [projectlessId]: { code: 'TEST', message: 'test' },
        [attachedId]: { code: 'TEST', message: 'test' }
      }
    })
    useWorkspaceStore.setState({
      root: {
        type: 'leaf',
        id: 'pane-lifecycle',
        activeTabId: `chat-${projectlessId}`,
        tabs: [
          { type: 'terminal', id: 'term-live', terminalId: 'terminal-live' },
          { type: 'agent-chat', id: `chat-${projectlessId}`, conversationId: projectlessId },
          { type: 'agent-chat', id: `chat-${attachedId}`, conversationId: attachedId }
        ]
      },
      activePaneId: 'pane-lifecycle'
    })

    expect(useConversationStore.getState().applyLifecycleOutcome(deleteOutcome(attached, 5))).toBe(
      true
    )
    expect(useConversationStore.getState().activeConversationId).toBe(projectlessId)
    expect(useConversationStore.getState().summariesById[attachedId]).toBeUndefined()

    window.location.hash = `#/c/${projectlessId}`
    expect(
      useConversationStore.getState().applyLifecycleOutcome(deleteOutcome(projectless, 5))
    ).toBe(true)
    const state = useConversationStore.getState()
    expect(state.activeConversationId).toBeNull()
    expect(state.conversationIds).toEqual([])
    expect(state.detailsById[projectlessId]).toBeUndefined()
    expect(state.openingById[projectlessId]).toBeUndefined()
    expect(state.aggregateBusyById[projectlessId]).toBeUndefined()
    expect(state.errorsById[projectlessId]).toBeUndefined()
    expect(navigateMock).toHaveBeenCalledWith('/conversations')

    useConversationStore.getState().replaceSummaries([projectless, attached])
    expect(useConversationStore.getState().conversationIds).toEqual([])

    expect(useWorkspaceStore.getState().root).toMatchObject({
      type: 'leaf',
      tabs: [{ type: 'terminal', id: 'term-live', terminalId: 'terminal-live' }]
    })
  })

  it('uses one epoch for A to B to A and suppresses both older completions', async () => {
    const firstA = deferred<Awaited<ReturnType<typeof conversationApi.openConversation>>>()
    const middleB = deferred<Awaited<ReturnType<typeof conversationApi.openConversation>>>()
    const latestA = deferred<Awaited<ReturnType<typeof conversationApi.openConversation>>>()
    let aCalls = 0
    vi.mocked(conversationApi.openConversation).mockImplementation((conversationId) => {
      if (conversationId === attachedId) return middleB.promise
      aCalls += 1
      return aCalls === 1 ? firstA.promise : latestA.promise
    })
    useAcpStore.setState({
      sessions: {
        'session-a': {
          id: 'session-a',
          conversationId: projectlessId,
          agentId: 'agent-a',
          cwd: projectless.workspaceCwd,
          projectId: '',
          status: 'active',
          activeTurn: false,
          openTurnId: null,
          modes: null,
          configOptions: [],
          lastError: null,
          createdAt: 1
        },
        'session-b': {
          id: 'session-b',
          conversationId: attachedId,
          agentId: 'agent-b',
          cwd: attached.workspaceCwd,
          projectId: 'project-1',
          status: 'active',
          activeTurn: false,
          openTurnId: null,
          modes: null,
          configOptions: [],
          lastError: null,
          createdAt: 2
        }
      },
      sessionIndex: []
    })

    const store = useConversationStore.getState()
    const epochA1 = store.beginConversationActivation(projectlessId)
    const activationA1 = store.activateConversation(projectlessId, epochA1)
    const epochB = store.beginConversationActivation(attachedId)
    const activationB = store.activateConversation(attachedId, epochB)
    const epochA2 = store.beginConversationActivation(projectlessId)
    const activationA2 = store.activateConversation(projectlessId, epochA2)

    latestA.resolve({
      success: true,
      data: {
        conversation: projectless,
        workspace: { status: 'missing', conversationId: projectlessId }
      }
    })
    await expect(activationA2).resolves.toBe(true)

    middleB.resolve({
      success: true,
      data: {
        conversation: attached,
        workspace: { status: 'missing', conversationId: attachedId }
      }
    })
    firstA.resolve({
      success: true,
      data: {
        conversation: projectless,
        workspace: { status: 'missing', conversationId: projectlessId }
      }
    })
    await expect(Promise.all([activationA1, activationB])).resolves.toEqual([false, false])

    expect(useConversationStore.getState().activeConversationId).toBe(projectlessId)
    expect(useAcpStore.getState().activeSessionId).toBe('session-a')
    expect(loadSessionWorkspaceMock).toHaveBeenCalledTimes(1)
    expect(loadSessionWorkspaceMock).toHaveBeenCalledWith(projectlessId, expect.any(Function))
    expect(addAgentChatTabMock).toHaveBeenCalledTimes(1)
    expect(addAgentChatTabMock).toHaveBeenCalledWith(projectlessId, undefined, false)
  })

  it('applies optional search/project filters without mutating attachment or cwd invariants', () => {
    useConversationStore.getState().replaceSummaries([projectless, attached])
    const before = structuredClone(useConversationStore.getState().summariesById)

    useConversationStore.getState().setProjectFilter('project-1')
    expect(selectVisibleConversations(useConversationStore.getState())).toEqual([attached])
    useConversationStore.getState().setProjectFilter(null)
    useConversationStore.getState().setSearchQuery('projectless')
    expect(selectVisibleConversations(useConversationStore.getState())).toEqual([projectless])
    expect(useConversationStore.getState().summariesById).toEqual(before)
  })

  it('keeps stable not-found and recovery-required errors for retry UI', async () => {
    vi.mocked(conversationApi.openConversation)
      .mockResolvedValueOnce({
        success: false,
        code: 'CONVERSATION_NOT_FOUND',
        error: 'missing'
      })
      .mockResolvedValueOnce({
        success: false,
        code: 'CONVERSATION_RECOVERY_REQUIRED',
        error: 'recover'
      })

    await useConversationStore.getState().openConversation(projectlessId)
    expect(useConversationStore.getState().errorsById[projectlessId]?.code).toBe(
      'CONVERSATION_NOT_FOUND'
    )
    await useConversationStore.getState().openConversation(projectlessId)
    expect(useConversationStore.getState().errorsById[projectlessId]?.code).toBe(
      'CONVERSATION_RECOVERY_REQUIRED'
    )
  })

  it('uses the current revision and applies attach, retarget, workspace, and detach atomically', async () => {
    useConversationStore.getState().replaceSummaries([projectless])
    const attachedOutcome = aggregateOutcome(
      projectless,
      'attachProject',
      attachment,
      projectless.executionTarget
    )
    const target: ExecutionTarget = {
      kind: 'project_root',
      projectId: attachment.projectId,
      projectRoot: attachment.projectPathSnapshot
    }
    const targetedOutcome = aggregateOutcome(
      attachedOutcome.conversation,
      'updateExecutionTarget',
      attachment,
      target
    )
    const workspaceOutcome = aggregateOutcome(
      targetedOutcome.conversation,
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
      .mockResolvedValueOnce({ success: true, data: targetedOutcome })
      .mockResolvedValueOnce({ success: true, data: workspaceOutcome })
    vi.mocked(conversationApi.detachProject).mockResolvedValue({
      success: true,
      data: detachedOutcome
    })

    await expect(
      useConversationStore.getState().attachProject(projectlessId, attachment)
    ).resolves.toEqual(attachedOutcome)
    await expect(
      useConversationStore.getState().updateExecutionTarget(projectlessId, target)
    ).resolves.toEqual(targetedOutcome)
    await expect(
      useConversationStore.getState().updateExecutionTarget(projectlessId, { kind: 'workspace' })
    ).resolves.toEqual(workspaceOutcome)
    await expect(useConversationStore.getState().detachProject(projectlessId)).resolves.toEqual(
      detachedOutcome
    )

    expect(conversationApi.attachProject).toHaveBeenCalledWith(projectlessId, 4, attachment)
    expect(conversationApi.updateExecutionTarget).toHaveBeenNthCalledWith(
      1,
      projectlessId,
      5,
      target
    )
    expect(conversationApi.updateExecutionTarget).toHaveBeenNthCalledWith(2, projectlessId, 6, {
      kind: 'workspace'
    })
    expect(conversationApi.detachProject).toHaveBeenCalledWith(projectlessId, 7)
    const finalRecord = useConversationStore.getState().summariesById[projectlessId]
    expect(finalRecord).toEqual(detachedOutcome.conversation)
    expect(finalRecord.workspaceCwd).toBe(projectless.workspaceCwd)
    expect(finalRecord.createdAtUtc).toBe(projectless.createdAtUtc)
    expect(useConversationStore.getState().aggregateBusyById[projectlessId]).toBe(false)
  })

  it('fails closed when a host aggregate outcome changes immutable identity', async () => {
    useConversationStore.getState().replaceSummaries([projectless])
    const invalid = aggregateOutcome(
      projectless,
      'attachProject',
      attachment,
      projectless.executionTarget
    )
    invalid.conversation = {
      ...invalid.conversation,
      workspaceCwd: '/unexpected/changed-workspace'
    }
    vi.mocked(conversationApi.attachProject).mockResolvedValue({ success: true, data: invalid })

    await expect(
      useConversationStore.getState().attachProject(projectlessId, attachment)
    ).resolves.toBeNull()
    expect(useConversationStore.getState().summariesById[projectlessId]).toEqual(projectless)
    expect(useConversationStore.getState().errorsById[projectlessId]?.code).toBe(
      'CONVERSATION_IDENTITY_CHANGED'
    )
  })
})
