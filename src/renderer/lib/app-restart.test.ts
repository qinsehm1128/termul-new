import { beforeEach, describe, expect, it, vi } from 'vitest'

const { mockInvoke, mockRelaunch, mockLog } = vi.hoisted(() => ({
  mockInvoke: vi.fn(),
  mockRelaunch: vi.fn(),
  mockLog: vi.fn()
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }))
vi.mock('@tauri-apps/plugin-process', () => ({ relaunch: mockRelaunch }))
vi.mock('./log-api', () => ({ logFrontendError: mockLog }))

import { restartApp } from './app-restart'

describe('restartApp', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    mockRelaunch.mockReset()
    mockLog.mockReset()
  })

  it('asks the host for a graceful restart instead of relaunching directly', async () => {
    mockInvoke.mockResolvedValue(undefined)

    await restartApp()

    expect(mockInvoke).toHaveBeenCalledWith('restart_app')
    expect(mockRelaunch).not.toHaveBeenCalled()
  })

  it('falls back to relaunch when the host lacks the command', async () => {
    mockInvoke.mockRejectedValue(new Error('command restart_app not found'))
    mockRelaunch.mockResolvedValue(undefined)

    await restartApp()

    expect(mockRelaunch).toHaveBeenCalledTimes(1)
    expect(mockLog).toHaveBeenCalledWith(
      expect.objectContaining({ level: 'warn', source: 'app-restart' })
    )
  })
})
