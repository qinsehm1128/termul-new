import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { _resetSystemHeartbeatForTesting, createTauriSystemApi } from '../tauri-system-api'

/**
 * `onPowerResume` is subscribed once per mounted terminal. The heartbeat that
 * implements it must therefore be a single shared timer with reference
 * counting: one interval per subscriber leaks a timer for every terminal ever
 * mounted, and the leaked callback retains that terminal's component instance
 * (and its xterm buffer) for the lifetime of the renderer.
 */
describe('tauri system api power-resume heartbeat', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    _resetSystemHeartbeatForTesting()
  })

  afterEach(() => {
    _resetSystemHeartbeatForTesting()
    vi.useRealTimers()
  })

  it('runs one shared interval for many subscribers and clears it with the last unsubscribe', () => {
    const api = createTauriSystemApi()
    const off1 = api.onPowerResume(() => {})
    const off2 = api.onPowerResume(() => {})
    const off3 = api.onPowerResume(() => {})

    expect(vi.getTimerCount()).toBe(1)

    off1()
    off2()
    expect(vi.getTimerCount()).toBe(1)

    off3()
    expect(vi.getTimerCount()).toBe(0)
  })

  it('keeps delivering resume to the subscribers that are still attached', () => {
    const api = createTauriSystemApi()
    const first = vi.fn()
    const second = vi.fn()
    const offFirst = api.onPowerResume(first)
    api.onPowerResume(second)

    offFirst()
    // Simulate a sleep: wall clock jumps while the timer does not tick.
    vi.setSystemTime(Date.now() + 5 * 60_000)
    vi.advanceTimersByTime(30_000)

    expect(first).not.toHaveBeenCalled()
    expect(second).toHaveBeenCalledTimes(1)
  })

  it('leaves no interval behind when subscribers unmount in mount order', () => {
    const api = createTauriSystemApi()
    const cleanups: Array<() => void> = []
    for (let index = 0; index < 5; index += 1) {
      cleanups.push(api.onPowerResume(() => {}))
    }
    // React unmount order is not the subscribe order; every path must release.
    for (const cleanup of cleanups) cleanup()
    expect(vi.getTimerCount()).toBe(0)
  })
})
