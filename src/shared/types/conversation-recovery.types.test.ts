import { describe, expect, expectTypeOf, it } from 'vitest'

import {
  parseRecoveryActionResult,
  parseRecoveryItemV1,
  parseResolveRecoveryItemRequest,
  RECOVERY_ACTION_FIXTURES,
  RECOVERY_ACTION_FIXTURES_JSON,
  type RecoveryAction,
  type RecoveryActionResult,
  type RecoveryItemV1
} from './conversation-recovery.types'

describe('canonical RecoveryAction contract', () => {
  it('round-trips the exact four camelCase fixtures and pins effects', () => {
    expect(RECOVERY_ACTION_FIXTURES.map((fixture) => fixture.request.action)).toEqual([
      'inspect',
      'associateConversation',
      'startEmptyWorkspace',
      'dismissPreservedSource'
    ])

    expect(JSON.parse(RECOVERY_ACTION_FIXTURES_JSON)).toEqual(RECOVERY_ACTION_FIXTURES)

    for (const fixture of RECOVERY_ACTION_FIXTURES) {
      expect(parseResolveRecoveryItemRequest(structuredClone(fixture.request))).toEqual(
        fixture.request
      )
    }

    expect(RECOVERY_ACTION_FIXTURES.map((fixture) => fixture.authorization)).toEqual([
      'read',
      'mutation',
      'mutation',
      'mutation'
    ])
    expect(
      RECOVERY_ACTION_FIXTURES.map(({ result }) => ({
        action: result.action,
        authorization: result.authorization,
        status: result.status,
        recoveryRevision: result.recoveryRevision,
        workspaceRevision: result.workspaceRevision,
        workspaceChanged: result.workspaceChanged
      }))
    ).toEqual([
      {
        action: 'inspect',
        authorization: 'read',
        status: 'unresolved',
        recoveryRevision: 1,
        workspaceRevision: null,
        workspaceChanged: false
      },
      {
        action: 'associateConversation',
        authorization: 'mutation',
        status: 'resolvedAssociated',
        recoveryRevision: 3,
        workspaceRevision: null,
        workspaceChanged: false
      },
      {
        action: 'startEmptyWorkspace',
        authorization: 'mutation',
        status: 'resolvedStartedEmpty',
        recoveryRevision: 4,
        workspaceRevision: 1,
        workspaceChanged: true
      },
      {
        action: 'dismissPreservedSource',
        authorization: 'mutation',
        status: 'dismissedPreserved',
        recoveryRevision: 5,
        workspaceRevision: null,
        workspaceChanged: false
      }
    ])
    for (const { result } of RECOVERY_ACTION_FIXTURES) {
      expect(result.sourcePaths).toEqual(['legacy_workspace_manifests/0/project.json'])
      expect(result.sourceSha256).toEqual(['e'.repeat(64)])
      expect(result.candidateFacts).toEqual([{ candidate: 'preserved' }])
      expect(result.provenance).toEqual([
        {
          sourceKind: 'legacy_workspace_manifests',
          relativePath: 'legacy_workspace_manifests/0/project.json',
          sha256: 'e'.repeat(64),
          preservedReadOnly: true
        }
      ])
    }
  })

  it.each([
    '00000000-0000-0000-0000-000000000000',
    '018f7a1c-1b4d-1c8a-1f01-0123456789ab',
    '018f7a1c-1b4d-4c8a-2f01-0123456789ab',
    '018f7a1c-1b4d-7c8a-ff01-0123456789ab'
  ])('accepts Rust-parity canonical ConversationId fixture %s', (conversationId) => {
    expect(
      parseResolveRecoveryItemRequest({
        recoveryId: 'a'.repeat(64),
        expectedRevision: 1,
        idempotencyKey: '21aee10a-56b8-4624-a5e7-586c25dc8d1f',
        action: 'associateConversation',
        payload: { conversationId }
      })
    ).toMatchObject({ payload: { conversationId } })
  })

  it.each([
    '018F7A1C-1B4D-7C8A-9F01-0123456789AB',
    '{018f7a1c-1b4d-7c8a-9f01-0123456789ab}',
    '018f7a1c1b4d7c8a9f010123456789ab',
    '018f7a1c-1b4d-7c8a-9f01-0123456789ab/../escape',
    '../018f7a1c-1b4d-7c8a-9f01-0123456789ab',
    'not-a-uuid'
  ])('rejects non-canonical recovery ConversationId %s', (conversationId) => {
    expect(() =>
      parseResolveRecoveryItemRequest({
        recoveryId: 'a'.repeat(64),
        expectedRevision: 1,
        idempotencyKey: '21aee10a-56b8-4624-a5e7-586c25dc8d1f',
        action: 'associateConversation',
        payload: { conversationId }
      })
    ).toThrow('conversationId must be a canonical lowercase-hyphenated UUID')
  })

  it('keeps the intentionally version-restricted idempotency-key validator separate', () => {
    expect(() =>
      parseResolveRecoveryItemRequest({
        recoveryId: 'a'.repeat(64),
        expectedRevision: 1,
        idempotencyKey: '00000000-0000-0000-0000-000000000000',
        action: 'associateConversation',
        payload: { conversationId: '00000000-0000-0000-0000-000000000000' }
      })
    ).toThrow('idempotencyKey')
  })

  it('rejects snake_case, aliases, missing revision/idempotency, bad UUIDs, and payload drift', () => {
    const invalid: unknown[] = [
      {
        recovery_id: 'a'.repeat(64),
        expectedRevision: 1,
        action: 'inspect',
        payload: {}
      },
      { recoveryId: 'a'.repeat(64), action: 'inspect', payload: {} },
      {
        recoveryId: 'a'.repeat(64),
        expectedRevision: 1,
        idempotencyKey: '21aee10a-56b8-4624-a5e7-586c25dc8d1f',
        action: 'associate_conversation',
        payload: { conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab' }
      },
      {
        recoveryId: 'a'.repeat(64),
        expectedRevision: 1,
        action: 'associateConversation',
        payload: { conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab' }
      },
      {
        recoveryId: 'a'.repeat(64),
        expectedRevision: 1,
        idempotencyKey: 'not-a-uuid',
        action: 'dismissPreservedSource',
        payload: { reasonCode: 'notApplicable' }
      },
      {
        recoveryId: 'a'.repeat(64),
        expectedRevision: 1,
        idempotencyKey: '21AEE10A-56B8-4624-A5E7-586C25DC8D1F',
        action: 'dismissPreservedSource',
        payload: { reasonCode: 'notApplicable' }
      },
      {
        recoveryId: 'a'.repeat(64),
        expectedRevision: 1,
        idempotencyKey: '21aee10a-56b8-4624-a5e7-586c25dc8d1f',
        action: 'startEmptyWorkspace',
        payload: {
          conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
          expected_workspace_revision: null
        }
      },
      {
        recoveryId: 'a'.repeat(64),
        expectedRevision: 1,
        action: 'inspect',
        payload: { unexpected: true }
      },
      {
        recoveryId: 'a'.repeat(64),
        expectedRevision: 1,
        action: 'inspect',
        payload: {},
        sourcePaths: ['attempted-provenance-mutation']
      }
    ]

    for (const value of invalid) expect(() => parseResolveRecoveryItemRequest(value)).toThrow()
  })

  it('parses exact recovery results and rejects mutable or sensitive evidence fields', () => {
    for (const fixture of RECOVERY_ACTION_FIXTURES) {
      expect(parseRecoveryActionResult(fixture.result)).toBe(fixture.result)
    }
    const item = {
      recoveryId: 'a'.repeat(64),
      kind: 'ambiguous_workspace_manifest' as const,
      severity: 'warning' as const,
      sourcePaths: ['legacy_workspace_manifests/0/project.json'],
      conversationIds: ['018f7a1c-1b4d-7c8a-9f01-0123456789ab'],
      sourceSha256: ['e'.repeat(64)],
      candidateFacts: [{ candidate: 'preserved' }],
      provenance: [
        {
          sourceKind: 'legacy_workspace_manifests',
          relativePath: 'legacy_workspace_manifests/0/project.json',
          sha256: 'e'.repeat(64),
          preservedReadOnly: true as const
        }
      ],
      status: 'unresolved' as const,
      suggestedActions: ['inspect'] as const,
      revision: 1,
      associationDecisions: []
    }
    expect(parseRecoveryItemV1(item)).toBe(item)

    const inspected = RECOVERY_ACTION_FIXTURES[0].result
    const invalidResults: unknown[] = [
      { ...inspected, extra: true },
      { ...inspected, authorization: 'mutation' },
      { ...inspected, status: 'resolvedAssociated' },
      { ...inspected, workspaceChanged: true },
      { ...inspected, sourceSha256: ['not-a-digest'] },
      { ...inspected, candidateFacts: [{ claim: 'raw-secret' }] },
      { ...inspected, candidateFacts: [{ nested: { environment: { SECRET: 'value' } } }] },
      {
        ...inspected,
        provenance: [{ ...inspected.provenance[0], unknownEvidence: true }]
      },
      {
        ...inspected,
        provenance: [{ ...inspected.provenance[0], preservedReadOnly: false }]
      }
    ]
    for (const value of invalidResults) expect(() => parseRecoveryActionResult(value)).toThrow()
    expect(() => parseRecoveryItemV1({ ...item, unexpected: true })).toThrow()
    expect(() => parseRecoveryItemV1({ ...item, candidateFacts: [{ token: 'secret' }] })).toThrow()
  })

  it('keeps source provenance readonly in items and results', () => {
    expectTypeOf<RecoveryItemV1['sourcePaths']>().toEqualTypeOf<readonly string[]>()
    expectTypeOf<RecoveryItemV1['sourceSha256']>().toEqualTypeOf<readonly string[]>()
    expectTypeOf<RecoveryItemV1['provenance']>().toEqualTypeOf<
      readonly {
        readonly sourceKind: string
        readonly relativePath: string
        readonly sha256: string
        readonly preservedReadOnly: true
      }[]
    >()
    expectTypeOf<RecoveryActionResult['candidateFacts']>().toEqualTypeOf<
      readonly Readonly<Record<string, unknown>>[]
    >()
    expectTypeOf<RecoveryAction>().toMatchTypeOf<
      | { action: 'inspect'; expectedRevision: number }
      | { action: 'associateConversation'; expectedRevision: number; idempotencyKey: string }
      | { action: 'startEmptyWorkspace'; expectedRevision: number; idempotencyKey: string }
      | { action: 'dismissPreservedSource'; expectedRevision: number; idempotencyKey: string }
    >()
  })
})
