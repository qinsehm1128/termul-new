import type { HookStateDto, HookStateSyncResponse } from '@shared/types/agent-hooks.types'
import { act, cleanup, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useAgentHookStateStore } from '@/stores/agent-hook-state-store'
import { AgentHookSessionsPanel } from './AgentHookSessionsPanel'

const mocks = vi.hoisted(() => ({ state: vi.fn(), log: vi.fn() }))
vi.mock('@/lib/agent-hooks-api', () => ({ agentHooksApi: { state: mocks.state } }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: mocks.log }))

const SESSION_ID = 'session-0123456789abcdef'
const TERMINAL_ID = 'pty-0123456789abcdef'
const PROJECT_ID = 'project-0123456789abcdef'

function hookState(overrides: Partial<HookStateDto> = {}): HookStateDto {
  return {
    instance_id: 'host-1',
    terminal_id: TERMINAL_ID,
    generation: 1,
    adapter_install_id: 'install-1',
    agent_session_id: SESSION_ID,
    project_id: PROJECT_ID,
    state: 'busy',
    ready: true,
    identity: {
      adapter_id: 'claude-code',
      agent_session_id: SESSION_ID,
      transcript_path: '/home/dev/.claude/transcripts/full.jsonl',
      cwd: '/home/dev/work/alpha',
      source: 'hook',
      descriptor_id: null,
      resume_argv: null
    },
    restore_state: 'confirmed',
    revision: 1,
    ...overrides
  }
}

function snapshot(states: HookStateDto[], revision = 1): HookStateSyncResponse {
  return {
    kind: 'snapshot',
    instance_id: 'host-1',
    current_revision: revision,
    states,
    tombstones: []
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  useAgentHookStateStore.getState().reset()
  mocks.log.mockResolvedValue(undefined)
  mocks.state.mockResolvedValue(snapshot([]))
})

afterEach(cleanup)

describe('AgentHookSessionsPanel', () => {
  it('shows a single empty line when no hook state exists', async () => {
    render(<AgentHookSessionsPanel />)
    expect(await screen.findByText('No hook sessions')).toBeInTheDocument()
  })

  it('renders the state, ready/restore flags, shortened ids and relative update time', async () => {
    mocks.state.mockResolvedValue(
      snapshot([{ ...hookState(), hook_at: Date.now() } as HookStateDto], 1)
    )
    render(<AgentHookSessionsPanel />)

    expect(await screen.findByText('Busy')).toBeInTheDocument()
    expect(screen.getByText('claude-code')).toBeInTheDocument()
    expect(screen.getByText('Yes')).toBeInTheDocument()
    expect(screen.getByText('Confirmed')).toBeInTheDocument()
    expect(screen.getByText('just now')).toBeInTheDocument()
    expect(screen.getByText('pty-0123…')).toBeInTheDocument()
    expect(screen.getByText('project-…')).toBeInTheDocument()
    // Never render the full transcript path, cwd or complete opaque ids.
    expect(screen.queryByText('/home/dev/.claude/transcripts/full.jsonl')).not.toBeInTheDocument()
    expect(screen.queryByText('/home/dev/work/alpha')).not.toBeInTheDocument()
    expect(screen.queryByText(TERMINAL_ID)).not.toBeInTheDocument()
    expect(screen.queryByText(PROJECT_ID)).not.toBeInTheDocument()
    expect(screen.queryByText(SESSION_ID)).not.toBeInTheDocument()
  })

  it('updates rows when the subscription pushes a delta into the shared store', async () => {
    render(<AgentHookSessionsPanel />)
    await screen.findByText('No hook sessions')

    act(() => {
      useAgentHookStateStore.getState().applySync({
        kind: 'delta',
        instance_id: 'host-1',
        current_revision: 2,
        states: [hookState({ revision: 2, state: 'idle' })],
        tombstones: []
      })
    })

    expect(await screen.findByText('Idle')).toBeInTheDocument()
    expect(screen.queryByText('No hook sessions')).not.toBeInTheDocument()
  })

  it('logs a load failure without forwarding ids or paths', async () => {
    mocks.state.mockRejectedValue(new Error(`failed for ${TERMINAL_ID}`))
    render(<AgentHookSessionsPanel />)

    await waitFor(() =>
      expect(mocks.log).toHaveBeenCalledWith({
        source: 'AgentHookSessionsPanel',
        message: 'Agent hook state load failed'
      })
    )
    expect(JSON.stringify(mocks.log.mock.calls)).not.toContain(TERMINAL_ID)
    expect(JSON.stringify(mocks.log.mock.calls)).not.toContain('/home/dev')
  })
})
