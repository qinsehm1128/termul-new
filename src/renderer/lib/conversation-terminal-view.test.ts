import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Terminal } from '@/types/project'

const spawnTerminalInPane = vi.fn()
const provisionTerminalConversation = vi.fn()
const reopenTerminalView = vi.fn()
const toastError = vi.fn()
const logFrontendError = vi.fn()

vi.mock('@/lib/terminal-spawn', () => ({
  spawnTerminalInPane: (...args: unknown[]) => spawnTerminalInPane(...args)
}))
vi.mock('@/lib/conversation-api', () => ({
  conversationApi: {
    provisionTerminalConversation: (...args: unknown[]) => provisionTerminalConversation(...args)
  }
}))
vi.mock('@/lib/log-api', () => ({ logFrontendError: (...a: unknown[]) => logFrontendError(...a) }))
vi.mock('sonner', () => ({ toast: { error: (...a: unknown[]) => toastError(...a) } }))
vi.mock('@/i18n/runtime', () => ({
  runtimeT: (_ns: string, _key: string, fallback: string) => fallback
}))
vi.mock('@/stores/app-settings-store', () => ({
  useAppSettingsStore: { getState: () => ({ settings: { maxTerminalsPerProject: 8 } }) }
}))

const terminalState = { terminals: [] as Terminal[] }
vi.mock('@/stores/terminal-store', () => ({
  useTerminalStore: { getState: () => terminalState }
}))

const workspaceState = {
  root: { type: 'leaf', id: 'pane-1', tabs: [] as Array<{ type: string; terminalId?: string }> },
  activePaneId: 'pane-1' as string | null,
  reopenTerminalView: (...args: unknown[]) => reopenTerminalView(...args)
}
vi.mock('@/stores/workspace-store', () => ({
  useWorkspaceStore: { getState: () => workspaceState },
  getAllLeafPanes: (root: { type: string }) => (root.type === 'leaf' ? [root] : [])
}))

const conversationState = {
  summariesById: {} as Record<string, unknown>,
  detailsById: {} as Record<string, unknown>
}
vi.mock('@/stores/conversation-store', () => ({
  useConversationStore: { getState: () => conversationState }
}))

const adoptHostWorkspaceRevision = vi.fn()
vi.mock('@/hooks/use-session-workspace-sync', () => ({
  adoptHostWorkspaceRevision: (...args: unknown[]) => adoptHostWorkspaceRevision(...args)
}))

import {
  closingKeepsProcessAlive,
  conversationTerminalCwd,
  ensureConversationTerminal,
  isConversationBackendTerminal,
  terminalCloseIntent
} from './conversation-terminal-view'

const CONVERSATION_ID = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'

function terminal(overrides: Partial<Terminal> = {}): Terminal {
  return {
    id: 'terminal-1',
    name: 'Terminal 1',
    shell: 'zsh',
    conversationId: CONVERSATION_ID,
    ptyId: 'pty-1',
    healthStatus: 'running',
    viewState: 'visible',
    ...overrides
  } as Terminal
}

function seedConversation(executionTarget: unknown, workspaceCwd = '/sessions/2026/09/08/x'): void {
  conversationState.summariesById[CONVERSATION_ID] = {
    conversationId: CONVERSATION_ID,
    workspaceCwd,
    executionTarget,
    backend: 'terminal'
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  terminalState.terminals = []
  workspaceState.root = { type: 'leaf', id: 'pane-1', tabs: [] }
  workspaceState.activePaneId = 'pane-1'
  conversationState.summariesById = {}
  conversationState.detailsById = {}
  spawnTerminalInPane.mockResolvedValue({ success: true, terminalId: 'terminal-9' })
  provisionTerminalConversation.mockResolvedValue({ success: true, data: null })
})

describe('conversationTerminalCwd', () => {
  it('uses the Conversation folder for a project-root target', () => {
    // The target widens which other roots are reachable; it does not move the
    // Conversation. Opening the project root instead makes the terminal
    // indistinguishable from an ordinary project shell, and reopening the
    // Conversation stops returning the user to where its work was.
    expect(
      conversationTerminalCwd({
        executionTarget: { kind: 'project_root', projectId: 'p1', projectRoot: '/repo' },
        workspaceCwd: '/sessions/x'
      })
    ).toBe('/sessions/x')
  })

  it('uses the Conversation folder for a worktree target', () => {
    expect(
      conversationTerminalCwd({
        executionTarget: {
          kind: 'worktree',
          projectId: 'p1',
          worktreePath: '/repo/.worktrees/feat',
          worktreeBranch: 'feat'
        },
        workspaceCwd: '/sessions/x'
      })
    ).toBe('/sessions/x')
  })

  it('uses the Conversation folder for a workspace target', () => {
    expect(
      conversationTerminalCwd({
        executionTarget: { kind: 'workspace' },
        workspaceCwd: '/sessions/x'
      })
    ).toBe('/sessions/x')
  })
})

describe('ensureConversationTerminal', () => {
  it('spawns in the Conversation folder and records the terminal', async () => {
    seedConversation({ kind: 'project_root', projectId: 'p1', projectRoot: '/repo' })

    const outcome = await ensureConversationTerminal(CONVERSATION_ID)

    expect(outcome).toBe('spawned')
    // Folder from the Conversation, project attribution from the target: the
    // terminal belongs to the project's group but runs in its own directory.
    expect(spawnTerminalInPane).toHaveBeenCalledWith(
      'pane-1',
      'p1',
      '/sessions/2026/09/08/x',
      expect.objectContaining({ conversationId: CONVERSATION_ID, maxTerminalsPerProject: 8 })
    )
    expect(provisionTerminalConversation).toHaveBeenCalledWith(CONVERSATION_ID, 'terminal-9')
  })

  it('takes the revision the host bumped when it registered the terminal', async () => {
    // Spawning registers the terminal as a Conversation resource host-side,
    // which bumps the workspace revision. Leaving the renderer on its older
    // revision makes the next auto-save collide and shows the user a
    // "workspace changed elsewhere" banner about their own click.
    seedConversation({ kind: 'workspace' })

    await ensureConversationTerminal(CONVERSATION_ID)

    expect(adoptHostWorkspaceRevision).toHaveBeenCalledWith(CONVERSATION_ID)
  })

  it('does not touch the revision when nothing was spawned', async () => {
    seedConversation({ kind: 'workspace' })
    terminalState.terminals = [terminal({ viewState: 'hidden', isHidden: true })]

    await ensureConversationTerminal(CONVERSATION_ID)

    expect(adoptHostWorkspaceRevision).not.toHaveBeenCalled()
  })

  it('does nothing when the Conversation already shows a terminal', async () => {
    seedConversation({ kind: 'workspace' })
    terminalState.terminals = [terminal()]
    workspaceState.root = {
      type: 'leaf',
      id: 'pane-1',
      tabs: [{ type: 'terminal', terminalId: 'terminal-1' }]
    }

    const outcome = await ensureConversationTerminal(CONVERSATION_ID)

    expect(outcome).toBe('already-open')
    expect(spawnTerminalInPane).not.toHaveBeenCalled()
  })

  it('opens the terminal when its record survived but its tab did not', async () => {
    // The manifest restores terminal *records*; the topology restored alongside
    // them can carry no tab for any of them — which is exactly the state every
    // Conversation created before this fix is in. A record with no tab is
    // invisible, and the empty pane renders the agent launcher, so treating
    // "record exists" as "already on screen" is what sent users back to the
    // launcher when they clicked their own terminal.
    seedConversation({ kind: 'workspace' })
    terminalState.terminals = [terminal()]
    workspaceState.root = { type: 'leaf', id: 'pane-1', tabs: [] }

    const outcome = await ensureConversationTerminal(CONVERSATION_ID)

    expect(outcome).toBe('reattached')
    expect(reopenTerminalView).toHaveBeenCalledWith('terminal-1')
  })

  it('reattaches a live terminal instead of spawning a second one', async () => {
    // The running process holds the user's scrollback and whatever command is
    // mid-flight. Spawning a replacement would strand both.
    seedConversation({ kind: 'workspace' })
    terminalState.terminals = [terminal({ viewState: 'hidden', isHidden: true })]

    const outcome = await ensureConversationTerminal(CONVERSATION_ID)

    expect(outcome).toBe('reattached')
    expect(reopenTerminalView).toHaveBeenCalledWith('terminal-1')
    expect(spawnTerminalInPane).not.toHaveBeenCalled()
  })

  it('spawns a replacement when the previous terminal exited', async () => {
    seedConversation({ kind: 'workspace' })
    terminalState.terminals = [
      terminal({ viewState: 'hidden', isHidden: true, healthStatus: 'exited' })
    ]

    const outcome = await ensureConversationTerminal(CONVERSATION_ID)

    expect(outcome).toBe('spawned')
    expect(reopenTerminalView).not.toHaveBeenCalled()
    expect(spawnTerminalInPane).toHaveBeenCalled()
  })

  it('ignores terminals belonging to other Conversations', async () => {
    seedConversation({ kind: 'workspace' })
    terminalState.terminals = [terminal({ id: 'other', conversationId: 'someone-else' })]

    const outcome = await ensureConversationTerminal(CONVERSATION_ID)

    expect(outcome).toBe('spawned')
    expect(reopenTerminalView).not.toHaveBeenCalled()
  })

  it('surfaces a spawn failure instead of leaving an empty Conversation', async () => {
    // Silence here reads as "the Conversation is empty", which is the one
    // outcome the user cannot act on. The per-project limit in particular is
    // only actionable if they see it.
    seedConversation({ kind: 'workspace' })
    spawnTerminalInPane.mockResolvedValue({ success: false, error: 'limit reached' })

    const outcome = await ensureConversationTerminal(CONVERSATION_ID)

    expect(outcome).toBe('failed')
    expect(toastError).toHaveBeenCalledWith('limit reached')
    expect(provisionTerminalConversation).not.toHaveBeenCalled()
  })

  it('keeps the terminal when recording it fails', async () => {
    seedConversation({ kind: 'workspace' })
    provisionTerminalConversation.mockResolvedValue({
      success: false,
      code: 'VALIDATION_ERROR',
      error: 'already agent-backed'
    })

    const outcome = await ensureConversationTerminal(CONVERSATION_ID)

    expect(outcome).toBe('spawned')
    expect(logFrontendError).toHaveBeenCalledWith(
      expect.objectContaining({ source: 'conversation-terminal-view.provision' })
    )
  })

  it('does not spawn into a workspace the user has already left', async () => {
    // Activation is racy by nature: clicking a second Conversation while the
    // first is still resolving must not drop a terminal into the wrong one.
    seedConversation({ kind: 'workspace' })

    const outcome = await ensureConversationTerminal(CONVERSATION_ID, () => false)

    expect(outcome).toBe('stale')
    expect(spawnTerminalInPane).not.toHaveBeenCalled()
  })

  it('refuses to spawn when the Conversation is unknown', async () => {
    const outcome = await ensureConversationTerminal(CONVERSATION_ID)

    expect(outcome).toBe('failed')
    expect(spawnTerminalInPane).not.toHaveBeenCalled()
  })
})

describe('closingKeepsProcessAlive', () => {
  it('is false for a project shell', () => {
    // Nothing else owns it, so a tab close that left it running would leak it.
    expect(closingKeepsProcessAlive({ conversationId: undefined })).toBe(false)
  })

  it('is true for a terminal an agent opened', () => {
    conversationState.summariesById[CONVERSATION_ID] = { backend: 'agent' }
    expect(closingKeepsProcessAlive({ conversationId: CONVERSATION_ID })).toBe(true)
  })

  it('is false for the terminal that *is* the Conversation', () => {
    // Keeping it alive gives the user a shell with no tab, no listing and no
    // way back — which is what made these feel unkillable.
    seedConversation({ kind: 'workspace' })
    expect(closingKeepsProcessAlive({ conversationId: CONVERSATION_ID })).toBe(false)
    expect(isConversationBackendTerminal({ conversationId: CONVERSATION_ID })).toBe(true)
  })

  it('treats a Conversation with no recorded backend as agent-backed', () => {
    conversationState.summariesById[CONVERSATION_ID] = { conversationId: CONVERSATION_ID }
    expect(closingKeepsProcessAlive({ conversationId: CONVERSATION_ID })).toBe(true)
  })
})

describe('terminalCloseIntent', () => {
  it('asks before killing by default', () => {
    // Two separate settings on purpose: the view-close opt-out promises the
    // process keeps running, so it must not silence the prompt for a kill.
    expect(terminalCloseIntent({ conversationId: undefined }, false, true)).toBe(
      'terminate-confirm'
    )
    expect(terminalCloseIntent({ conversationId: undefined }, true, true)).toBe('terminate-confirm')
  })

  it('kills without asking once the user turns that confirmation off', () => {
    // The opt-out is about the kill itself, not inherited from the view-close
    // setting — note the view-close flag is left ON here.
    expect(terminalCloseIntent({ conversationId: undefined }, true, false)).toBe('terminate')
  })

  it('asks before killing the terminal that is the Conversation', () => {
    seedConversation({ kind: 'workspace' })
    expect(terminalCloseIntent({ conversationId: CONVERSATION_ID }, false, true)).toBe(
      'terminate-confirm'
    )
    expect(terminalCloseIntent({ conversationId: CONVERSATION_ID }, false, false)).toBe('terminate')
  })

  it('honours the view-close setting for a close that keeps the process', () => {
    conversationState.summariesById[CONVERSATION_ID] = { backend: 'agent' }
    expect(terminalCloseIntent({ conversationId: CONVERSATION_ID }, true, true)).toBe(
      'close-view-confirm'
    )
    expect(terminalCloseIntent({ conversationId: CONVERSATION_ID }, false, true)).toBe('close-view')
  })

  it('never kills a view close, whatever the terminate setting says', () => {
    // The kill opt-out must not leak into the branch that only hides a tab.
    conversationState.summariesById[CONVERSATION_ID] = { backend: 'agent' }
    expect(terminalCloseIntent({ conversationId: CONVERSATION_ID }, true, false)).toBe(
      'close-view-confirm'
    )
  })

  it('asks before killing a terminal whose record has already gone', () => {
    // No record means nothing proves an agent owns it; killing is the branch
    // that must not happen silently.
    expect(terminalCloseIntent(undefined, false, true)).toBe('terminate-confirm')
  })
})
