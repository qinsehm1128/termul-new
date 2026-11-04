import type { IpcResult } from '@shared/types/ipc.types'
import type { PluginInfo } from '@shared/types/plugin.types'
import type { HostProcessStatus } from '@shared/types/plugin-host.types'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { pluginApi } from '../plugin-api'
import { PluginProcesses } from './PluginProcesses'

vi.mock('../plugin-api', () => ({
  pluginApi: { listProcesses: vi.fn(), getProcessLogs: vi.fn(), controlProcess: vi.fn() }
}))
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }))

const status: HostProcessStatus = {
  state: 'running',
  pid: 1234,
  restarts: 2,
  startedAt: 123456
}
function fixture(): PluginInfo {
  return {
    id: 'org.demo',
    desiredEnabled: true,
    state: 'active',
    generation: 'g1',
    configRevision: 1,
    sha256: '',
    lastError: null,
    manifest: {
      schemaVersion: 1,
      id: 'org.demo',
      name: 'Demo',
      version: '1.0.0',
      abi: { min: 1, max: 1 },
      services: { provides: [], requires: [] },
      contributes: {
        commands: [],
        views: [],
        configuration: { title: '', properties: {} },
        processes: [
          {
            id: 'worker',
            command: 'bin/worker',
            args: [],
            env: {},
            restart: 'never',
            autostart: true
          }
        ]
      }
    }
  }
}
const flush = async () => {
  await act(async () => {
    await Promise.resolve()
  })
}

beforeEach(() => {
  vi.useFakeTimers()
  vi.clearAllMocks()
  vi.mocked(pluginApi.listProcesses).mockResolvedValue({
    success: true,
    data: { processes: [{ id: 'worker', status }] }
  })
  vi.mocked(pluginApi.getProcessLogs).mockResolvedValue({
    success: true,
    data: {
      lines: [{ stream: 'stdout', at: 123456, text: '<script>literal output</script>' }],
      truncated: true
    }
  })
  vi.mocked(pluginApi.controlProcess).mockResolvedValue({ success: true, data: status })
})
afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('PluginProcesses', () => {
  it('shows declared IDs, live status, PID, restarts and escaped bounded tail snapshots', async () => {
    const { container } = render(<PluginProcesses plugin={fixture()} connected />)
    await flush()
    expect(screen.getByText('worker')).toBeInTheDocument()
    expect(screen.getByText('processes.states.running')).toBeInTheDocument()
    expect(screen.getByText('1234')).toBeInTheDocument()
    expect(screen.getByText('2')).toBeInTheDocument()
    expect(screen.getByText('[stdout] <script>literal output</script>')).toBeInTheDocument()
    expect(container.querySelector('script')).toBeNull()
    expect(screen.getByText('processes.truncated')).toBeInTheDocument()
    expect(pluginApi.getProcessLogs).toHaveBeenCalledWith({
      id: 'org.demo',
      generation: 'g1',
      processId: 'worker',
      tail: 200
    })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000)
    })
    expect(pluginApi.listProcesses).toHaveBeenCalledTimes(2)
    expect(screen.getAllByText('[stdout] <script>literal output</script>')).toHaveLength(1)
  })

  it('stops polling on unmount and never overlaps a slow poll', async () => {
    let resolve!: (
      reply: IpcResult<{ processes: { id: string; status: HostProcessStatus }[] }>
    ) => void
    vi.mocked(pluginApi.listProcesses).mockReturnValue(
      new Promise((done) => {
        resolve = done
      })
    )
    const { unmount } = render(<PluginProcesses plugin={fixture()} connected />)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10000)
    })
    expect(pluginApi.listProcesses).toHaveBeenCalledTimes(1)
    unmount()
    await act(async () => {
      resolve({ success: true, data: { processes: [{ id: 'worker', status }] } })
    })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10000)
    })
    expect(pluginApi.listProcesses).toHaveBeenCalledTimes(1)
  })

  it.each([
    'start',
    'stop',
    'restart'
  ] as const)('scopes %s control to plugin generation and process', async (action) => {
    const initial = {
      ...status,
      state: action === 'start' ? ('stopped' as const) : ('running' as const)
    }
    vi.mocked(pluginApi.listProcesses).mockResolvedValue({
      success: true,
      data: { processes: [{ id: 'worker', status: initial }] }
    })
    render(<PluginProcesses plugin={fixture()} connected />)
    await flush()
    fireEvent.click(screen.getByRole('button', { name: `processes.${action}` }))
    await flush()
    expect(pluginApi.controlProcess).toHaveBeenCalledWith({
      id: 'org.demo',
      generation: 'g1',
      processId: 'worker',
      action
    })
  })

  it('blocks concurrent controls and reports rejected or failed controls', async () => {
    let reject!: (error: Error) => void
    vi.mocked(pluginApi.controlProcess).mockReturnValue(
      new Promise((_, fail) => {
        reject = fail
      })
    )
    render(<PluginProcesses plugin={fixture()} connected />)
    await flush()
    fireEvent.click(screen.getByRole('button', { name: 'processes.restart' }))
    expect(screen.getByRole('button', { name: 'processes.stop' })).toBeDisabled()
    await act(async () => {
      reject(new Error('host disconnected'))
    })
    expect(screen.getByRole('alert')).toHaveTextContent('processes.error')
    expect(screen.getByRole('button', { name: 'processes.stop' })).toBeEnabled()
  })

  it.each([
    { connected: false },
    { connected: true, disabled: true },
    { connected: true, state: 'disabled' as const },
    { connected: true, desiredEnabled: false },
    { connected: true, generation: null }
  ])('has no polling or control side effects when unavailable: %j', async ({
    connected,
    disabled,
    ...overrides
  }) => {
    render(
      <PluginProcesses
        plugin={{ ...fixture(), ...overrides }}
        connected={connected}
        disabled={disabled}
      />
    )
    await flush()
    expect(pluginApi.listProcesses).not.toHaveBeenCalled()
    expect(pluginApi.getProcessLogs).not.toHaveBeenCalled()
    expect(screen.getByRole('status')).toHaveTextContent('processes.unavailable')
    for (const button of screen.getAllByRole('button')) expect(button).toBeDisabled()
  })

  it('shows an empty declaration without polling', async () => {
    const plugin = fixture()
    plugin.manifest.contributes.processes = []
    render(<PluginProcesses plugin={plugin} connected />)
    await flush()
    expect(screen.getByText('processes.empty')).toBeInTheDocument()
    expect(pluginApi.listProcesses).not.toHaveBeenCalled()
  })

  it('discards stale generation results and clears old logs when disconnected', async () => {
    let resolve!: (
      reply: IpcResult<{ processes: { id: string; status: HostProcessStatus }[] }>
    ) => void
    vi.mocked(pluginApi.listProcesses).mockReturnValueOnce(
      new Promise((done) => {
        resolve = done
      })
    )
    const plugin = fixture()
    const { rerender } = render(<PluginProcesses plugin={plugin} connected />)
    rerender(<PluginProcesses plugin={{ ...plugin, generation: 'g2' }} connected />)
    await flush()
    await act(async () => {
      resolve({
        success: true,
        data: { processes: [{ id: 'worker', status: { ...status, pid: 9999 } }] }
      })
    })
    expect(screen.queryByText('9999')).toBeNull()
    expect(screen.getByText('1234')).toBeInTheDocument()
    rerender(<PluginProcesses plugin={{ ...plugin, generation: 'g2' }} connected={false} />)
    expect(screen.queryByText('1234')).toBeNull()
    expect(screen.queryByText('[stdout] <script>literal output</script>')).toBeNull()
  })

  it('clears stale status on failures and recovers on the next refresh', async () => {
    render(<PluginProcesses plugin={fixture()} connected />)
    await flush()
    vi.mocked(pluginApi.listProcesses).mockResolvedValueOnce({
      success: false,
      error: 'unavailable',
      code: 'PLUGIN_CORE_DISCONNECTED'
    })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000)
    })
    expect(screen.getByRole('alert')).toBeInTheDocument()
    expect(screen.queryByText('1234')).toBeNull()
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000)
    })
    expect(screen.queryByRole('alert')).toBeNull()
    expect(screen.getByText('1234')).toBeInTheDocument()
  })
})
