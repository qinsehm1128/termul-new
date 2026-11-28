import type { RemoteStatus } from '@shared/types/ipc.types'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useRemoteStatusStore } from './remote-status-store'

const status = (overrides: Partial<RemoteStatus> = {}): RemoteStatus => ({
  running: true,
  url: 'http://127.0.0.1:4711',
  port: 4711,
  bindMode: 'localhost',
  bindHost: '127.0.0.1',
  tunnelUrl: null,
  ...overrides
})

/**
 * `use-remote-projects` polls the host every 3 s and the backend returns a new
 * object each time. A poll that learned nothing must not wake any subscriber:
 * the status is read by the status bar and every remote-access surface.
 */
describe('remote-status-store', () => {
  beforeEach(() => {
    useRemoteStatusStore.setState({ status: null, restoreError: null })
  })

  it('notifies once for a changed payload and not at all for an identical poll', () => {
    const listener = vi.fn()
    const unsubscribe = useRemoteStatusStore.subscribe(listener)
    try {
      useRemoteStatusStore.getState().setStatus(status())
      expect(listener).toHaveBeenCalledTimes(1)

      // Same values, fresh object — the shape of every idle 3 s poll.
      useRemoteStatusStore.getState().setStatus(status())
      useRemoteStatusStore.getState().setStatus(status())
      expect(listener).toHaveBeenCalledTimes(1)

      useRemoteStatusStore.getState().setStatus(status({ port: 4712 }))
      expect(listener).toHaveBeenCalledTimes(2)
    } finally {
      unsubscribe()
    }
  })

  it('treats a newly-appearing optional field as a change', () => {
    useRemoteStatusStore.getState().setStatus(status())
    const listener = vi.fn()
    const unsubscribe = useRemoteStatusStore.subscribe(listener)
    try {
      useRemoteStatusStore.getState().setStatus(status({ tailnetUrl: 'http://100.64.0.2:4711' }))
      expect(listener).toHaveBeenCalledTimes(1)
    } finally {
      unsubscribe()
    }
  })

  it('does not notify for a repeated restore error', () => {
    useRemoteStatusStore.getState().setRestoreError('boom')
    const listener = vi.fn()
    const unsubscribe = useRemoteStatusStore.subscribe(listener)
    try {
      useRemoteStatusStore.getState().setRestoreError('boom')
      expect(listener).not.toHaveBeenCalled()
      useRemoteStatusStore.getState().setRestoreError(null)
      expect(listener).toHaveBeenCalledTimes(1)
    } finally {
      unsubscribe()
    }
  })
})
