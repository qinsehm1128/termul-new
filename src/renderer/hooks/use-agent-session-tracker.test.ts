import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useTerminalStore } from '@/stores/terminal-store'
import type { Terminal } from '@/types/project'

const { mockList, mockDetect } = vi.hoisted(() => ({ mockList: vi.fn(), mockDetect: vi.fn() }))
vi.mock('@/lib/api', () => ({ terminalApi: { list: mockList } }))
vi.mock('@/lib/agents/live-agent-sessions', async () => {
  const actual = await vi.importActual<typeof import('@/lib/agents/live-agent-sessions')>(
    '@/lib/agents/live-agent-sessions'
  )
  return { ...actual, detectLiveAgentSessions: mockDetect }
})

import { syncTerminalAgentSessions } from './use-agent-session-tracker'

const terminal = (id: string, ptyId: string, extra: Partial<Terminal> = {}): Terminal => ({
  id,
  name: id,
  shell: 'zsh',
  ptyId,
  ...extra
})
const status = (id: string, pid: number) => ({
  id,
  pid,
  shell: 'zsh',
  cwd: '/',
  cols: 80,
  rows: 24
})
const remembered = {
  agentId: 'claude-code' as const,
  sessionId: 'old',
  resumeArgv: ['claude', '--resume', 'old']
}

describe('syncTerminalAgentSessions', () => {
  beforeEach(() => {
    mockList.mockReset()
    mockDetect.mockReset()
  })

  it('records the session found under each terminal’s shell', async () => {
    useTerminalStore.setState({ terminals: [terminal('t1', 'pty-1'), terminal('t2', 'pty-2')] })
    mockList.mockResolvedValue({
      success: true,
      data: [status('pty-1', 101), status('pty-2', 102)]
    })
    mockDetect.mockResolvedValue([
      { rootPid: 102, agentId: 'codex', sessionId: 'c-1', cwd: '/repo' }
    ])

    await syncTerminalAgentSessions()

    expect(mockDetect).toHaveBeenCalledWith([101, 102])
    const [t1, t2] = useTerminalStore.getState().terminals
    expect(t1.agentSession).toBeUndefined()
    expect(t2.agentSession).toEqual({
      agentId: 'codex',
      sessionId: 'c-1',
      cwd: '/repo',
      resumeArgv: ['codex', 'resume', 'c-1']
    })
  })

  it('forgets a session once the agent has exited its shell', async () => {
    useTerminalStore.setState({
      terminals: [terminal('t1', 'pty-1', { agentSession: remembered })]
    })
    mockList.mockResolvedValue({ success: true, data: [status('pty-1', 101)] })
    mockDetect.mockResolvedValue([])

    await syncTerminalAgentSessions()

    expect(useTerminalStore.getState().terminals[0].agentSession).toBeUndefined()
  })

  it('keeps what it last saw when the terminal’s process is not listed', async () => {
    useTerminalStore.setState({
      terminals: [terminal('t1', 'pty-gone', { agentSession: remembered })]
    })
    mockList.mockResolvedValue({ success: true, data: [] })
    mockDetect.mockResolvedValue([])

    await syncTerminalAgentSessions()

    expect(useTerminalStore.getState().terminals[0].agentSession).toEqual(remembered)
  })
})
