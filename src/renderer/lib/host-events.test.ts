import { listen } from '@tauri-apps/api/event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { onHostEvent } from './host-events'
import { logFrontendError } from './log-api'
import { isTauriContext } from './tauri-runtime'

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('./tauri-runtime', () => ({ isTauriContext: vi.fn(() => true) }))
vi.mock('./log-api', () => ({ logFrontendError: vi.fn(async () => {}) }))

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(isTauriContext).mockReturnValue(true)
})

describe('onHostEvent', () => {
  it('remains a synchronous disposer and resolves readiness after registration', async () => {
    let register!: (stop: () => void) => void
    vi.mocked(listen).mockReturnValue(new Promise((resolve) => (register = resolve)))
    const handler = vi.fn(),
      off = vi.fn()
    const stop = onHostEvent<number>('test:changed', handler)
    const ready = vi.fn()
    void stop.ready.then(ready)
    await Promise.resolve()
    expect(ready).not.toHaveBeenCalled()
    register(off)
    await stop.ready
    expect(ready).toHaveBeenCalledOnce()
    const callback = vi.mocked(listen).mock.calls[0]![1]
    callback({ event: 'test:changed', id: 1, payload: 7 })
    expect(handler).toHaveBeenCalledWith(7)
    stop()
    stop()
    callback({ event: 'test:changed', id: 1, payload: 8 })
    expect(off).toHaveBeenCalledOnce()
    expect(handler).toHaveBeenCalledOnce()
  })

  it('immediately unregisters if disposed before listener registration completes', async () => {
    let register!: (stop: () => void) => void
    vi.mocked(listen).mockReturnValue(new Promise((resolve) => (register = resolve)))
    const off = vi.fn()
    const stop = onHostEvent('test:changed', vi.fn())
    stop()
    register(off)
    await stop.ready
    expect(off).toHaveBeenCalledOnce()
  })

  it('exposes registration failure and still logs for legacy fire-and-forget callers', async () => {
    vi.mocked(listen).mockRejectedValue(new Error('registration failed'))
    const stop = onHostEvent('test:changed', vi.fn())
    await expect(stop.ready).rejects.toThrow('registration failed')
    expect(logFrontendError).toHaveBeenCalledWith(
      expect.objectContaining({
        source: 'host-events',
        message: expect.stringContaining('test:changed')
      })
    )
    stop()
  })

  it('returns an already-ready no-op in a browser without using Tauri', async () => {
    vi.mocked(isTauriContext).mockReturnValue(false)
    const stop = onHostEvent('test:changed', vi.fn())
    await stop.ready
    stop()
    expect(listen).not.toHaveBeenCalled()
  })
})
