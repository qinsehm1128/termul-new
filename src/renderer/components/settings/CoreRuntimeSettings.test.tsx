import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useAcpStore } from '@/features/agent-session/stores/acp-store'
import type { CoreProcessStatus, CoreRuntimeStatus } from '@/lib/core-runtime-api'
import { useTerminalStore } from '@/stores/terminal-store'
import { CoreRuntimeSettings } from './CoreRuntimeSettings'

const { mockStatus, mockRestartCore, mockRestartGui } = vi.hoisted(() => ({
  mockStatus: vi.fn(),
  mockRestartCore: vi.fn(),
  mockRestartGui: vi.fn()
}))

vi.mock('@/lib/core-runtime-api', () => ({
  coreRuntimeApi: {
    status: mockStatus,
    restartCore: mockRestartCore,
    restartGui: mockRestartGui
  }
}))

vi.mock('@/lib/log-api', () => ({
  logFrontendError: vi.fn()
}))

function core(overrides: Partial<CoreProcessStatus> = {}): CoreProcessStatus {
  return {
    mode: 'separate',
    running: true,
    pid: 97112,
    startedAtMs: Date.UTC(2026, 9, 11, 5, 49, 36),
    buildId: 'b1e2c3d4e5f6a7b8',
    activeResources: 0,
    ...overrides
  }
}

function runtime(overrides: Partial<CoreRuntimeStatus> = {}): CoreRuntimeStatus {
  return {
    gui: { pid: 34004, startedAtMs: null, buildId: 'gui-build', version: '0.15.4' },
    acpCore: core({ pid: 97103 }),
    terminalCore: core({ activeResources: 2 }),
    terminals: [
      {
        id: 'terminal-0',
        shell: '/bin/zsh',
        cwd: '/work/py_bi',
        pid: 99086,
        projectId: 'p1',
        active: true,
        lifecycle: 'running'
      },
      {
        id: 'terminal-1',
        shell: '/bin/zsh',
        cwd: '/work/ns-wms',
        pid: 65046,
        projectId: 'p2',
        active: true,
        lifecycle: 'running'
      }
    ],
    agents: [],
    ...overrides
  }
}

async function renderPanel(status: CoreRuntimeStatus = runtime()): Promise<void> {
  mockStatus.mockResolvedValue(status)
  render(<CoreRuntimeSettings />)
  await waitFor(() => {
    expect(document.querySelector('[data-runtime-card="terminalCore"]')).not.toBeNull()
  })
}

function dialogConfirmButton(): HTMLElement {
  const button = screen
    .getAllByRole('button', { name: 'Restart' })
    .find((candidate) => !candidate.dataset.testid)
  if (!button) throw new Error('confirm dialog is not open')
  return button
}

describe('CoreRuntimeSettings', () => {
  beforeEach(() => {
    mockStatus.mockReset()
    mockRestartCore.mockReset().mockResolvedValue(undefined)
    mockRestartGui.mockReset().mockResolvedValue(undefined)
    useTerminalStore.setState({
      terminals: [{ id: 't0', ptyId: 'terminal-0', name: 'Terminal 1', shell: '/bin/zsh' }]
    })
    useAcpStore.setState({ sessions: {}, configToLiveAgent: {} })
  })

  afterEach(() => {
    cleanup()
  })

  it('lists every PTY the Terminal Core holds and flags the ones no tab shows', async () => {
    await renderPanel()

    const shown = document.querySelector('[data-runtime-terminal="terminal-0"]')
    const hidden = document.querySelector('[data-runtime-terminal="terminal-1"]')
    expect(shown).toHaveTextContent('Terminal 1')
    expect(shown).toHaveTextContent('pid 99086')
    expect(shown).not.toHaveTextContent('No tab shows it')
    expect(hidden).toHaveTextContent('terminal-1')
    expect(hidden).toHaveTextContent('pid 65046')
    expect(hidden).toHaveTextContent('No tab shows it')
  })

  it('shows each process with its pid and whether it runs', async () => {
    await renderPanel(runtime({ acpCore: core({ running: false, pid: null }) }))

    expect(document.querySelector('[data-runtime-card="gui"]')).toHaveTextContent('pid 34004')
    expect(document.querySelector('[data-runtime-card="terminalCore"]')).toHaveAttribute(
      'data-runtime-state',
      'running'
    )
    expect(document.querySelector('[data-runtime-card="terminalCore"]')).toHaveTextContent(
      'pid 97112'
    )
    expect(document.querySelector('[data-runtime-card="acpCore"]')).toHaveAttribute(
      'data-runtime-state',
      'stopped'
    )
  })

  it('names live ACP agents after their agent config', async () => {
    useAcpStore.setState({ configToLiveAgent: { 'claude-code': 'agent-7' } })
    await renderPanel(runtime({ agents: ['agent-7'] }))

    expect(document.querySelector('[data-runtime-agent="agent-7"]')).toHaveTextContent(
      'claude-code'
    )
  })

  it('restarts the Terminal Core only after the user confirms, then re-reads the status', async () => {
    await renderPanel()

    fireEvent.click(screen.getByTestId('runtime-restart-terminalCore'))
    expect(screen.getByText(/Ends all 2 terminals/)).toBeInTheDocument()
    expect(mockRestartCore).not.toHaveBeenCalled()

    const readsBefore = mockStatus.mock.calls.length
    await act(async () => {
      fireEvent.click(dialogConfirmButton())
    })

    expect(mockRestartCore).toHaveBeenCalledWith('terminalCore')
    expect(mockRestartGui).not.toHaveBeenCalled()
    await waitFor(() => {
      expect(mockStatus.mock.calls.length).toBeGreaterThan(readsBefore)
    })
  })

  it('does nothing when the user cancels the restart', async () => {
    await renderPanel()

    fireEvent.click(screen.getByTestId('runtime-restart-acpCore'))
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))

    expect(mockRestartCore).not.toHaveBeenCalled()
  })

  it('restarts the GUI through relaunch, leaving the Cores alone', async () => {
    await renderPanel()

    fireEvent.click(screen.getByTestId('runtime-restart-gui'))
    await act(async () => {
      fireEvent.click(dialogConfirmButton())
    })

    expect(mockRestartGui).toHaveBeenCalledTimes(1)
    expect(mockRestartCore).not.toHaveBeenCalled()
  })

  it('cannot restart a Core that runs inside the app', async () => {
    await renderPanel(runtime({ acpCore: core({ mode: 'inProcess' }) }))

    expect(screen.getByTestId('runtime-restart-acpCore')).toBeDisabled()
    expect(screen.getByTestId('runtime-restart-terminalCore')).toBeEnabled()
  })

  it('shows the error when a restart fails', async () => {
    mockRestartCore.mockRejectedValue(new Error('the Core did not stop in time'))
    await renderPanel()

    fireEvent.click(screen.getByTestId('runtime-restart-terminalCore'))
    await act(async () => {
      fireEvent.click(dialogConfirmButton())
    })

    expect(await screen.findByTestId('runtime-error')).toHaveTextContent(
      'the Core did not stop in time'
    )
  })
})
