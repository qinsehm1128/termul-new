import type { ConversationRecordV2 } from '@shared/types/conversation.types'
import type { ConversationLifecycleOutcome } from '@shared/types/conversation-lifecycle.types'
import { act, cleanup, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const { subscribeMock, projectLifecycleMock } = vi.hoisted(() => ({
  subscribeMock: vi.fn(),
  projectLifecycleMock: vi.fn()
}))

vi.mock('@/lib/conversation-lifecycle-api', () => ({
  conversationLifecycleApi: { subscribe: subscribeMock }
}))

vi.mock('@/stores/acp-store', () => ({
  useAcpStore: {
    getState: () => ({ _onConversationLifecycle: projectLifecycleMock })
  }
}))

vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn() }))

import { useConversationStore } from '@/stores/conversation-store'
import { useConversationLifecycle } from './use-conversation-lifecycle'

const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'

const conversation: ConversationRecordV2 = {
  schemaVersion: 2,
  conversationId,
  createdAtUtc: '2026-08-15T09:45:15.123Z',
  creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
  workspaceCwd: '/workspace/conversation',
  executionTarget: { kind: 'workspace' },
  projectAttachment: null,
  lifecycleState: 'ready',
  lastSeq: 4,
  createdBy: 'termul'
}

function updated(revision: number): ConversationLifecycleOutcome {
  return {
    status: 'updated',
    action: 'suspendBinding',
    conversationId,
    previousRevision: revision - 1,
    revision,
    workspaceCwd: conversation.workspaceCwd,
    lifecycleState: 'ready',
    currentBinding: {
      schemaVersion: 1,
      bindingId: 'binding-1',
      agentSessionId: 'session-1',
      runtimeAgentId: 'agent-1',
      stableAgentNamespace: 'config:test',
      executionCwd: conversation.workspaceCwd,
      boundAtUtc: '2026-08-15T09:45:16.000Z',
      state: 'suspended'
    }
  }
}

function deleted(revision: number): ConversationLifecycleOutcome {
  return {
    status: 'updated',
    action: 'deleteConversation',
    conversationId,
    previousRevision: revision - 1,
    revision,
    workspaceCwd: conversation.workspaceCwd,
    lifecycleState: 'deleted',
    currentBinding: null
  }
}

let lifecycleListener: ((outcome: ConversationLifecycleOutcome) => void) | null = null
let unsubscribeMock: ReturnType<typeof vi.fn>

beforeEach(() => {
  vi.clearAllMocks()
  useConversationStore.getState().reset()
  useConversationStore.getState().replaceSummaries([conversation])
  lifecycleListener = null
  unsubscribeMock = vi.fn()
  subscribeMock.mockImplementation((listener: (outcome: ConversationLifecycleOutcome) => void) => {
    lifecycleListener = listener
    return unsubscribeMock
  })
})

afterEach(() => {
  cleanup()
})

describe('useConversationLifecycle authority ordering', () => {
  it('commits ConversationStore before ACP projection and ignores stale outcomes', () => {
    projectLifecycleMock.mockImplementation(() => {
      expect(useConversationStore.getState().summariesById[conversationId].lastSeq).toBe(6)
    })
    const { unmount } = renderHook(() => useConversationLifecycle())

    act(() => lifecycleListener?.(updated(6)))

    expect(projectLifecycleMock).toHaveBeenCalledTimes(1)

    act(() => lifecycleListener?.(updated(5)))
    expect(projectLifecycleMock).toHaveBeenCalledTimes(1)
    expect(useConversationStore.getState().summariesById[conversationId].lastSeq).toBe(6)

    unmount()
    expect(unsubscribeMock).toHaveBeenCalledTimes(1)
  })

  it('removes active Conversation state before projecting a delete', () => {
    useConversationStore.getState().setActiveConversationId(conversationId)
    projectLifecycleMock.mockImplementation(() => {
      const state = useConversationStore.getState()
      expect(state.summariesById[conversationId]).toBeUndefined()
      expect(state.detailsById[conversationId]).toBeUndefined()
      expect(state.activeConversationId).toBeNull()
    })
    renderHook(() => useConversationLifecycle())

    act(() => lifecycleListener?.(deleted(5)))

    expect(projectLifecycleMock).toHaveBeenCalledTimes(1)
  })
})
