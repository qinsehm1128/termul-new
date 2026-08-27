import { renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useTerminalDetachedOutput } from './use-terminal-detached-output'

const {
  mockOnData,
  mockOnExit,
  mockAppendTranscript,
  mockConsumeTranscript,
  mockFindTerminalByPtyId,
  mockPeekCachedTerminal
} = vi.hoisted(() => ({
  mockOnData: vi.fn(),
  mockOnExit: vi.fn(),
  mockAppendTranscript: vi.fn(),
  mockConsumeTranscript: vi.fn(() => ''),
  mockFindTerminalByPtyId: vi.fn(),
  mockPeekCachedTerminal: vi.fn()
}))

/** Convert a string to Uint8Array for binary channel test data */
function toBytes(str: string): Uint8Array {
  return new TextEncoder().encode(str)
}

vi.mock('@/lib/api', () => ({
  terminalApi: {
    onData: mockOnData,
    onExit: mockOnExit
  }
}))

vi.mock('@/lib/log-api', () => ({
  logFrontendError: vi.fn(async () => undefined)
}))

vi.mock('@/components/terminal/terminal-cache', () => ({
  peekCachedTerminal: mockPeekCachedTerminal
}))

// Only the store instance is faked. `rendererOwnsDetachedContinuity` comes
// through from the real module on purpose: it is the single answer to "will the
// host replay this interval", and a hand-rolled copy here would let these tests
// keep passing after the production rule had changed underneath them.
vi.mock('@/stores/terminal-store', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/stores/terminal-store')>()),
  useTerminalStore: {
    getState: vi.fn(() => ({
      appendTranscript: mockAppendTranscript,
      consumeTranscript: mockConsumeTranscript,
      findTerminalByPtyId: mockFindTerminalByPtyId
    }))
  }
}))

type DataListener = (ptyId: string, data: Uint8Array) => void
type ExitListener = (ptyId: string, exitCode: number, signal?: number) => void

/** Mount the hook and hand back the listeners it registered. */
function mountHook(): { emit: DataListener; exit: ExitListener } {
  let dataListener: DataListener | undefined
  let exitListener: ExitListener | undefined
  mockOnData.mockImplementation((callback: DataListener) => {
    dataListener = callback
    return vi.fn()
  })
  mockOnExit.mockImplementation((callback: ExitListener) => {
    exitListener = callback
    return vi.fn()
  })

  renderHook(() => useTerminalDetachedOutput())

  if (!dataListener || !exitListener) throw new Error('listeners were not registered')
  return { emit: dataListener, exit: exitListener }
}

function capturedTranscript(): string {
  return mockAppendTranscript.mock.calls.map(([, chunk]) => chunk as string).join('')
}

describe('useTerminalDetachedOutput', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockOnExit.mockImplementation(() => vi.fn())
    mockFindTerminalByPtyId.mockReturnValue({ rendererAttachmentCount: 0, isAppHidden: false })
  })

  it('captures PTY output into transcript when no renderer is mounted', () => {
    const unsubscribe = vi.fn()
    let capturedCallback: ((ptyId: string, data: Uint8Array) => void) | undefined

    mockOnData.mockImplementation((callback: (ptyId: string, data: Uint8Array) => void) => {
      capturedCallback = callback
      return unsubscribe
    })

    const { unmount } = renderHook(() => useTerminalDetachedOutput())

    capturedCallback?.('pty-a', toBytes('streaming output'))

    expect(mockAppendTranscript).toHaveBeenCalledWith('pty-a', 'streaming output')

    unmount()
    expect(unsubscribe).toHaveBeenCalled()
  })

  it('skips transcript capture for visible terminals with an attached renderer', () => {
    let capturedCallback: ((ptyId: string, data: Uint8Array) => void) | undefined

    mockFindTerminalByPtyId.mockReturnValue({ rendererAttachmentCount: 1, isAppHidden: false })
    mockOnData.mockImplementation((callback: (ptyId: string, data: Uint8Array) => void) => {
      capturedCallback = callback
      return vi.fn()
    })

    renderHook(() => useTerminalDetachedOutput())

    capturedCallback?.('pty-a', toBytes('visible output'))

    expect(mockAppendTranscript).not.toHaveBeenCalled()
  })

  it('does not capture PTY output when app is hidden but a renderer is still attached', () => {
    let capturedCallback: ((ptyId: string, data: Uint8Array) => void) | undefined

    mockFindTerminalByPtyId.mockReturnValue({ rendererAttachmentCount: 1, isAppHidden: true })
    mockOnData.mockImplementation((callback: (ptyId: string, data: Uint8Array) => void) => {
      capturedCallback = callback
      return vi.fn()
    })

    renderHook(() => useTerminalDetachedOutput())

    capturedCallback?.('pty-a', toBytes('hidden output'))

    // Transcript is only for detached terminals (project switch).
    // When a renderer IS attached, data flows through xterm naturally
    // and will resume when the app becomes visible again.
    expect(mockAppendTranscript).not.toHaveBeenCalled()
  })

  it('ignores empty terminal-data payloads', () => {
    let capturedCallback: ((ptyId: string, data: Uint8Array) => void) | undefined

    mockOnData.mockImplementation((callback: (ptyId: string, data: Uint8Array) => void) => {
      capturedCallback = callback
      return vi.fn()
    })

    renderHook(() => useTerminalDetachedOutput())

    capturedCallback?.('pty-a', new Uint8Array(0))

    expect(mockAppendTranscript).not.toHaveBeenCalled()
  })

  it('ignores data for unknown PTY (store record missing)', () => {
    let capturedCallback: ((ptyId: string, data: Uint8Array) => void) | undefined

    mockFindTerminalByPtyId.mockReturnValue(undefined)
    mockOnData.mockImplementation((callback: (ptyId: string, data: Uint8Array) => void) => {
      capturedCallback = callback
      return vi.fn()
    })

    renderHook(() => useTerminalDetachedOutput())

    capturedCallback?.('pty-x', toBytes('late data'))

    expect(mockAppendTranscript).not.toHaveBeenCalled()
  })

  it('keeps a multi-byte codepoint intact when it is split across two PTY chunks', () => {
    const { emit } = mountHook()
    const bytes = toBytes('中')
    expect(bytes.length).toBe(3)

    emit('pty-a', bytes.slice(0, 2))
    emit('pty-a', bytes.slice(2))

    const joined = capturedTranscript()
    expect(joined).toBe('中')
    expect(joined).not.toContain('\uFFFD')
  })

  it('does not desynchronise the decoder when a chunk is dropped for an attached renderer', () => {
    const { emit } = mountHook()
    const bytes = toBytes('中')

    // The leading two bytes arrive while a renderer is attached, so the event is
    // dropped — but it must still have been fed to the retained decoder.
    mockFindTerminalByPtyId.mockReturnValue({ rendererAttachmentCount: 1, isAppHidden: false })
    emit('pty-a', bytes.slice(0, 2))
    expect(mockAppendTranscript).not.toHaveBeenCalled()

    mockFindTerminalByPtyId.mockReturnValue({ rendererAttachmentCount: 0, isAppHidden: false })
    emit('pty-a', bytes.slice(2))

    expect(capturedTranscript()).not.toContain('\uFFFD')
  })

  it('releases the retained decoder when the pty exits', () => {
    const { emit, exit } = mountHook()
    const bytes = toBytes('中')

    emit('pty-a', bytes.slice(0, 2))
    exit('pty-a', 0)
    mockAppendTranscript.mockClear()

    // A fresh decoder must not carry the pre-exit partial sequence forward.
    emit('pty-a', bytes.slice(0, 2))
    emit('pty-a', bytes.slice(2))

    expect(capturedTranscript()).toBe('中')
  })

  // The cached xterm is a bounded, self-evicting sink that never has to be
  // spliced back onto a live screen. The transcript is the unbounded string
  // that did, and whose splice was being abandoned whole — losing the detached
  // interval and leaving the user on the frame they left.
  describe('cached xterm as the detached sink', () => {
    const cachedWrite = vi.fn()

    function cacheATerminal(): void {
      mockPeekCachedTerminal.mockReturnValue({ terminal: { write: cachedWrite } })
    }

    beforeEach(() => {
      cachedWrite.mockClear()
      mockPeekCachedTerminal.mockReturnValue(undefined)
      mockFindTerminalByPtyId.mockReturnValue({
        rendererAttachmentCount: 0,
        healthStatus: 'running',
        claim: 'claim-1'
      })
    })

    it('writes into the cached instance instead of growing a transcript', () => {
      cacheATerminal()
      const { emit } = mountHook()

      emit('pty-a', toBytes('detached output'))

      expect(cachedWrite).toHaveBeenCalledWith('detached output')
      expect(mockAppendTranscript).not.toHaveBeenCalled()
    })

    it('drains a transcript captured earlier ahead of the new bytes', () => {
      cacheATerminal()
      mockFindTerminalByPtyId.mockReturnValue({
        rendererAttachmentCount: 0,
        healthStatus: 'running',
        claim: 'claim-1',
        transcript: 'older span'
      })
      mockConsumeTranscript.mockReturnValue('older span')
      const { emit } = mountHook()

      emit('pty-a', toBytes('newer span'))

      // Order is the whole point: the transcript predates the instance becoming
      // the sink, so writing it second would replay the two halves reversed.
      expect(cachedWrite.mock.calls.map(([chunk]) => chunk)).toEqual(['older span', 'newer span'])
    })

    it('keeps using the transcript when the host will replay the interval', () => {
      cacheATerminal()
      // No claim — reattach goes through watch/attach and the host replays these
      // same bytes. Writing them here too would duplicate whole blocks.
      mockFindTerminalByPtyId.mockReturnValue({
        rendererAttachmentCount: 0,
        healthStatus: 'running',
        claim: undefined
      })
      const { emit } = mountHook()

      emit('pty-a', toBytes('host will replay this'))

      expect(cachedWrite).not.toHaveBeenCalled()
      expect(mockAppendTranscript).toHaveBeenCalledWith('pty-a', 'host will replay this')
    })

    it('falls back to the transcript when no instance is cached', () => {
      const { emit } = mountHook()

      emit('pty-a', toBytes('cold restore'))

      expect(mockAppendTranscript).toHaveBeenCalledWith('pty-a', 'cold restore')
    })

    it('falls back to the transcript when the cached instance rejects the write', () => {
      mockPeekCachedTerminal.mockReturnValue({
        terminal: {
          write: vi.fn(() => {
            throw new Error('terminal disposed')
          })
        }
      })
      const { emit } = mountHook()

      emit('pty-a', toBytes('survives a dead sink'))

      expect(mockAppendTranscript).toHaveBeenCalledWith('pty-a', 'survives a dead sink')
    })
  })
})
