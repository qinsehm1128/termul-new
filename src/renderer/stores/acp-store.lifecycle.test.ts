import type { ConversationRecordV2 } from '@shared/types/conversation.types'
import type { ConversationLifecycleOutcome } from '@shared/types/conversation-lifecycle.types'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const { closeViewSpy, closeTerminalViewSpy, remapViewSpy, invokeSpy, logFrontendError } =
  vi.hoisted(() => ({
    closeViewSpy: vi.fn(),
    closeTerminalViewSpy: vi.fn(),
    remapViewSpy: vi.fn(),
    invokeSpy: vi.fn(),
    logFrontendError: vi.fn()
  }))

vi.mock('@/stores/workspace-store', () => ({
  useWorkspaceStore: {
    getState: () => ({
      closeChatView: closeViewSpy,
      closeTerminalView: closeTerminalViewSpy,
      remapAgentChatSession: remapViewSpy,
      addAgentChatTab: vi.fn(),
      getActiveTab: vi.fn(() => undefined)
    })
  }
}))

vi.mock('@/lib/acp-history-persistence', async (importActual) => {
  const actual = await importActual<typeof import('@/lib/acp-history-persistence')>()
  return {
    ...actual,
    loadSessionIndex: vi.fn(async () => []),
    saveSessionIndex: vi.fn(async () => {}),
    queueSessionPayloadSave: vi.fn(async () => {})
  }
})
vi.mock('@/lib/log-api', () => ({ logFrontendError }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => true }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeSpy }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

import { conversationApi } from '@/lib/conversation-api'
import { terminalApi } from '@/lib/terminal-api'
import { useAcpStore } from './acp-store'
import { useConversationStore } from './conversation-store'
import { useTerminalStore } from './terminal-store'

const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'

const conversation: ConversationRecordV2 = {
  schemaVersion: 2,
  conversationId,
  createdAtUtc: '2026-08-15T09:45:15.123Z',
  creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
  workspaceCwd: '/visible/conversation',
  executionTarget: { kind: 'workspace' },
  projectAttachment: null,
  lifecycleState: 'ready',
  lastSeq: 4,
  createdBy: 'termul'
}

function updated(
  action: 'detachBinding' | 'rebindDetachedBinding' | 'suspendBinding' | 'replaceBinding',
  state: 'active' | 'detached' | 'suspended',
  sessionId = 'session-old',
  revision = 5
): ConversationLifecycleOutcome {
  return {
    status: 'updated',
    action,
    conversationId,
    previousRevision: revision - 1,
    revision,
    workspaceCwd: '/visible/conversation',
    lifecycleState: 'ready',
    currentBinding: {
      schemaVersion: 1,
      bindingId: 'b2832b54-2ca4-4db4-93fd-f93bf6793114',
      agentSessionId: sessionId,
      runtimeAgentId: 'agent-1',
      stableAgentNamespace: 'config:test',
      executionCwd: '/visible/conversation',
      boundAtUtc: '2026-08-15T09:45:16.000Z',
      state
    }
  }
}

function seed(): void {
  useConversationStore.getState().reset()
  useTerminalStore.setState({
    terminals: [],
    activeTerminalId: '',
    ptyIdIndex: new Map(),
    cleanupRecoveries: {}
  })
  useConversationStore.getState().replaceSummaries([conversation])
  useAcpStore.setState({
    sessionIndex: [
      {
        id: 'session-old',
        conversationId,
        agentId: 'agent-1',
        title: 'Lifecycle chat',
        cwd: '/visible/conversation',
        projectId: '',
        createdAt: 1,
        lastActivityAt: 2,
        messageCount: 1,
        lastSeq: 4,
        status: 'active'
      }
    ],
    sessions: {
      'session-old': {
        id: 'session-old',
        conversationId,
        agentId: 'agent-1',
        cwd: '/visible/conversation',
        projectId: '',
        status: 'active',
        title: 'Lifecycle chat',
        activeTurn: false,
        openTurnId: null,
        modes: null,
        configOptions: [],
        lastError: null,
        createdAt: 1
      }
    },
    activeSessionId: 'session-old',
    messages: {
      'session-old': [
        {
          id: 'message-1',
          role: 'user',
          blocks: [{ type: 'text', text: 'retained transcript' }],
          streaming: false,
          timestamp: 1
        }
      ]
    },
    toolCalls: { 'session-old': [] },
    plans: { 'session-old': [] },
    commands: { 'session-old': [] },
    sessionUsage: {},
    promptQueues: {},
    suppressQueueFlush: {},
    restoringChatIds: {},
    launchingSessionIds: {},
    degradedRecoverySessions: {},
    pendingPermissions: {},
    pendingQuestions: {}
  })
}

describe('ACP Conversation lifecycle store', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    seed()
  })

  it('closes only the renderer view and preserves session and transcript state', () => {
    useAcpStore.getState().closeChatView(conversationId)

    expect(closeViewSpy).toHaveBeenCalledWith(conversationId)
    expect(invokeSpy).not.toHaveBeenCalled()
    expect(useAcpStore.getState().sessions['session-old']).toBeDefined()
    expect(useAcpStore.getState().messages['session-old']?.[0].blocks[0]).toMatchObject({
      text: 'retained transcript'
    })
  })

  it('preserves Conversation transcript maps when provider close emits session_closed', () => {
    useAcpStore.getState()._onSessionClosed({ agentId: 'agent-1', sessionId: 'session-old' })

    expect(useAcpStore.getState().sessions['session-old']?.status).toBe('closed')
    expect(useAcpStore.getState().messages['session-old']).toHaveLength(1)
  })

  it('keeps ACP lifecycle handling as a derived projection with no route ownership', () => {
    useAcpStore.getState()._onConversationLifecycle({
      status: 'updated',
      action: 'deleteConversation',
      conversationId,
      previousRevision: 4,
      revision: 5,
      workspaceCwd: '/visible/conversation',
      lifecycleState: 'deleted',
      currentBinding: null
    })

    expect(useConversationStore.getState().summariesById[conversationId]).toEqual(conversation)
    expect(useAcpStore.getState().sessionIndex).toEqual([])
    expect(closeViewSpy).not.toHaveBeenCalled()
  })

  it('rejects a non-canonical lifecycle id before the production facade dispatches', async () => {
    await expect(
      useAcpStore.getState().detachAgentBinding('018F7A1C-1B4D-7C8A-9F01-0123456789AB')
    ).rejects.toMatchObject({ code: 'VALIDATION_ERROR' })
    expect(invokeSpy).not.toHaveBeenCalled()
  })

  it('dispatches detach, rebind, and suspend through the real production lifecycle factory', async () => {
    invokeSpy.mockResolvedValueOnce({ success: true, data: updated('detachBinding', 'detached') })
    await useAcpStore.getState().detachAgentBinding(conversationId)
    expect(invokeSpy).toHaveBeenNthCalledWith(1, 'conversation_detach_binding', {
      conversationId,
      expectedRevision: 4
    })

    useAcpStore.setState((state) => ({
      sessionIndex: state.sessionIndex.map((entry) => ({ ...entry, lastSeq: 5 }))
    }))
    invokeSpy.mockResolvedValueOnce({
      success: true,
      data: updated('rebindDetachedBinding', 'active', 'session-old', 6)
    })
    await useAcpStore.getState().rebindDetachedBinding(conversationId)
    expect(invokeSpy).toHaveBeenNthCalledWith(2, 'conversation_rebind_detached_binding', {
      conversationId,
      expectedRevision: 5
    })

    invokeSpy.mockResolvedValueOnce({
      success: true,
      data: updated('suspendBinding', 'suspended', 'session-old', 7)
    })
    await useAcpStore.getState().suspendAgentBinding(conversationId)
    expect(invokeSpy).toHaveBeenNthCalledWith(3, 'conversation_suspend_binding', {
      conversationId,
      expectedRevision: 6
    })
    expect(useAcpStore.getState().messages['session-old']).toHaveLength(1)
  })

  it('dispatches replace through the real factory while retaining identity and transcript maps', async () => {
    invokeSpy.mockResolvedValueOnce({
      success: true,
      data: updated('replaceBinding', 'active', 'session-new')
    })

    await useAcpStore.getState().replaceAgentBinding(conversationId)

    expect(invokeSpy).toHaveBeenCalledWith(
      'conversation_replace_binding',
      expect.objectContaining({
        conversationId,
        expectedRevision: 4,
        request: expect.objectContaining({
          conversationId,
          executionTarget: { kind: 'workspace' }
        })
      })
    )
    expect(useAcpStore.getState().sessions['session-old']).toBeUndefined()
    expect(useAcpStore.getState().sessions['session-new']?.conversationId).toBe(conversationId)
    expect(useAcpStore.getState().messages['session-new']).toHaveLength(1)
    expect(remapViewSpy).not.toHaveBeenCalled()
  })

  it('switching agents spawns the chosen config and binds the Conversation to it', async () => {
    // Switching needs a LIVE process to hand the session to, so the store must
    // spawn the target config first and pass THAT runtime id — not the config id
    // and not the Conversation's current agent.
    useAcpStore.setState((state) => ({
      agentConfigs: [
        {
          id: 'cfg-other',
          configId: 'cfg-other',
          name: 'Other agent',
          command: 'other',
          args: [],
          env: {},
          allowTerminal: false,
          permissionPolicy: 'ask'
        },
        ...state.agentConfigs
      ]
    }))
    invokeSpy.mockImplementation(async (command: string) => {
      if (command === 'acp_spawn_agent') {
        return { agentId: 'agent-other-runtime', capabilities: {}, authMethods: [] }
      }
      if (command === 'conversation_replace_binding') {
        return { success: true, data: updated('replaceBinding', 'active', 'session-new') }
      }
      throw new Error(`unexpected invoke in agent-switch test: ${command}`)
    })

    await useAcpStore.getState().replaceAgentBinding(conversationId, 'cfg-other')

    expect(invokeSpy).toHaveBeenCalledWith(
      'acp_spawn_agent',
      expect.objectContaining({
        // The agent runs in the Conversation's own directory, as always.
        config: expect.objectContaining({ configId: 'cfg-other' })
      })
    )
    expect(invokeSpy).toHaveBeenCalledWith(
      'conversation_replace_binding',
      expect.objectContaining({
        conversationId,
        targetRuntimeAgentId: 'agent-other-runtime'
      })
    )
    invokeSpy.mockReset()
  })

  it('a plain restart sends no target so the Conversation keeps its current agent', async () => {
    invokeSpy.mockResolvedValueOnce({
      success: true,
      data: updated('replaceBinding', 'active', 'session-new')
    })

    await useAcpStore.getState().replaceAgentBinding(conversationId)

    const call = invokeSpy.mock.calls.find(([name]) => name === 'conversation_replace_binding')
    expect(call).toBeTruthy()
    expect(call?.[1]).not.toHaveProperty('targetRuntimeAgentId')
  })

  it('dispatches blocked delete through the real factory and keeps state intact', async () => {
    invokeSpy.mockResolvedValueOnce({
      success: true,
      data: {
        status: 'blocked',
        action: 'deleteConversation',
        conversationId,
        revision: 4,
        code: 'CONVERSATION_LIVE_RESOURCES',
        blockers: [
          { kind: 'liveBinding', count: 1, ids: ['session-old'] },
          { kind: 'terminalResources', count: 1, ids: ['terminal-live'] }
        ]
      }
    })

    const outcome = await useAcpStore.getState().deleteConversation(conversationId)

    expect(invokeSpy).toHaveBeenCalledWith('conversation_delete', {
      conversationId,
      expectedRevision: 4
    })
    expect(outcome.status).toBe('blocked')
    expect(useAcpStore.getState().sessionIndex).toHaveLength(1)
    expect(useAcpStore.getState().sessions['session-old']).toBeDefined()
    expect(closeViewSpy).not.toHaveBeenCalled()
  })

  it('applies explicit tombstone returned by the real delete route without resource teardown', async () => {
    invokeSpy.mockResolvedValueOnce({
      success: true,
      data: {
        status: 'updated',
        action: 'deleteConversation',
        conversationId,
        previousRevision: 4,
        revision: 4,
        workspaceCwd: '/visible/conversation',
        lifecycleState: 'deleted',
        currentBinding: {
          ...updated('suspendBinding', 'suspended').currentBinding!,
          state: 'suspended'
        }
      }
    })

    await useAcpStore.getState().deleteConversation(conversationId)

    expect(invokeSpy).toHaveBeenCalledWith('conversation_delete', {
      conversationId,
      expectedRevision: 4
    })
    expect(useAcpStore.getState().sessionIndex).toEqual([])
    expect(useAcpStore.getState().sessions['session-old']).toBeUndefined()
    expect(closeViewSpy).toHaveBeenCalledWith(conversationId)
  })

  it('terminates conversation-scoped terminals before the host delete route', async () => {
    useTerminalStore.setState({
      terminals: [
        {
          id: 'term-chat',
          conversationId,
          name: 'Chat shell',
          projectId: '',
          shell: 'bash',
          cwd: '/visible/conversation',
          output: [],
          healthStatus: 'running',
          viewState: 'visible',
          isHidden: false,
          ptyId: 'pty-live'
        },
        {
          id: 'term-project',
          name: 'Project shell',
          projectId: 'project-1',
          shell: 'bash',
          cwd: '/visible/project',
          output: [],
          healthStatus: 'running',
          viewState: 'visible',
          isHidden: false,
          ptyId: 'pty-other'
        }
      ],
      activeTerminalId: 'term-chat',
      ptyIdIndex: new Map([
        ['pty-live', 'term-chat'],
        ['pty-other', 'term-project']
      ]),
      cleanupRecoveries: {}
    })
    const terminateSpy = vi.spyOn(terminalApi, 'terminate').mockResolvedValue({
      success: true
    })
    invokeSpy.mockResolvedValueOnce({
      success: true,
      data: {
        status: 'updated',
        action: 'deleteConversation',
        conversationId,
        previousRevision: 4,
        revision: 5,
        workspaceCwd: '/visible/conversation',
        lifecycleState: 'deleted',
        currentBinding: null
      }
    })

    await useAcpStore.getState().deleteConversation(conversationId)

    expect(terminateSpy).toHaveBeenCalledTimes(1)
    expect(terminateSpy).toHaveBeenCalledWith('pty-live')
    expect(closeTerminalViewSpy).toHaveBeenCalledWith('term-chat')
    expect(useTerminalStore.getState().terminals.map((item) => item.id)).toEqual(['term-project'])
    expect(invokeSpy).toHaveBeenCalledWith('conversation_delete', {
      conversationId,
      expectedRevision: 4
    })
    terminateSpy.mockRestore()
  })

  it('reloads the conversation list when the host reports a new session', async () => {
    const phoneConversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789cd'
    const phoneConversation: ConversationRecordV2 = {
      ...conversation,
      conversationId: phoneConversationId,
      workspaceCwd: '/visible/phone',
      lastSeq: 0
    }
    const listSpy = vi.spyOn(conversationApi, 'listConversations').mockResolvedValue({
      success: true,
      data: [conversation, phoneConversation]
    })

    useAcpStore.getState()._onSessionCreated({
      agentId: 'agent-1',
      sessionId: 'session-phone'
    })

    await vi.waitFor(() => {
      expect(listSpy).toHaveBeenCalledTimes(1)
      expect(useConversationStore.getState().summariesById[phoneConversationId]).toBeDefined()
    })
    expect(useAcpStore.getState().sessions['session-phone']?.agentId).toBe('agent-1')
    expect(logFrontendError).toHaveBeenCalledWith({
      level: 'warn',
      source: 'acp-store.sessionCreated',
      message: 'Reloading conversation list after remote session_created'
    })
    listSpy.mockRestore()
  })
})
