import type { Terminal } from '@xterm/xterm'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { readTerminalInputModes, supportsTerminalInputModes, terminalApi } from '@/lib/terminal-api'
import { useTerminalStore } from '@/stores/terminal-store'
import { registerTerminal, unregisterTerminal } from '@/utils/terminal-registry'
import { resyncRendererInputModes } from './terminal-input-resync'

vi.mock('@/lib/terminal-api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/terminal-api')>()
  return {
    ...actual,
    supportsTerminalInputModes: vi.fn(() => true),
    readTerminalInputModes: vi.fn(),
    terminalApi: {
      ...actual.terminalApi,
      resume: vi.fn(),
      attach: vi.fn(),
      watch: vi.fn()
    }
  }
})

const CONVERSATION_ID = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
const PTY_ID = 'pty-rehydrated'
const MODES = {
  mouseTracking: 1002,
  mouseEncoding: 1006,
  bracketedPaste: true,
  applicationCursor: false,
  focusEvents: true,
  applicationKeypad: false,
  alternateScreen: false
}

function registerFakeRenderer(ptyId: string): Terminal {
  const terminal = {
    write: vi.fn((_data: string, callback?: () => void) => callback?.()),
    modes: { mouseTrackingMode: 'none' }
  } as unknown as Terminal
  registerTerminal(ptyId, terminal)
  return terminal
}

function seedRehydratedTerminal(overrides: Record<string, unknown> = {}): void {
  useTerminalStore.setState({
    terminals: [
      {
        id: 'record-rehydrated',
        name: 'Restored terminal',
        projectId: 'project-1',
        conversationId: CONVERSATION_ID,
        shell: 'bash',
        ptyId: PTY_ID,
        // Exactly what a successful `terminal_watch` leaves behind: the view
        // is live and healthy, but no claim was ever issued to this window.
        healthStatus: 'running',
        resumeCursor: 12,
        output: [],
        ...overrides
      }
    ],
    activeTerminalId: 'record-rehydrated',
    ptyIdIndex: new Map([[PTY_ID, 'record-rehydrated']]),
    cleanupRecoveries: {}
  })
}

function resumeGrant(claim = 'recovered-claim') {
  return {
    success: true as const,
    data: {
      claim,
      terminal: {
        id: PTY_ID,
        shell: 'bash',
        cwd: '/terminal-cwd',
        pid: 4242,
        cols: 80,
        rows: 24,
        latestSeq: 87,
        gap: false
      }
    }
  }
}

describe('resyncRendererInputModes after a claim-less rehydrate', () => {
  beforeEach(() => {
    unregisterTerminal(PTY_ID)
    vi.mocked(terminalApi.resume).mockReset()
    vi.mocked(terminalApi.attach).mockReset()
    vi.mocked(terminalApi.watch).mockReset()
    vi.mocked(readTerminalInputModes).mockReset()
    vi.mocked(supportsTerminalInputModes).mockReturnValue(true)
  })

  it('reproduces the chain: watch reattached the record, resync has no claim', () => {
    // The watch branch marked it running without issuing a claim (the exact
    // state attachResumedTerminalRenderer leaves after a successful watch).
    seedRehydratedTerminal({ resumeCursor: 87, healthStatus: 'running' })
    registerFakeRenderer(PTY_ID)

    expect(useTerminalStore.getState().findTerminalByPtyId(PTY_ID)?.claim).toBeUndefined()

    vi.mocked(terminalApi.resume).mockResolvedValue(resumeGrant())
    vi.mocked(readTerminalInputModes).mockResolvedValue({ success: true, data: MODES })

    return expect(resyncRendererInputModes(PTY_ID, { recoverClaim: true })).resolves.toMatchObject({
      status: 'applied'
    })
  })

  it('recovers the claim through the host-authorized resume path, live-only', async () => {
    seedRehydratedTerminal()
    registerFakeRenderer(PTY_ID)
    vi.mocked(terminalApi.resume).mockResolvedValue(resumeGrant())
    vi.mocked(readTerminalInputModes).mockResolvedValue({ success: true, data: MODES })

    const result = await resyncRendererInputModes(PTY_ID, { recoverClaim: true })

    expect(result.status).toBe('applied')
    // The host remains the authority: the existing resume path validates the
    // Conversation reference and rotates the claim.
    expect(terminalApi.resume).toHaveBeenCalledTimes(1)
    expect(terminalApi.resume).toHaveBeenCalledWith({
      conversationId: CONVERSATION_ID,
      terminalId: PTY_ID,
      // Live-only: a replayed backlog would be written to xterm a second time.
      lastSeq: Number.MAX_SAFE_INTEGER
    })
    // No second data channel: resume owns the one and only attach/handoff.
    expect(terminalApi.attach).not.toHaveBeenCalled()
    expect(terminalApi.watch).not.toHaveBeenCalled()
    // The retry uses the freshly issued credential.
    expect(readTerminalInputModes).toHaveBeenCalledWith(PTY_ID, 'recovered-claim')
    expect(useTerminalStore.getState().findTerminalByPtyId(PTY_ID)).toMatchObject({
      claim: 'recovered-claim',
      healthStatus: 'running',
      resumeCursor: 87
    })
  })

  it('acquires the claim once when two claim-gated actions race', async () => {
    seedRehydratedTerminal()
    registerFakeRenderer(PTY_ID)
    vi.mocked(terminalApi.resume).mockResolvedValue(resumeGrant())
    vi.mocked(readTerminalInputModes).mockResolvedValue({ success: true, data: MODES })

    const [first, second] = await Promise.all([
      resyncRendererInputModes(PTY_ID, { recoverClaim: true }),
      resyncRendererInputModes(PTY_ID, { recoverClaim: true })
    ])

    expect(first.status).toBe('applied')
    expect(second.status).toBe('applied')
    expect(terminalApi.resume).toHaveBeenCalledTimes(1)
  })

  it('reports host-denied when the host refuses the claim', async () => {
    seedRehydratedTerminal()
    registerFakeRenderer(PTY_ID)
    vi.mocked(terminalApi.resume).mockResolvedValue({
      success: false,
      error: 'Unauthorized',
      code: 'UNAUTHORIZED'
    })

    const result = await resyncRendererInputModes(PTY_ID, { recoverClaim: true })

    expect(result).toEqual({ status: 'unavailable', reason: 'host-denied' })
    expect(readTerminalInputModes).not.toHaveBeenCalled()
    // A refused action must not rewrite the record behind the user's back.
    expect(useTerminalStore.getState().findTerminalByPtyId(PTY_ID)).toMatchObject({
      healthStatus: 'running'
    })
    expect(useTerminalStore.getState().findTerminalByPtyId(PTY_ID)?.claim).toBeUndefined()
  })

  it('does not rotate a claim unless the takeover is explicit', async () => {
    // The automatic post-restore resync must stay view-only: rotating here
    // would revoke the phone/companion credential on every renderer reload.
    seedRehydratedTerminal()
    registerFakeRenderer(PTY_ID)

    const result = await resyncRendererInputModes(PTY_ID)

    expect(result).toEqual({ status: 'unavailable', reason: 'no-claim' })
    expect(terminalApi.resume).not.toHaveBeenCalled()
    expect(readTerminalInputModes).not.toHaveBeenCalled()
  })

  it('keeps the no-claim message for terminals without a Conversation id', async () => {
    seedRehydratedTerminal({ conversationId: undefined })
    registerFakeRenderer(PTY_ID)

    const result = await resyncRendererInputModes(PTY_ID, { recoverClaim: true })

    expect(result).toEqual({ status: 'unavailable', reason: 'no-claim' })
    expect(terminalApi.resume).not.toHaveBeenCalled()
  })

  it('stays unsupported on a transport without claim-gated input modes', async () => {
    seedRehydratedTerminal()
    registerFakeRenderer(PTY_ID)
    vi.mocked(supportsTerminalInputModes).mockReturnValue(false)

    const result = await resyncRendererInputModes(PTY_ID)

    expect(result).toEqual({ status: 'unavailable', reason: 'unsupported' })
    expect(terminalApi.resume).not.toHaveBeenCalled()
  })
})
