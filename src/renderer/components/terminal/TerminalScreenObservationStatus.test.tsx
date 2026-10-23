import type { ScreenObservationStatus } from '@shared/types/terminal-screen.types'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useScreenObservationStore } from '@/stores/screen-observation-store'
import { useTerminalStore } from '@/stores/terminal-store'
import type { Terminal } from '@/types/project'
import { TerminalScreenObservationStatus } from './TerminalScreenObservationStatus'

const mocks = vi.hoisted(() => ({ status: vi.fn(), log: vi.fn() }))
vi.mock('@/lib/terminal-api', () => ({ readTerminalScreenStatus: mocks.status }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: mocks.log }))
const ready: ScreenObservationStatus = {
  terminalScreenEngine: 'ghostty-vt',
  state: 'ready',
  reason: null,
  capturedSeq: 3,
  appliedSeq: 3,
  observerBuildId: 'build'
}
const terminal = { id: 'tab', ptyId: 'pty', claim: 'claim', name: 'Terminal' } as Terminal

describe('read-only host screen status surface', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    useTerminalStore.setState({ terminals: [{ ...terminal }] })
    useScreenObservationStore.setState({ byPtyId: {} })
    mocks.status.mockResolvedValue({ success: true, data: ready })
    mocks.log.mockResolvedValue(undefined)
  })

  it('defaults to legacy unsupported and does not query on mount', () => {
    render(<TerminalScreenObservationStatus terminalId="tab" />)
    expect(mocks.status).not.toHaveBeenCalled()
    expect(screen.getByText(/This host does not provide/)).toBeInTheDocument()
    expect(screen.getByText(/Observation: Unsupported/)).toBeInTheDocument()
  })

  it('refreshes only by explicit action using the actual PTY claim', async () => {
    render(<TerminalScreenObservationStatus terminalId="tab" />)
    fireEvent.click(screen.getByText('Refresh host status'))
    await waitFor(() => expect(screen.getByText('Observation: Ready')).toBeInTheDocument())
    expect(mocks.status).toHaveBeenCalledExactlyOnceWith('pty', 'claim')
    expect(useScreenObservationStore.getState().byPtyId.pty.terminalScreenEngine).toBe('ghostty-vt')
  })

  it.each([
    'starting',
    'catching-up',
    'degraded',
    'unsupported'
  ] as const)('shows the real host state %s and reason without screen content', (state) => {
    useScreenObservationStore.getState().recordHost('pty', {
      terminalScreenEngine: 'ghostty-vt',
      status: { ...ready, state, reason: 'helper-missing' }
    })
    render(<TerminalScreenObservationStatus terminalId="tab" />)
    expect(screen.queryByText('Observation: Ready')).not.toBeInTheDocument()
    expect(screen.getByText('Observer helper is not available on this host.')).toBeInTheDocument()
    expect(mocks.status).not.toHaveBeenCalled()
  })

  it('does not keep a ready label after a failed explicit refresh and logs no error body', async () => {
    useScreenObservationStore
      .getState()
      .recordHost('pty', { terminalScreenEngine: 'ghostty-vt', status: ready })
    mocks.status.mockRejectedValue(new Error('PRIVATE PAYLOAD'))
    render(<TerminalScreenObservationStatus terminalId="tab" />)
    fireEvent.click(screen.getByText('Refresh host status'))
    await waitFor(() =>
      expect(screen.getByText(/Host observation is unavailable/)).toBeInTheDocument()
    )
    expect(screen.queryByText('Observation: Ready')).not.toBeInTheDocument()
    expect(JSON.stringify(mocks.log.mock.calls)).not.toContain('PRIVATE PAYLOAD')
  })

  it('a late ready reply cannot recover a degraded observer', async () => {
    let resolve!: (value: unknown) => void
    mocks.status.mockImplementation(
      () =>
        new Promise((done) => {
          resolve = done
        })
    )
    render(<TerminalScreenObservationStatus terminalId="tab" />)
    fireEvent.click(screen.getByText('Refresh host status'))
    act(() =>
      useScreenObservationStore.getState().recordHost('pty', {
        terminalScreenEngine: 'ghostty-vt',
        status: { ...ready, state: 'degraded', reason: 'overflow' }
      })
    )
    await act(async () => {
      resolve({ success: true, data: ready })
    })
    expect(screen.getByText('Observation: Degraded')).toBeInTheDocument()
  })

  it('rejects replies from a previous PTY/claim generation', async () => {
    let resolve!: (value: unknown) => void
    mocks.status.mockImplementation(
      () =>
        new Promise((done) => {
          resolve = done
        })
    )
    render(<TerminalScreenObservationStatus terminalId="tab" />)
    fireEvent.click(screen.getByText('Refresh host status'))
    act(() => useTerminalStore.setState({ terminals: [{ ...terminal, claim: 'new-claim' }] }))
    await act(async () => {
      resolve({ success: true, data: ready })
    })
    expect(screen.queryByText('Observation: Ready')).not.toBeInTheDocument()
    expect(useScreenObservationStore.getState().byPtyId.pty).toBeUndefined()
  })
})
