import { beforeEach, describe, expect, it, vi } from 'vitest'

const prepareTerminalConversation = vi.fn()
const spawnTerminalInPane = vi.fn()

vi.mock('@/lib/conversation-api', () => ({
  conversationApi: {
    prepareTerminalConversation: (...args: unknown[]) => prepareTerminalConversation(...args)
  }
}))
vi.mock('@/lib/terminal-spawn', () => ({
  spawnTerminalInPane: (...args: unknown[]) => spawnTerminalInPane(...args)
}))
vi.mock('@/i18n/runtime', () => ({
  runtimeT: (_ns: string, _key: string, fallback: string) => fallback
}))

import { launchTerminalConversation } from './terminal-conversation-launch'

const CONVERSATION_ID = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'

function preparedOk(): void {
  prepareTerminalConversation.mockResolvedValue({
    success: true,
    data: {
      schemaVersion: 1,
      conversationId: CONVERSATION_ID,
      createdAtUtc: '2026-09-08T00:00:00.000Z',
      creationPartition: { year: 2026, month: 9, day: 8, path: '2026/09/08' },
      workspaceCwd: '/sessions/2026/09/08/x',
      executionCwd: '/sessions/2026/09/08/x',
      lifecycleState: 'allocating_workspace'
    }
  })
}

const input = { executionTarget: { kind: 'workspace' } as const }

describe('launchTerminalConversation', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('prepares the Conversation and reports its id', async () => {
    preparedOk()

    const result = await launchTerminalConversation(input)

    expect(result).toEqual({ success: true, conversationId: CONVERSATION_ID })
  })

  it('never spawns a terminal', async () => {
    // The pane on screen belongs to the workspace activation is about to
    // replace. A terminal spawned here lands in the *previous* project's tab
    // bar and never appears in the Conversation the user just created — the
    // exact bug this boundary exists to prevent.
    preparedOk()

    await launchTerminalConversation(input)

    expect(spawnTerminalInPane).not.toHaveBeenCalled()
  })

  it('reports the failure when preparation fails', async () => {
    prepareTerminalConversation.mockResolvedValue({
      success: false,
      code: 'CONVERSATION_CREATE_FAILED',
      error: 'no room'
    })

    const result = await launchTerminalConversation(input)

    expect(result.success).toBe(false)
    expect(result.error).toBe('no room')
    expect(result.conversationId).toBeUndefined()
  })

  it('falls back to a readable message when the host sends none', async () => {
    prepareTerminalConversation.mockResolvedValue({
      success: false,
      code: 'CONVERSATION_CREATE_FAILED'
    })

    const result = await launchTerminalConversation(input)

    expect(result.success).toBe(false)
    expect(result.error).toBe('Failed to create the conversation')
  })

  it('sends an explicit null attachment rather than omitting it', async () => {
    // `deny_unknown_fields` on the host accepts an absent optional, but an
    // explicit null keeps the wire shape identical on both transports.
    preparedOk()

    await launchTerminalConversation(input)

    expect(prepareTerminalConversation).toHaveBeenCalledWith(
      expect.objectContaining({ projectAttachment: null, schemaVersion: 1 })
    )
  })

  it('passes the project attachment through', async () => {
    preparedOk()
    const attachment = {
      schemaVersion: 1 as const,
      projectId: 'project-1',
      attachedAtUtc: '2026-09-08T00:00:00.000Z',
      projectPathSnapshot: '/projects/p',
      worktreePath: null,
      worktreeBranch: null
    }

    await launchTerminalConversation({ ...input, projectAttachment: attachment })

    expect(prepareTerminalConversation).toHaveBeenCalledWith(
      expect.objectContaining({ projectAttachment: attachment })
    )
  })
})
