import { beforeEach, describe, expect, it, vi } from 'vitest'
import { type CoreRuntimeStatus, coreRuntimeApi } from './core-runtime-api'

const { mockInvoke, mockRelaunch } = vi.hoisted(() => ({
  mockInvoke: vi.fn(),
  mockRelaunch: vi.fn()
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }))
vi.mock('./app-restart', () => ({ restartApp: mockRelaunch }))

function runtimeStatus(): CoreRuntimeStatus {
  const core = {
    mode: 'separate' as const,
    running: true,
    pid: 1234,
    startedAtMs: 1000,
    buildId: 'core-build',
    activeResources: 0
  }
  return {
    gui: { pid: 100, startedAtMs: null, buildId: 'gui-build', version: '1.2.3' },
    acpCore: { ...core },
    terminalCore: { ...core },
    pluginCore: { ...core, pid: 1235, buildId: 'plugin-build', activeResources: 2 },
    terminals: [],
    agents: []
  }
}

describe('coreRuntimeApi', () => {
  beforeEach(() => {
    mockInvoke.mockReset().mockResolvedValue(undefined)
    mockRelaunch.mockReset().mockResolvedValue(undefined)
  })

  it('reads Plugin Core status alongside the other processes', async () => {
    const status = runtimeStatus()
    mockInvoke.mockResolvedValue(status)

    expect(await coreRuntimeApi.status()).toEqual(status)
    expect(mockInvoke).toHaveBeenCalledWith('core_runtime_status')
    expect(status.pluginCore.activeResources).toBe(2)
  })

  it.each([
    'acpCore',
    'terminalCore',
    'pluginCore'
  ] as const)('restarts %s using the native restart role', async (role) => {
    await coreRuntimeApi.restartCore(role)

    expect(mockInvoke).toHaveBeenCalledExactlyOnceWith('restart_core', { role })
    expect(mockRelaunch).not.toHaveBeenCalled()
  })

  it('surfaces Plugin Core restart failures to the caller', async () => {
    mockInvoke.mockRejectedValue(new Error('Plugin Core is unavailable'))

    await expect(coreRuntimeApi.restartCore('pluginCore')).rejects.toThrow(
      'Plugin Core is unavailable'
    )
  })

  it('relaunches the GUI without restarting a Core', async () => {
    await coreRuntimeApi.restartGui()

    expect(mockRelaunch).toHaveBeenCalledTimes(1)
    expect(mockInvoke).not.toHaveBeenCalled()
  })
})
