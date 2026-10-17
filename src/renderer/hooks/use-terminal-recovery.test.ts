import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  recover: vi.fn(),
  loadQuick: vi.fn(() => Promise.resolve()),
  success: vi.fn(),
  error: vi.fn()
}))
vi.mock('@/lib/terminal-recovery', () => ({ recoverTerminalWorkspace: mocks.recover }))
vi.mock('@/features/quick-terminal/quick-terminal-store', () => ({
  useQuickTerminalStore: { getState: () => ({ load: mocks.loadQuick }) }
}))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn(() => Promise.resolve()) }))
vi.mock('sonner', () => ({ toast: { success: mocks.success, error: mocks.error } }))

import { recoverTerminals, reloadAndRecover } from './use-terminal-recovery'

beforeEach(() => vi.clearAllMocks())

describe('recoverTerminals', () => {
  it('runs one recovery for overlapping requests and refreshes quick terminals', async () => {
    let finish: (value: unknown) => void = () => undefined
    mocks.recover.mockReturnValue(new Promise((resolve) => (finish = resolve)))

    const first = recoverTerminals()
    const second = recoverTerminals()
    finish({ adopted: 2, tabsRestored: 3, disconnected: 1 })
    await Promise.all([first, second])

    expect(mocks.recover).toHaveBeenCalledTimes(1)
    expect(mocks.loadQuick).toHaveBeenCalledTimes(1)
    expect(mocks.success).toHaveBeenCalledWith(expect.stringContaining('Tabs back: 3'))
  })

  it('says so when everything was already connected', async () => {
    mocks.recover.mockResolvedValue({ adopted: 0, tabsRestored: 0, disconnected: 0 })
    await recoverTerminals()
    expect(mocks.success).toHaveBeenCalledWith('Everything is already connected.')
  })

  it('offers a reload when it cannot restore in place', async () => {
    mocks.recover.mockRejectedValue(new Error('core down'))
    await recoverTerminals()
    expect(mocks.error).toHaveBeenCalledWith('Could not restore in place.', {
      action: { label: 'Reload UI and restore', onClick: reloadAndRecover }
    })
  })
})
