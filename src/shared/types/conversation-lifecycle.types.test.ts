import { describe, expect, it } from 'vitest'
import {
  CONVERSATION_LIFECYCLE_ACTIONS,
  CONVERSATION_LIFECYCLE_ERROR_CODES,
  type ConversationLifecycleOutcome,
  parseConversationLifecycleOutcome,
  parseConversationReplacementRequest
} from './conversation-lifecycle.types'

const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'

describe('Conversation lifecycle wire contract', () => {
  it('pins camelCase lifecycle action discriminators', () => {
    expect(CONVERSATION_LIFECYCLE_ACTIONS).toEqual([
      'detachBinding',
      'rebindDetachedBinding',
      'suspendBinding',
      'replaceBinding',
      'deleteConversation'
    ])
  })

  it('pins stable conflict, blocker, provider, and recovery codes', () => {
    expect(CONVERSATION_LIFECYCLE_ERROR_CODES).toContain('CONVERSATION_CONFLICT')
    expect(CONVERSATION_LIFECYCLE_ERROR_CODES).toContain('CONVERSATION_LIVE_RESOURCES')
    expect(CONVERSATION_LIFECYCLE_ERROR_CODES).toContain('ACP_CLOSE_UNSUPPORTED')
    expect(CONVERSATION_LIFECYCLE_ERROR_CODES).toContain('CONVERSATION_RECOVERY_REQUIRED')
  })

  it('distinguishes updated binding state from delete blockers', () => {
    const detached: ConversationLifecycleOutcome = {
      status: 'updated',
      action: 'detachBinding',
      conversationId,
      previousRevision: 4,
      revision: 5,
      workspaceCwd: '/visible/conversation',
      lifecycleState: 'ready',
      currentBinding: {
        schemaVersion: 1,
        bindingId: 'b2832b54-2ca4-4db4-93fd-f93bf6793114',
        agentSessionId: 'opaque/session',
        runtimeAgentId: 'agent-runtime',
        stableAgentNamespace: 'config:test',
        executionCwd: '/visible/conversation',
        boundAtUtc: '2026-08-15T09:45:16.000Z',
        state: 'detached'
      }
    }
    const blocked: ConversationLifecycleOutcome = {
      status: 'blocked',
      action: 'deleteConversation',
      conversationId,
      revision: 5,
      code: 'CONVERSATION_LIVE_RESOURCES',
      blockers: [
        { kind: 'liveBinding', count: 1, ids: ['opaque/session'] },
        { kind: 'terminalResources', count: 2, ids: ['terminal-1', 'terminal-2'] }
      ]
    }
    expect(detached.currentBinding?.state).toBe('detached')
    expect(blocked.blockers.map((blocker) => blocker.kind)).toEqual([
      'liveBinding',
      'terminalResources'
    ])
  })

  it('parses every exact lifecycle action and rejects malformed blocker outcomes', () => {
    const binding = {
      schemaVersion: 1 as const,
      bindingId: 'b2832b54-2ca4-4db4-93fd-f93bf6793114',
      agentSessionId: 'opaque/session',
      runtimeAgentId: 'agent-runtime',
      stableAgentNamespace: 'config:test',
      executionCwd: '/visible/conversation',
      boundAtUtc: '2026-08-15T09:45:16.000Z',
      state: 'active' as const
    }
    const actions = [
      'detachBinding',
      'rebindDetachedBinding',
      'suspendBinding',
      'replaceBinding',
      'deleteConversation'
    ] as const
    for (const action of actions) {
      const outcome = {
        status: 'updated' as const,
        action,
        conversationId,
        previousRevision: 4,
        revision: action === 'deleteConversation' ? 4 : 5,
        workspaceCwd: '/visible/conversation',
        lifecycleState: action === 'deleteConversation' ? ('deleted' as const) : ('ready' as const),
        currentBinding: action === 'deleteConversation' ? null : binding,
        ...(action === 'replaceBinding' ? { previousAgentSessionId: 'opaque/previous' } : {})
      }
      expect(parseConversationLifecycleOutcome(outcome)).toBe(outcome)
    }

    const blocked = {
      status: 'blocked' as const,
      action: 'deleteConversation' as const,
      conversationId,
      revision: 5,
      code: 'CONVERSATION_LIVE_RESOURCES' as const,
      blockers: [
        { kind: 'liveBinding' as const, count: 1, ids: ['opaque/session'] },
        { kind: 'terminalResources' as const, count: 2, ids: ['terminal-1', 'terminal-2'] }
      ]
    }
    expect(parseConversationLifecycleOutcome(blocked)).toBe(blocked)

    const request = {
      schemaVersion: 1 as const,
      conversationId,
      projectAttachment: null,
      executionTarget: { kind: 'workspace' as const }
    }
    expect(parseConversationReplacementRequest(request)).toBe(request)

    const invalid: unknown[] = [
      { ...blocked, code: 'WRONG_CODE' },
      { ...blocked, blockers: [] },
      { ...blocked, blockers: [{ kind: 'liveBinding', count: 2, ids: ['one'] }] },
      { ...blocked, blockers: [{ kind: 'terminalResources', count: 1, ids: [''] }] },
      {
        ...blocked,
        blockers: [{ kind: 'terminalResources', count: 2, ids: ['same', 'same'] }]
      },
      { ...blocked, extra: true },
      {
        status: 'updated',
        action: 'detachBinding',
        conversationId,
        previousRevision: 4,
        revision: 7,
        workspaceCwd: '/visible/conversation',
        lifecycleState: 'ready',
        currentBinding: binding
      },
      {
        status: 'updated',
        action: 'deleteConversation',
        conversationId,
        previousRevision: 4,
        revision: 4,
        workspaceCwd: '/visible/conversation',
        lifecycleState: 'ready',
        currentBinding: null
      }
    ]
    for (const value of invalid) expect(() => parseConversationLifecycleOutcome(value)).toThrow()
    expect(() => parseConversationReplacementRequest({ ...request, extra: true })).toThrow()
  })
})
