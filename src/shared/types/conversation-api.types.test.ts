import { expect, it } from 'vitest'
import { parseConversationRecordV2 } from './conversation.types'
import {
  parseConversationBindingSnapshot,
  parseConversationHostStatus,
  parseConversationOpenOutcome,
  parseConversationRecordV2Array,
  parseLegacyConversationResolution
} from './conversation-api.types'

const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'

function conversation() {
  return {
    schemaVersion: 2 as const,
    conversationId,
    createdAtUtc: '2026-08-15T09:45:15.123Z',
    creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
    workspaceCwd: '/visible/conversation',
    executionTarget: { kind: 'workspace' as const },
    projectAttachment: null,
    lifecycleState: 'ready' as const,
    lastSeq: 4,
    createdBy: 'termul' as const
  }
}

function recoveryItem() {
  return {
    recoveryId: 'a'.repeat(64),
    kind: 'ambiguous_workspace_manifest' as const,
    severity: 'warning' as const,
    sourcePaths: [],
    conversationIds: [conversationId],
    sourceSha256: [],
    candidateFacts: [],
    provenance: [],
    status: 'unresolved' as const,
    suggestedActions: ['inspect'] as const,
    revision: 1,
    associationDecisions: []
  }
}

it('parses exact conversation API payloads and rejects malformed or extra keys', () => {
  const record = conversation()
  expect(parseConversationRecordV2(record)).toBe(record)
  const records = [record]
  expect(parseConversationRecordV2Array(records)).toBe(records)

  const status = {
    hostKind: 'standalone' as const,
    state: 'recovery' as const,
    code: 'CONVERSATION_RECOVERY_REQUIRED',
    migrationPhase: 'observationWindow' as const,
    readerPrecedence: 'conversationV2First' as const,
    recoveryItemCount: 1,
    recoveryItems: [recoveryItem()]
  }
  expect(parseConversationHostStatus(status)).toBe(status)

  const open = {
    conversation: record,
    workspace: {
      status: 'loaded' as const,
      workspace: {
        schemaVersion: 1 as const,
        conversationId,
        revision: 1,
        updatedAtUtc: '2026-08-15T10:00:00.000Z',
        resources: [],
        projectionState: { status: 'native' as const }
      }
    }
  }
  expect(parseConversationOpenOutcome(open)).toBe(open)

  const binding = {
    conversationId,
    binding: {
      schemaVersion: 1 as const,
      bindingId: 'b2832b54-2ca4-4db4-93fd-f93bf6793114',
      agentSessionId: 'opaque/session',
      runtimeAgentId: 'agent-runtime',
      stableAgentNamespace: 'config:test',
      executionCwd: '/visible/conversation',
      boundAtUtc: '2026-08-15T09:45:16.000Z',
      state: 'active' as const
    }
  }
  expect(parseConversationBindingSnapshot(binding)).toBe(binding)
  expect(parseConversationBindingSnapshot({ conversationId, binding: null })).toEqual({
    conversationId,
    binding: null
  })

  const legacy = { conversationId, canonicalRoute: `#/c/${conversationId}` as const }
  expect(parseLegacyConversationResolution(legacy)).toBe(legacy)

  const invalid: Array<() => unknown> = [
    () => parseConversationRecordV2({ ...record, schemaVersion: 1 }),
    () =>
      parseConversationRecordV2({
        ...record,
        creationPartition: { ...record.creationPartition, day: 14 }
      }),
    () => parseConversationRecordV2({ ...record, extra: true }),
    () => parseConversationRecordV2Array([record, { ...record, conversationId: 'foreign' }]),
    () => parseConversationHostStatus({ ...status, recoveryItemCount: 0 }),
    () => parseConversationHostStatus({ ...status, extra: true }),
    () =>
      parseConversationHostStatus({
        ...status,
        recoveryItems: [{ ...recoveryItem(), unexpected: true }]
      }),
    () =>
      parseConversationHostStatus({
        ...status,
        recoveryItems: [{ ...recoveryItem(), candidateFacts: [{ token: 'forbidden' }] }]
      }),
    () => parseConversationOpenOutcome({ ...open, extra: true }),
    () => parseConversationBindingSnapshot({ ...binding, extra: true }),
    () => parseConversationBindingSnapshot({ conversationId, binding: { extra: true } }),
    () =>
      parseConversationOpenOutcome({
        ...open,
        workspace: { status: 'missing', conversationId: '11111111-1111-4111-8111-111111111111' }
      }),
    () => parseLegacyConversationResolution({ ...legacy, canonicalRoute: '#/c/wrong' }),
    () => parseLegacyConversationResolution({ ...legacy, extra: true })
  ]
  for (const parse of invalid) expect(parse).toThrow()
})
