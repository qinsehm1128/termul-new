import { describe, expect, it } from 'vitest'

import {
  AGENT_SESSION_BINDING_SCHEMA_VERSION,
  AGENT_SESSION_BINDING_STATES,
  type AgentSessionBinding,
  CONVERSATION_ERROR_CODES,
  CONVERSATION_LIFECYCLE_STATES,
  CONVERSATION_SCHEMA_VERSION,
  type ConversationRecordV2,
  type ExecutionTarget,
  isConversationId,
  parseAgentSessionBinding,
  parseConversationAggregateMutationOutcome,
  parseConversationId,
  parseConversationRecordV2,
  parseExecutionTarget,
  parseProjectAttachment,
  TERMINAL_RESOURCE_REF_SCHEMA_VERSION,
  type TerminalResourceRef
} from './conversation.types'

const canonicalConversationId = parseConversationId('018f7a1c-1b4d-7c8a-9f01-0123456789ab')
const opaqueAgentSessionId = 'agent/session:not-a-uuid?generation=2'

const projectlessConversation: ConversationRecordV2 = {
  schemaVersion: CONVERSATION_SCHEMA_VERSION,
  conversationId: canonicalConversationId,
  createdAtUtc: '2026-08-15T09:45:15.123Z',
  creationPartition: {
    year: 2026,
    month: 8,
    day: 15,
    path: '2026/08/15'
  },
  workspaceCwd: '/user-visible-root/sessions/2026/08/15/018f7a1c-1b4d-7c8a-9f01-0123456789ab',
  executionTarget: { kind: 'workspace' },
  projectAttachment: null,
  lifecycleState: 'allocating_workspace',
  lastSeq: 0,
  createdBy: 'termul'
}

const opaqueBinding: AgentSessionBinding = {
  schemaVersion: AGENT_SESSION_BINDING_SCHEMA_VERSION,
  bindingId: 'b2832b54-2ca4-4db4-93fd-f93bf6793114',
  agentSessionId: opaqueAgentSessionId,
  runtimeAgentId: 'agent-runtime-id',
  stableAgentNamespace: 'config:configured-agent-id',
  executionCwd: projectlessConversation.workspaceCwd,
  boundAtUtc: '2026-08-15T09:45:16.000Z',
  state: 'active'
}

describe('Conversation runtime-neutral wire contracts', () => {
  it('matches Rust canonical path parsing without UUID version restrictions', () => {
    const accepted = [
      '018f7a1c-1b4d-1c8a-1f01-0123456789ab',
      '018f7a1c-1b4d-4c8a-2f01-0123456789ab',
      '018f7a1c-1b4d-7c8a-ff01-0123456789ab'
    ]
    for (const value of accepted) {
      expect(isConversationId(value)).toBe(true)
      expect(parseConversationId(value)).toBe(value)
    }
  })

  it.each([
    '018F7A1C-1B4D-7C8A-9F01-0123456789AB',
    '018f7a1c1b4d7c8a9f010123456789ab',
    '018f7a1c-1b4d-7c8a-9f01-0123456789a',
    '018f7a1c-1b4d-7c8a-9f01-0123456789ab/child',
    ' 018f7a1c-1b4d-7c8a-9f01-0123456789ab',
    '018f7a1c-1b4d-7c8a-9f01-0123456789ab '
  ])('rejects non-canonical ConversationId %s', (value) => {
    expect(isConversationId(value)).toBe(false)
    expect(() => parseConversationId(value)).toThrow(
      'conversationId must be a canonical lowercase-hyphenated UUID'
    )
  })

  it('round-trips a project-less Conversation separately from its opaque ACP binding', () => {
    const fixture = { conversation: projectlessConversation, binding: opaqueBinding }
    const roundTripped: unknown = JSON.parse(JSON.stringify(fixture))

    expect(roundTripped).toEqual(fixture)
    expect(projectlessConversation.projectAttachment).toBeNull()
    expect(opaqueBinding.agentSessionId).toBe(opaqueAgentSessionId)
    expect(opaqueBinding.agentSessionId).not.toBe(projectlessConversation.conversationId)
    expect(opaqueBinding.agentSessionId).not.toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
    )
  })

  it('pins canonical lowercase UUID and RFC3339-millisecond UTC forms', () => {
    expect(projectlessConversation.conversationId).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
    )
    expect(projectlessConversation.createdAtUtc).toMatch(
      /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/
    )
    expect(opaqueBinding.boundAtUtc).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/)
  })

  it('keeps workspace cwd ownership separate from explicit execution target choice', () => {
    const targets: ExecutionTarget[] = [
      { kind: 'workspace' },
      {
        kind: 'project_root',
        projectId: 'project-opaque-id',
        projectRoot: '/projects/example'
      },
      {
        kind: 'worktree',
        projectId: 'project-opaque-id',
        worktreePath: '/projects/example-worktree',
        worktreeBranch: 'chat/example'
      }
    ]

    expect(targets.map((target) => target.kind)).toEqual(['workspace', 'project_root', 'worktree'])
    expect(projectlessConversation.workspaceCwd).toContain(projectlessConversation.conversationId)
    expect(projectlessConversation.executionTarget).toEqual({ kind: 'workspace' })
    expect(projectlessConversation).not.toHaveProperty('projectId')
  })

  it('pins lifecycle, binding-state, and stable error unions exactly', () => {
    expect(CONVERSATION_LIFECYCLE_STATES).toEqual([
      'allocating_workspace',
      'initializing_agent',
      'ready',
      'agent_failed',
      'recovery_required',
      'deleted'
    ])
    expect(AGENT_SESSION_BINDING_STATES).toEqual(['active', 'detached', 'suspended', 'replaced'])
    expect(CONVERSATION_ERROR_CODES).toEqual([
      'CONVERSATION_INVALID_ID',
      'CONVERSATION_INVALID_CREATED_AT',
      'CONVERSATION_UNSUPPORTED_SCHEMA',
      'CONVERSATION_NOT_FOUND',
      'CONVERSATION_CORRUPT',
      'CONVERSATION_PATH_ESCAPE',
      'CONVERSATION_SYMLINK_COMPONENT',
      'CONVERSATION_DURABILITY_FAILED',
      'CONVERSATION_CREATE_FAILED',
      'CONVERSATION_BIND_FAILED',
      'CONVERSATION_CONFLICT',
      'CONVERSATION_BINDING_NOT_FOUND',
      'CONVERSATION_BINDING_NOT_ACTIVE',
      'CONVERSATION_BINDING_NOT_DETACHED',
      'CONVERSATION_BINDING_NOT_ADDRESSABLE',
      'CONVERSATION_LIVE_RESOURCES',
      'CONVERSATION_RECOVERY_REQUIRED',
      'CONVERSATION_DURABILITY_UNSUPPORTED',
      'LEGACY_COMPATIBILITY_READ_ONLY',
      'VALIDATION_ERROR'
    ])
  })

  it('persists only a non-owning terminal reference without a raw claim', () => {
    const terminalRef: TerminalResourceRef = {
      schemaVersion: TERMINAL_RESOURCE_REF_SCHEMA_VERSION,
      terminalId: 'terminal-1'
    }

    expect(terminalRef).toEqual({ schemaVersion: 1, terminalId: 'terminal-1' })
    expect(terminalRef).not.toHaveProperty('claim')
    expect(terminalRef).not.toHaveProperty('environment')
    expect(terminalRef).not.toHaveProperty('output')
    expect(terminalRef).not.toHaveProperty('projectId')
  })

  it('contains no hidden or default project semantics', () => {
    const serialized = JSON.stringify(projectlessConversation)
    expect(serialized).not.toContain('homeProject')
    expect(serialized).not.toContain('defaultProject')
    expect(serialized).not.toContain('hiddenProject')
  })

  it('parses exact Conversation records and aggregate outcomes without accepting drift', () => {
    expect(parseConversationRecordV2(projectlessConversation)).toBe(projectlessConversation)
    expect(parseAgentSessionBinding(opaqueBinding)).toBe(opaqueBinding)
    const attachment = {
      schemaVersion: 1 as const,
      projectId: 'project-1',
      attachedAtUtc: '2026-08-15T10:00:00.000Z',
      projectPathSnapshot: '/projects/termul',
      worktreePath: null,
      worktreeBranch: null
    }
    expect(parseProjectAttachment(attachment)).toBe(attachment)
    expect(parseExecutionTarget({ kind: 'workspace' })).toEqual({ kind: 'workspace' })

    const identity = {
      conversationId: canonicalConversationId,
      createdAtUtc: projectlessConversation.createdAtUtc,
      creationPartition: projectlessConversation.creationPartition,
      workspaceCwd: projectlessConversation.workspaceCwd
    }
    const aggregate = {
      status: 'updated' as const,
      action: 'attachProject' as const,
      conversationId: canonicalConversationId,
      previousRevision: 0,
      revision: 1,
      identityBefore: identity,
      identityAfter: { ...identity },
      projectAttachment: attachment,
      executionTarget: { kind: 'workspace' as const },
      conversation: {
        ...projectlessConversation,
        projectAttachment: attachment,
        lifecycleState: 'ready' as const,
        lastSeq: 1
      }
    }
    expect(parseConversationAggregateMutationOutcome(aggregate)).toBe(aggregate)

    const invalidRecords: unknown[] = [
      { ...projectlessConversation, schemaVersion: 1 },
      { ...projectlessConversation, conversationId: 'not-a-uuid' },
      { ...projectlessConversation, createdAtUtc: '2026-08-15T09:45:15Z' },
      {
        ...projectlessConversation,
        creationPartition: { ...projectlessConversation.creationPartition, path: '2026/08/14' }
      },
      { ...projectlessConversation, lifecycleState: 'unknown' },
      { ...projectlessConversation, lastSeq: -1 },
      { ...projectlessConversation, projectAttachment: { ...attachment, extra: true } },
      { ...projectlessConversation, executionTarget: { kind: 'workspace', projectId: 'hidden' } },
      { ...projectlessConversation, extra: true }
    ]
    for (const value of invalidRecords) expect(() => parseConversationRecordV2(value)).toThrow()

    expect(() =>
      parseConversationAggregateMutationOutcome({
        ...aggregate,
        identityAfter: { ...identity, workspaceCwd: '/changed' }
      })
    ).toThrow(/identity/)
    expect(() =>
      parseConversationAggregateMutationOutcome({
        ...aggregate,
        conversation: { ...aggregate.conversation, lastSeq: 2 }
      })
    ).toThrow()
    expect(() => parseConversationAggregateMutationOutcome({ ...aggregate, extra: true })).toThrow()
  })
})
