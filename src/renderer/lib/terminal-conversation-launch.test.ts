import { beforeEach, describe, expect, it, vi } from 'vitest'

const prepareTerminalConversation = vi.fn()
const provisionTerminalConversation = vi.fn()
const spawnTerminalInPane = vi.fn()
const logFrontendError = vi.fn()

vi.mock('@/lib/conversation-api', () => ({
  conversationApi: {
    prepareTerminalConversation: (...args: unknown[]) => prepareTerminalConversation(...args),
    provisionTerminalConversation: (...args: unknown[]) => provisionTerminalConversation(...args)
  }
}))
vi.mock('@/lib/terminal-spawn', () => ({
  spawnTerminalInPane: (...args: unknown[]) => spawnTerminalInPane(...args)
}))
vi.mock('@/lib/log-api', () => ({ logFrontendError: (...a: unknown[]) => logFrontendError(...a) }))
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

const input = { paneId: 'pane-1', executionTarget: { kind: 'workspace' } as const }

describe('launchTerminalConversation', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('prepares, spawns into the prepared cwd, then provisions — in that order', async () => {
    const calls: string[] = []
    prepareTerminalConversation.mockImplementation(async () => {
      calls.push('prepare')
      return {
        success: true,
        data: {
          schemaVersion: 1,
          conversationId: CONVERSATION_ID,
          createdAtUtc: '2026-09-08T00:00:00.000Z',
          creationPartition: { year: 2026, month: 9, day: 8, path: '2026/09/08' },
          workspaceCwd: '/sessions/x',
          executionCwd: '/sessions/x',
          lifecycleState: 'allocating_workspace'
        }
      }
    })
    spawnTerminalInPane.mockImplementation(async () => {
      calls.push('spawn')
      return { success: true, terminalId: 'terminal-1' }
    })
    provisionTerminalConversation.mockImplementation(async () => {
      calls.push('provision')
      return { success: true, data: null }
    })

    const result = await launchTerminalConversation(input)

    expect(result).toEqual({
      success: true,
      conversationId: CONVERSATION_ID,
      terminalId: 'terminal-1'
    })
    // Provisioning before the terminal exists would record a backend for a
    // terminal that may never start.
    expect(calls).toEqual(['prepare', 'spawn', 'provision'])
    // The terminal must land in the Conversation's own directory, and must
    // carry the conversation id so the host binds it as a durable resource.
    expect(spawnTerminalInPane).toHaveBeenCalledWith(
      'pane-1',
      '',
      '/sessions/x',
      expect.objectContaining({ conversationId: CONVERSATION_ID })
    )
  })

  it('never spawns when preparation fails', async () => {
    prepareTerminalConversation.mockResolvedValue({
      success: false,
      code: 'CONVERSATION_CREATE_FAILED',
      error: 'no room'
    })

    const result = await launchTerminalConversation(input)

    expect(result.success).toBe(false)
    expect(result.error).toBe('no room')
    expect(spawnTerminalInPane).not.toHaveBeenCalled()
    expect(provisionTerminalConversation).not.toHaveBeenCalled()
  })

  it('does not provision when the terminal failed to spawn', async () => {
    // Provisioning here would carry the Conversation to `ready` with nothing
    // behind it — exactly the state the lifecycle is designed to prevent.
    preparedOk()
    spawnTerminalInPane.mockResolvedValue({ success: false, error: 'limit reached' })

    const result = await launchTerminalConversation(input)

    expect(result.success).toBe(false)
    expect(result.error).toBe('limit reached')
    expect(provisionTerminalConversation).not.toHaveBeenCalled()
    // The caller is told which Conversation was left behind rather than losing it.
    expect(result.conversationId).toBe(CONVERSATION_ID)
  })

  it('keeps the terminal alive when provisioning fails', async () => {
    // The terminal is real and visible. Killing it to tidy up a durable record
    // would destroy work the user can see.
    preparedOk()
    spawnTerminalInPane.mockResolvedValue({ success: true, terminalId: 'terminal-1' })
    provisionTerminalConversation.mockResolvedValue({
      success: false,
      code: 'VALIDATION_ERROR',
      error: 'already agent-backed'
    })

    const result = await launchTerminalConversation(input)

    expect(result.success).toBe(false)
    expect(result.error).toBe('already agent-backed')
    expect(result.conversationId).toBe(CONVERSATION_ID)
    expect(logFrontendError).toHaveBeenCalledWith(
      expect.objectContaining({ source: 'terminal-conversation.provision' })
    )
  })

  it('passes the project attachment and terminal limit through', async () => {
    preparedOk()
    spawnTerminalInPane.mockResolvedValue({ success: true, terminalId: 'terminal-1' })
    provisionTerminalConversation.mockResolvedValue({ success: true, data: null })

    const attachment = {
      schemaVersion: 1 as const,
      projectId: 'project-1',
      attachedAtUtc: '2026-09-08T00:00:00.000Z',
      projectPathSnapshot: '/projects/p',
      worktreePath: null,
      worktreeBranch: null
    }
    await launchTerminalConversation({
      ...input,
      projectId: 'project-1',
      projectAttachment: attachment,
      maxTerminalsPerProject: 4
    })

    expect(prepareTerminalConversation).toHaveBeenCalledWith(
      expect.objectContaining({ projectAttachment: attachment })
    )
    expect(spawnTerminalInPane).toHaveBeenCalledWith(
      'pane-1',
      'project-1',
      expect.any(String),
      expect.objectContaining({ maxTerminalsPerProject: 4 })
    )
  })

  it('sends an explicit null attachment rather than omitting it', async () => {
    // `deny_unknown_fields` on the host accepts an absent optional, but an
    // explicit null keeps the wire shape identical on both transports.
    preparedOk()
    spawnTerminalInPane.mockResolvedValue({ success: true, terminalId: 'terminal-1' })
    provisionTerminalConversation.mockResolvedValue({ success: true, data: null })

    await launchTerminalConversation(input)

    expect(prepareTerminalConversation).toHaveBeenCalledWith(
      expect.objectContaining({ projectAttachment: null, schemaVersion: 1 })
    )
  })
})
