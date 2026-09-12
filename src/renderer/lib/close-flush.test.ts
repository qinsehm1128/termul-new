import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const { mockLogFrontendError } = vi.hoisted(() => ({
  mockLogFrontendError: vi.fn(async () => undefined)
}))

vi.mock('./log-api', () => ({
  logFrontendError: mockLogFrontendError
}))

import { CLOSE_FLUSH_TIMEOUT_MS, runCloseFlush } from './close-flush'

describe('runCloseFlush', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('resolves "done" as soon as the work settles, without waiting out the budget', async () => {
    const outcome = await runCloseFlush('app-settings', Promise.resolve('ok'))
    expect(outcome).toBe('done')
    expect(mockLogFrontendError).not.toHaveBeenCalled()
  })

  it('gives up on a flush that never settles instead of blocking the close', async () => {
    // This is the whole point: `onCloseRequested` has already prevented the
    // close, so an unbounded flush reads to the user as a frozen window.
    const never = new Promise(() => {})
    const pending = runCloseFlush('pending-writes', never)

    await vi.advanceTimersByTimeAsync(CLOSE_FLUSH_TIMEOUT_MS)

    await expect(pending).resolves.toBe('timeout')
    expect(mockLogFrontendError).toHaveBeenCalledTimes(1)
    expect(mockLogFrontendError.mock.calls[0][0]).toMatchObject({
      source: 'workspace-close.flush',
      level: 'error'
    })
    expect(mockLogFrontendError.mock.calls[0][0].message).toContain('stage=pending-writes')
    expect(mockLogFrontendError.mock.calls[0][0].message).toContain(
      'stable_code=CLOSE_FLUSH_TIMEOUT'
    )
  })

  it('does not report a timeout before the budget is spent', async () => {
    const never = new Promise(() => {})
    const pending = runCloseFlush('acp-history', never, 1000)

    await vi.advanceTimersByTimeAsync(999)
    expect(mockLogFrontendError).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(1)
    await expect(pending).resolves.toBe('timeout')
  })

  it('degrades rather than rejects when the flush throws', async () => {
    const outcome = await runCloseFlush('session-index', Promise.reject(new Error('disk full')))

    expect(outcome).toBe('failed')
    expect(mockLogFrontendError).toHaveBeenCalledTimes(1)
    expect(mockLogFrontendError.mock.calls[0][0].message).toContain(
      'stable_code=CLOSE_FLUSH_FAILED'
    )
    expect(mockLogFrontendError.mock.calls[0][0].message).toContain('disk full')
  })

  it('clears its timer when the work wins, leaving no pending timers behind', async () => {
    await runCloseFlush('app-settings', Promise.resolve())
    expect(vi.getTimerCount()).toBe(0)
  })

  it('bounds each stage independently — one slow stage does not consume another budget', async () => {
    const slow = runCloseFlush('pending-writes', new Promise(() => {}), 500)
    const alsoSlow = runCloseFlush('acp-history', new Promise(() => {}), 500)

    await vi.advanceTimersByTimeAsync(500)

    await expect(slow).resolves.toBe('timeout')
    await expect(alsoSlow).resolves.toBe('timeout')
    // Both timed out at the same mark rather than serialising into 1000ms.
    expect(mockLogFrontendError).toHaveBeenCalledTimes(2)
  })
})
