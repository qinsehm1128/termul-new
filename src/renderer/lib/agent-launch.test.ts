/**
 * Unit tests for launchAgentInPane orchestration (ADR-004.4).
 *
 * Verifies the agent is spawned with program/args/kind:'agent', the prompt is
 * passed as a discrete arg (never interpolated), the terminal is tagged with
 * descriptive agent metadata, and the seed prompt is NOT stored on the record.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'

const {
  mockSetTerminals,
  mockSelectTerminal,
  mockIsTerminalLimitReached,
  mockAddTabToPane,
  mockTerminalApiSpawn,
  mockTerminalApiWrite,
  mockSpawnTerminalInPane,
  mockEnsureWorktreeSymlinks,
  mockTerminals,
  mockActiveConversationId
} = vi.hoisted(() => ({
  mockSetTerminals: vi.fn(),
  mockSelectTerminal: vi.fn(),
  mockIsTerminalLimitReached: vi.fn(),
  mockAddTabToPane: vi.fn(),
  mockTerminalApiSpawn: vi.fn(),
  mockTerminalApiWrite: vi.fn(),
  mockSpawnTerminalInPane: vi.fn(),
  mockEnsureWorktreeSymlinks: vi.fn(),
  mockTerminals: [] as Array<{ id?: string; projectId: string; ptyId?: string }>,
  mockActiveConversationId: { current: '018f7a1c-1b4d-7c8a-9f01-0123456789ab' as string | null }
}))

vi.mock('@/stores/session-workspace-sync-store', () => ({
  useSessionWorkspaceSyncStore: {
    getState: () => ({ activeConversationId: mockActiveConversationId.current })
  }
}))

vi.mock('@/stores/terminal-store', () => ({
  useTerminalStore: {
    getState: () => ({
      terminals: mockTerminals,
      setTerminals: mockSetTerminals,
      selectTerminal: mockSelectTerminal,
      isTerminalLimitReached: mockIsTerminalLimitReached
    })
  }
}))

vi.mock('@/stores/workspace-store', () => ({
  useWorkspaceStore: {
    getState: () => ({
      activePaneId: 'pane-1',
      addTabToPane: mockAddTabToPane
    })
  }
}))

vi.mock('@/stores/project-store', () => ({
  useProjectStore: {
    getState: () => ({
      projects: [{ id: 'proj-1', name: 'Test', path: '/test', envVars: [] }]
    })
  }
}))

vi.mock('@/stores/app-settings-store', () => ({
  useAppSettingsStore: {
    getState: () => ({ settings: { maxTerminalsPerProject: 10 } })
  }
}))

vi.mock('@/lib/api', () => ({
  terminalApi: { spawn: mockTerminalApiSpawn, write: mockTerminalApiWrite }
}))

vi.mock('@/lib/terminal-spawn', () => ({
  spawnTerminalInPane: (...args: unknown[]) => mockSpawnTerminalInPane(...args)
}))

vi.mock('@/lib/env-parser', () => ({
  resolveEnvForSpawn: () => ({ env: {}, hasProjectEnv: false })
}))

vi.mock('@/lib/worktree-context', () => ({
  ensureWorktreeSymlinks: mockEnsureWorktreeSymlinks
}))

import { launchAgentInPane, launchAgentResumeInPane } from '@/lib/agent-launch'
import { getBuiltInAgent } from '@/lib/agents/agent-registry'

const claude = getBuiltInAgent('claude-code')!
const gemini = getBuiltInAgent('gemini-cli')!

function lastCreatedTerminal(): Record<string, unknown> {
  const batch = mockSetTerminals.mock.calls.at(-1)?.[0] as Array<Record<string, unknown>>
  return batch[batch.length - 1]
}

describe('launchAgentInPane', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockTerminals.length = 0
    mockActiveConversationId.current = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
    mockIsTerminalLimitReached.mockReturnValue(false)
    // CAP-3: spawn is the only claim issuance path — the fixture carries the
    // issued lease credential alongside the terminal info.
    mockTerminalApiSpawn.mockResolvedValue({
      success: true,
      data: { id: 'pty-1', shell: 'claude', cwd: '/test', claim: 'claim-agent-1' }
    })
    mockSpawnTerminalInPane.mockResolvedValue({ success: true, terminalId: 'term-1' })
    mockTerminalApiWrite.mockResolvedValue({ success: true })
  })

  it('spawns with program/args/kind:agent and a positional prompt', async () => {
    const result = await launchAgentInPane('pane-1', 'proj-1', '/test', claude, 'explain this')
    expect(result.success).toBe(true)
    expect(mockTerminalApiSpawn).toHaveBeenCalledWith(
      expect.objectContaining({
        projectId: 'proj-1',
        cwd: '/test',
        program: 'claude',
        args: ['explain this'],
        kind: 'agent'
      })
    )
  })

  it('uses the flag form for gemini', async () => {
    await launchAgentInPane('pane-1', 'proj-1', '/test', gemini, 'query')
    expect(mockTerminalApiSpawn).toHaveBeenCalledWith(
      expect.objectContaining({
        projectId: 'proj-1',
        program: 'gemini',
        args: ['-i', 'query'],
        kind: 'agent'
      })
    )
  })

  it('passes a dangerous prompt as a single discrete arg', async () => {
    const dangerous = '"; rm -rf ~ # `whoami`'
    await launchAgentInPane('pane-1', 'proj-1', '/test', claude, dangerous)
    const opts = mockTerminalApiSpawn.mock.calls[0][0]
    expect(opts.args).toEqual([dangerous])
  })

  it('tags the terminal with agent metadata excluding the seed prompt', async () => {
    await launchAgentInPane('pane-1', 'proj-1', '/test', claude, 'do a thing')
    expect(mockSetTerminals).toHaveBeenCalled()
    expect(lastCreatedTerminal()).toMatchObject({
      kind: 'agent',
      agentId: 'claude-code',
      agentName: 'Claude Code',
      agentProgram: 'claude',
      agentArgs: [],
      ptyId: 'pty-1'
    })
    expect(JSON.stringify(lastCreatedTerminal())).not.toContain('do a thing')
  })

  it('captures the issued claim on the created terminal record', async () => {
    const result = await launchAgentInPane('pane-1', 'proj-1', '/test', claude, 'x')

    expect(result.success).toBe(true)
    // CAP-3: the lease credential lands in the batched set() record alongside
    // the ptyId — no spawn path may produce a lease-less terminal record.
    expect(lastCreatedTerminal()).toMatchObject({
      ptyId: 'pty-1',
      claim: 'claim-agent-1'
    })
  })

  it('omits the claim key when the spawn response carries none', async () => {
    mockTerminalApiSpawn.mockResolvedValue({
      success: true,
      data: { id: 'pty-2', shell: 'claude', cwd: '/test' }
    })

    await launchAgentInPane('pane-1', 'proj-1', '/test', claude, 'x')

    expect(lastCreatedTerminal()).not.toHaveProperty('claim')
  })

  it('names the terminal after the agent, selects it, and adds a tab', async () => {
    const result = await launchAgentInPane('pane-1', 'proj-1', '/test', claude, 'x')
    expect(result.terminalId).toBeTruthy()
    expect(lastCreatedTerminal()).toMatchObject({
      name: 'Claude Code',
      projectId: 'proj-1',
      shell: 'claude',
      cwd: '/test'
    })
    expect(mockSelectTerminal).toHaveBeenCalledWith(result.terminalId)
    expect(mockAddTabToPane).toHaveBeenCalledWith(
      'pane-1',
      expect.objectContaining({ type: 'terminal', terminalId: result.terminalId })
    )
  })

  it('passes the prompt positionally for pi', async () => {
    const pi = getBuiltInAgent('pi')!
    await launchAgentInPane('pane-1', 'proj-1', '/test', pi, 'ignored prompt')
    expect(mockTerminalApiSpawn).toHaveBeenCalledWith(
      expect.objectContaining({
        projectId: 'proj-1',
        program: 'pi',
        args: ['ignored prompt'],
        kind: 'agent'
      })
    )
  })

  it('blocks when the per-project terminal limit is reached', async () => {
    mockTerminals.push({ projectId: 'proj-1' }, { projectId: 'proj-1' })
    const result = await launchAgentInPane('pane-1', 'proj-1', '/test', claude, 'x', {
      maxTerminalsPerProject: 2
    })
    expect(result.success).toBe(false)
    expect(mockTerminalApiSpawn).not.toHaveBeenCalled()
  })

  it('surfaces a spawn failure as a result error', async () => {
    mockTerminalApiSpawn.mockResolvedValue({ success: false, error: 'no binary' })
    const result = await launchAgentInPane('pane-1', 'proj-1', '/test', claude, 'x')
    expect(result.success).toBe(false)
    expect(result.error).toBe('no binary')
    expect(mockSetTerminals).not.toHaveBeenCalled()
  })

  it('resumes from the project workspace without an open conversation', async () => {
    mockActiveConversationId.current = null
    mockTerminals.push({ id: 'term-1', projectId: 'proj-1', ptyId: 'pty-1' })
    const result = await launchAgentResumeInPane(
      'pane-1',
      'proj-1',
      '/test',
      claude,
      {
        schemaVersion: 1,
        id: 'claude-code:abc:/tmp/a.jsonl',
        agentId: 'claude-code',
        sessionId: 'abc',
        cwd: '/test',
        title: 'Hello',
        createdAt: null,
        updatedAt: null,
        messageCount: 1,
        filePath: '/tmp/a.jsonl',
        resumable: true
      },
      '',
      '',
      { shellSettleMs: 0 }
    )
    expect(result.success).toBe(true)
    expect(mockSpawnTerminalInPane).toHaveBeenCalledWith(
      'pane-1',
      'proj-1',
      '/test',
      expect.objectContaining({ extraEnv: {}, maxTerminalsPerProject: 10 })
    )
    expect(mockSpawnTerminalInPane.mock.calls[0][3]).not.toHaveProperty('conversationId')
    expect(mockTerminalApiSpawn).not.toHaveBeenCalled()
    expect(mockTerminalApiWrite).toHaveBeenCalledWith('pty-1', 'claude --resume abc\r')
  })

  it('resumes with extras before --resume and without a seed prompt', async () => {
    mockTerminals.push({ id: 'term-1', projectId: 'proj-1', ptyId: 'pty-1' })
    const result = await launchAgentResumeInPane(
      'pane-1',
      'proj-1',
      '/test',
      claude,
      {
        schemaVersion: 1,
        id: 'claude-code:abc:/tmp/a.jsonl',
        agentId: 'claude-code',
        sessionId: 'abc',
        cwd: '/test',
        title: 'Hello',
        createdAt: null,
        updatedAt: null,
        messageCount: 1,
        filePath: '/tmp/a.jsonl',
        resumable: true
      },
      '--dangerously-skip-permissions',
      '',
      { shellSettleMs: 0 }
    )
    expect(result.success).toBe(true)
    expect(mockTerminalApiWrite).toHaveBeenCalledWith(
      'pty-1',
      'claude --dangerously-skip-permissions --resume abc\r'
    )
  })

  it('resumes Codex in a project shell with CODEX_HOME', async () => {
    mockTerminals.push({ id: 'term-1', projectId: 'proj-1', ptyId: 'pty-1' })
    const result = await launchAgentResumeInPane(
      'pane-1',
      'proj-1',
      '/test',
      getBuiltInAgent('codex')!,
      {
        schemaVersion: 1,
        id: 'codex:s1:/tmp/a.jsonl',
        agentId: 'codex',
        sessionId: 's1',
        cwd: '/test',
        title: 'Hello',
        createdAt: null,
        updatedAt: null,
        messageCount: 1,
        filePath: '/tmp/a.jsonl',
        codexHome: '/tmp/codex-home',
        resumable: true
      },
      '',
      '',
      { shellSettleMs: 0 }
    )
    expect(result.success).toBe(true)
    expect(mockSpawnTerminalInPane).toHaveBeenCalledWith(
      'pane-1',
      'proj-1',
      '/test',
      expect.objectContaining({ extraEnv: { CODEX_HOME: '/tmp/codex-home' } })
    )
    expect(mockTerminalApiWrite).toHaveBeenCalledWith('pty-1', 'codex resume s1\r')
  })
})
