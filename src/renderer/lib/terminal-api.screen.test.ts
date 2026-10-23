import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  resume: vi.fn(),
  recordHost: vi.fn(),
  clear: vi.fn()
}))
vi.mock('./tauri-runtime', () => ({ isTauriContext: () => true }))
vi.mock('./local-terminal-spawns', () => ({
  trackLocalTerminalSpawn: (run: () => unknown) => run()
}))
vi.mock('@/stores/screen-observation-store', () => ({
  useScreenObservationStore: {
    getState: () => ({ recordHost: mocks.recordHost, clear: mocks.clear })
  }
}))
vi.mock('./tauri-terminal-api', () => ({
  createTauriTerminalApi: () => ({
    spawn: vi.fn(),
    attach: vi.fn(),
    terminate: vi.fn(),
    resume: mocks.resume
  }),
  addRendererRef: vi.fn(),
  removeRendererRef: vi.fn(),
  setTerminalProtected: vi.fn()
}))
vi.mock('./web-terminal-api', () => ({
  createWebTerminalApi: vi.fn(),
  webTerminalInternals: {}
}))

import { resumeTerminal } from './terminal-api'

describe('cold resume preserves the host screen engine', () => {
  beforeEach(() => vi.clearAllMocks())

  it('records the issued claim and original host mode without consulting project preferences', async () => {
    const terminal = {
      id: 'pty-1',
      shell: '/bin/sh',
      cwd: '/project',
      pid: 42,
      cols: 80,
      rows: 24,
      latestSeq: 10,
      gap: false,
      terminalScreenEngine: 'ghostty-vt'
    }
    const result = { success: true, data: { terminal, claim: 'new-claim' } }
    mocks.resume.mockResolvedValue(result)
    const request = { terminalId: 'pty-1', conversationId: 'conversation-1', lastSeq: 8 }
    expect(await resumeTerminal(request)).toBe(result)
    expect(mocks.resume).toHaveBeenCalledWith(request)
    expect(mocks.recordHost).toHaveBeenCalledWith('pty-1', {
      ...terminal,
      claim: 'new-claim'
    })
  })

  it('does not overwrite an observation after a failed resume', async () => {
    mocks.resume.mockResolvedValue({
      success: false,
      error: { code: 'UNAUTHORIZED', message: 'denied' }
    })
    await resumeTerminal({ terminalId: 'pty-1', conversationId: 'conversation-1', lastSeq: 0 })
    expect(mocks.recordHost).not.toHaveBeenCalled()
  })
})
