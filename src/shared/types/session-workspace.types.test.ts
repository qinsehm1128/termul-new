import { describe, expect, expectTypeOf, it } from 'vitest'
import type { ResolveRecoveryItemRequest } from './conversation-recovery.types'
import {
  parseSessionWorkspaceLoadOutcome,
  parseSessionWorkspaceV1,
  parseSessionWorkspaceWriteOutcome,
  SESSION_WORKSPACE_SCHEMA_VERSION,
  type SessionWorkspaceLoadOutcome,
  type SessionWorkspaceV1,
  type SessionWorkspaceWriteOutcome,
  type TerminalResourceDescriptor
} from './session-workspace.types'

const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'

function workspace(revision = 4): SessionWorkspaceV1 {
  return {
    schemaVersion: SESSION_WORKSPACE_SCHEMA_VERSION,
    conversationId,
    revision,
    updatedAtUtc: '2026-08-15T10:00:00.000Z',
    updateIdentity: 'renderer-one',
    topology: {
      type: 'leaf',
      id: 'leaf-one',
      terminalIds: ['terminal-one'],
      editorIds: ['edit-/src/main.ts'],
      activeTabId: 'term-terminal-one'
    },
    activePaneId: 'leaf-one',
    resources: [
      { kind: 'terminal', terminalId: 'terminal-one', conversationId },
      { kind: 'editor', editorId: 'edit-/src/main.ts', filePath: '/src/main.ts' }
    ],
    projectionState: { status: 'native' }
  }
}

describe('SessionWorkspace contract', () => {
  it('pins ConversationId identity, passive resources, and host revision fields', () => {
    const value = workspace()
    expect(value.conversationId).toBe(conversationId)
    expect(value.revision).toBe(4)
    expect(value.updatedAtUtc).toMatch(/\.000Z$/)
    expect(value.resources).toEqual([
      { kind: 'terminal', terminalId: 'terminal-one', conversationId },
      { kind: 'editor', editorId: 'edit-/src/main.ts', filePath: '/src/main.ts' }
    ])
    expect(JSON.stringify(value)).not.toMatch(/claim|envVars|credentials|terminalOutput|viewport/i)
  })

  it('pins load, conflict, and recovery discriminators', () => {
    const outcomes: SessionWorkspaceLoadOutcome[] = [
      { status: 'missing', conversationId },
      { status: 'loaded', workspace: workspace() },
      { status: 'recoveryRequired', conversationId, recoveryItems: [] }
    ]
    const writes: SessionWorkspaceWriteOutcome[] = [
      { status: 'updated', revision: 5, updatedAtUtc: '2026-08-15T10:00:01.000Z' },
      {
        status: 'conflict',
        currentRevision: 5,
        currentUpdatedAtUtc: '2026-08-15T10:00:01.000Z',
        currentUpdateIdentity: 'renderer-two'
      },
      { status: 'recoveryRequired', recoveryItems: [] }
    ]
    expect(outcomes.map((outcome) => outcome.status)).toEqual([
      'missing',
      'loaded',
      'recoveryRequired'
    ])
    expect(writes.map((outcome) => outcome.status)).toEqual([
      'updated',
      'conflict',
      'recoveryRequired'
    ])
  })

  it('parses exact workspace outcomes and rejects topology, identity, and raw terminal drift', () => {
    const value = workspace()
    expect(parseSessionWorkspaceV1(value)).toBe(value)
    const missing = { status: 'missing' as const, conversationId }
    expect(parseSessionWorkspaceLoadOutcome(missing)).toBe(missing)
    const loaded = { status: 'loaded' as const, workspace: value }
    expect(parseSessionWorkspaceLoadOutcome(loaded)).toBe(loaded)
    const conflict = {
      status: 'conflict' as const,
      currentRevision: 5,
      currentUpdatedAtUtc: '2026-08-15T10:00:01.000Z',
      currentUpdateIdentity: 'renderer-two'
    }
    expect(parseSessionWorkspaceWriteOutcome(conflict)).toBe(conflict)
    const recoveryRequired = { status: 'recoveryRequired' as const, recoveryItems: [] }
    expect(parseSessionWorkspaceWriteOutcome(recoveryRequired)).toBe(recoveryRequired)

    const invalid: unknown[] = [
      { ...value, extra: true },
      {
        ...value,
        topology: {
          type: 'split',
          id: 'root',
          direction: 'horizontal',
          children: [value.topology, { ...value.topology, id: 'leaf-two' }],
          sizes: [1]
        }
      },
      {
        ...value,
        topology: {
          type: 'split',
          id: 'root',
          direction: 'horizontal',
          children: [value.topology, value.topology],
          sizes: [1, 1]
        }
      },
      {
        ...value,
        resources: [
          {
            kind: 'terminal',
            terminalId: 'terminal-one',
            conversationId: '11111111-1111-4111-8111-111111111111'
          }
        ]
      },
      {
        ...value,
        resources: [
          {
            kind: 'terminal',
            terminalId: 'terminal-one',
            conversationId,
            claim: 'raw-secret'
          }
        ]
      },
      {
        ...value,
        resources: [
          {
            kind: 'terminal',
            terminalId: 'terminal-one',
            conversationId,
            envVars: { SECRET: 'value' }
          }
        ]
      },
      { status: 'conflict', currentRevision: 0, currentUpdatedAtUtc: '' },
      { status: 'updated', revision: 1, updatedAtUtc: '', extra: true },
      { status: 'recoveryRequired', recoveryItems: {}, extra: true }
    ]
    for (const candidate of invalid) {
      const parse =
        typeof candidate === 'object' && candidate !== null && 'schemaVersion' in candidate
          ? parseSessionWorkspaceV1
          : parseSessionWorkspaceWriteOutcome
      expect(() => parse(candidate)).toThrow()
    }
  })

  it('imports the exact shared recovery request contract', () => {
    expectTypeOf<ResolveRecoveryItemRequest['action']>().toEqualTypeOf<
      'inspect' | 'associateConversation' | 'startEmptyWorkspace' | 'dismissPreservedSource'
    >()
    expectTypeOf<TerminalResourceDescriptor>().not.toHaveProperty('claim')
    expectTypeOf<TerminalResourceDescriptor>().not.toHaveProperty('envVars')
    expectTypeOf<TerminalResourceDescriptor>().not.toHaveProperty('credentials')
  })
})
