/**
 * Idle-agent reclamation.
 *
 * An agent adapter is one OS process per (agent config, cwd) and it is kept
 * warm on purpose — the pre-warm pool exists so the first prompt is not slow.
 * What it never had was an upper bound: nothing killed a warm process, ever,
 * short of deleting its config or quitting the app. A measured session showed
 * ~250 MB sitting in adapters that had been idle for nine hours, one of them
 * spawned three seconds after launch and never used.
 *
 * `reclaimIdleAgents` takes `now` rather than reading the clock so the TTL is
 * exercised directly instead of through fake timers.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'

const { killAgentSpy, logFrontendError } = vi.hoisted(() => ({
  killAgentSpy: vi.fn(async () => {}),
  logFrontendError: vi.fn(async () => {})
}))

vi.mock('@/lib/acp-api', async (importActual) => {
  const actual = await importActual<typeof import('@/lib/acp-api')>()
  return {
    ...actual,
    acpApi: { ...actual.acpApi, killAgent: killAgentSpy }
  }
})
vi.mock('@/lib/acp-history-persistence', async (importActual) => {
  const actual = await importActual<typeof import('@/lib/acp-history-persistence')>()
  return {
    ...actual,
    loadSessionIndex: vi.fn(async () => []),
    saveSessionIndex: vi.fn(async () => {}),
    queueSessionPayloadSave: vi.fn(async () => {})
  }
})
vi.mock('@/lib/log-api', () => ({ logFrontendError }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => true }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

import { agentReuseKey, IDLE_AGENT_TTL_MS, useAcpStore } from './acp-store'

const KEY_A = agentReuseKey('cfg-a', '/projects/a')
const KEY_B = agentReuseKey('cfg-b', '/projects/b')

function session(id: string, agentId: string, status: 'active' | 'closed') {
  return {
    id,
    conversationId: undefined,
    agentId,
    cwd: '/projects/a',
    projectId: '',
    status,
    title: null,
    activeTurn: false,
    openTurnId: null,
    modes: null,
    configOptions: [],
    lastError: null,
    createdAt: 1
  }
}

beforeEach(() => {
  killAgentSpy.mockClear()
  logFrontendError.mockClear()
  useAcpStore.setState({
    agents: {
      'agent-a': { id: 'agent-a', capabilities: null },
      'agent-b': { id: 'agent-b', capabilities: null }
    },
    agentStatus: { 'agent-a': 'connected', 'agent-b': 'connected' },
    configToLiveAgent: { [KEY_A]: 'agent-a', [KEY_B]: 'agent-b' },
    sessions: {},
    sessionIndex: [],
    messages: {},
    pendingPermissions: {},
    pendingQuestions: {}
  })
  useAcpStore.getState().resetIdleAgentTracking()
})

describe('reclaimIdleAgents', () => {
  it('does not kill an agent on the first sweep that observes it idle', async () => {
    await useAcpStore.getState().reclaimIdleAgents(1_000)

    expect(killAgentSpy).not.toHaveBeenCalled()
  })

  it('kills an agent that has stayed idle for the whole TTL', async () => {
    await useAcpStore.getState().reclaimIdleAgents(1_000)
    await useAcpStore.getState().reclaimIdleAgents(1_000 + IDLE_AGENT_TTL_MS)

    expect(killAgentSpy).toHaveBeenCalledWith('agent-a')
    expect(killAgentSpy).toHaveBeenCalledWith('agent-b')
    expect(useAcpStore.getState().configToLiveAgent).toEqual({})
  })

  it('spares an agent that still has an open session', async () => {
    useAcpStore.setState({ sessions: { s1: session('s1', 'agent-a', 'active') } })

    await useAcpStore.getState().reclaimIdleAgents(1_000)
    await useAcpStore.getState().reclaimIdleAgents(1_000 + IDLE_AGENT_TTL_MS)

    expect(killAgentSpy).not.toHaveBeenCalledWith('agent-a')
    expect(killAgentSpy).toHaveBeenCalledWith('agent-b')
  })

  it('closed sessions do not keep their agent alive', async () => {
    useAcpStore.setState({ sessions: { s1: session('s1', 'agent-a', 'closed') } })

    await useAcpStore.getState().reclaimIdleAgents(1_000)
    await useAcpStore.getState().reclaimIdleAgents(1_000 + IDLE_AGENT_TTL_MS)

    expect(killAgentSpy).toHaveBeenCalledWith('agent-a')
  })

  /**
   * The idle clock has to restart, not merely pause: an agent used at minute 9
   * of a 10-minute TTL would otherwise be reaped one minute later despite being
   * the freshest thing in the pool.
   */
  it('restarts the idle clock when an agent is used again', async () => {
    await useAcpStore.getState().reclaimIdleAgents(1_000)

    useAcpStore.setState({ sessions: { s1: session('s1', 'agent-a', 'active') } })
    await useAcpStore.getState().reclaimIdleAgents(2_000)
    useAcpStore.setState({ sessions: { s1: session('s1', 'agent-a', 'closed') } })

    // The original idle start is gone, so this sweep only re-arms the clock at
    // `1_000 + TTL` — it does not inherit the head start.
    await useAcpStore.getState().reclaimIdleAgents(1_000 + IDLE_AGENT_TTL_MS)
    expect(killAgentSpy).not.toHaveBeenCalledWith('agent-a')

    // Still short of a full idle stretch from the restart.
    await useAcpStore.getState().reclaimIdleAgents(2_000 + IDLE_AGENT_TTL_MS)
    expect(killAgentSpy).not.toHaveBeenCalledWith('agent-a')

    await useAcpStore.getState().reclaimIdleAgents(1_000 + 2 * IDLE_AGENT_TTL_MS)
    expect(killAgentSpy).toHaveBeenCalledWith('agent-a')
  })

  it('leaves a still-spawning agent alone', async () => {
    useAcpStore.setState({ agentStatus: { 'agent-a': 'spawning', 'agent-b': 'connected' } })

    await useAcpStore.getState().reclaimIdleAgents(1_000)
    await useAcpStore.getState().reclaimIdleAgents(1_000 + IDLE_AGENT_TTL_MS)

    expect(killAgentSpy).not.toHaveBeenCalledWith('agent-a')
    expect(killAgentSpy).toHaveBeenCalledWith('agent-b')
  })

  // One agent refusing to die must not stop the sweep: the whole point is to
  // bound memory, and a wedged process is exactly the one worth retrying later.
  it('keeps sweeping when a kill fails', async () => {
    killAgentSpy.mockRejectedValueOnce(new Error('kill failed'))

    await useAcpStore.getState().reclaimIdleAgents(1_000)
    await useAcpStore.getState().reclaimIdleAgents(1_000 + IDLE_AGENT_TTL_MS)

    expect(killAgentSpy).toHaveBeenCalledTimes(2)
    expect(logFrontendError).toHaveBeenCalled()
  })
})
