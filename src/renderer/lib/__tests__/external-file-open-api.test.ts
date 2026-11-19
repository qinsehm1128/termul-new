/**
 * Dual-branch tests for external-file-open-api.ts.
 *
 * Desktop (`isTauriContext() === true`) invokes `take_pending_external_files`
 * and listens on `external-file-open`; the web client returns `[]`, never calls
 * the Tauri APIs, and its cleanup is a no-op. Follows the
 * `tauri-opener-api.web.test.ts` hoisted-mock pattern.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const { mockInvoke, mockListen, mockIsTauriContext } = vi.hoisted(() => ({
  mockInvoke: vi.fn(),
  mockListen: vi.fn(),
  mockIsTauriContext: vi.fn()
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }))

vi.mock('@tauri-apps/api/event', () => ({ listen: mockListen }))

vi.mock('../tauri-runtime', () => ({
  isTauriContext: mockIsTauriContext,
  cleanupTauriListener: (unlisten: unknown) => {
    if (typeof unlisten === 'function') unlisten()
    else if (unlisten && typeof (unlisten as Promise<unknown>).then === 'function') {
      void (unlisten as Promise<() => void>).then((dispose) => dispose()).catch(() => {})
    }
  }
}))

vi.mock('../log-api', () => ({
  logFrontendError: vi.fn()
}))

import {
  EXTERNAL_FILE_OPEN_EVENT,
  onExternalFileOpen,
  supportsExternalFileOpen,
  takePendingExternalFiles
} from '../external-file-open-api'

describe('external-file-open-api', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockListen.mockReturnValue(Promise.resolve(() => {}))
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('web: supportsExternalFileOpen is false and no Tauri API is touched', async () => {
    mockIsTauriContext.mockReturnValue(false)

    expect(supportsExternalFileOpen()).toBe(false)
    await expect(takePendingExternalFiles()).resolves.toEqual([])
    expect(mockInvoke).not.toHaveBeenCalled()

    const cleanup = onExternalFileOpen(() => {
      throw new Error('must not fire on web')
    })
    expect(typeof cleanup).toBe('function')
    expect(() => cleanup()).not.toThrow()
    expect(mockListen).not.toHaveBeenCalled()
  })

  it('desktop: takePendingExternalFiles invokes the host command and filters non-strings', async () => {
    mockIsTauriContext.mockReturnValue(true)
    mockInvoke.mockResolvedValue(['/a/one.md', 42, '', '/b/two.html'])

    expect(supportsExternalFileOpen()).toBe(true)
    await expect(takePendingExternalFiles()).resolves.toEqual(['/a/one.md', '/b/two.html'])
    expect(mockInvoke).toHaveBeenCalledWith('take_pending_external_files')
  })

  it('desktop: a host failure degrades to an empty list instead of throwing', async () => {
    mockIsTauriContext.mockReturnValue(true)
    mockInvoke.mockRejectedValue(new Error('no pending state'))

    await expect(takePendingExternalFiles()).resolves.toEqual([])
  })

  it('desktop: onExternalFileOpen listens on the event and forwards paths', async () => {
    mockIsTauriContext.mockReturnValue(true)
    let handler: ((event: { payload: { paths?: unknown } }) => void) | undefined
    const unlisten = vi.fn()
    mockListen.mockImplementation((eventName: string, registered: typeof handler) => {
      expect(eventName).toBe(EXTERNAL_FILE_OPEN_EVENT)
      handler = registered
      return Promise.resolve(unlisten)
    })

    const received: string[][] = []
    const cleanup = onExternalFileOpen((paths) => received.push(paths))
    expect(mockListen).toHaveBeenCalledTimes(1)

    handler?.({ payload: { paths: ['/x/a.md', 7] } })
    handler?.({ payload: { paths: [] } })
    handler?.({ payload: {} })

    expect(received).toEqual([['/x/a.md']])

    cleanup()
    await Promise.resolve()
    expect(unlisten).toHaveBeenCalled()
  })
})
