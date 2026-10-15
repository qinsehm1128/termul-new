import { beforeEach, describe, expect, it, vi } from 'vitest'

const { mockInvoke, runtime } = vi.hoisted(() => ({
  mockInvoke: vi.fn(),
  runtime: { desktop: true }
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }))
vi.mock('../tauri-runtime', () => ({ isTauriContext: () => runtime.desktop }))
vi.mock('../web-server-api', () => ({ webServerLog: {} }))

import { setLogRetentionDays } from '../log-api'

describe('setLogRetentionDays', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    mockInvoke.mockResolvedValue(undefined)
    runtime.desktop = true
  })

  it('hands the days to the desktop host', async () => {
    await setLogRetentionDays(14)

    expect(mockInvoke).toHaveBeenCalledWith('set_log_retention_days', { days: 14 })
  })

  it('does nothing in the web client', async () => {
    runtime.desktop = false

    await setLogRetentionDays(14)

    expect(mockInvoke).not.toHaveBeenCalled()
  })
})
