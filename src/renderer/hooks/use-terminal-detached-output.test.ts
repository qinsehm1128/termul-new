import { renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useTerminalDetachedOutput } from './use-terminal-detached-output'

const { mockOnData, mockOnExit, mockAppendTranscript, mockFindTerminalByPtyId } = vi.hoisted(
  () => ({
    mockOnData: vi.fn(),
    mockOnExit: vi.fn(),
    mockAppendTranscript: vi.fn(),
    mockFindTerminalByPtyId: vi.fn()
  })
)

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

vi.mock('@/stores/terminal-store', () => ({
  useTerminalStore: {
    getState: vi.fn(() => ({
      appendTranscript: mockAppendTranscript,
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
})
