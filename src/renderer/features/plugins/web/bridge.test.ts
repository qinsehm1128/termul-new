import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { TauriPluginApi } from '../plugin-api'
import { createWebBridge } from './bridge'
import { webSession } from './test-fixtures'

let iframe: HTMLIFrameElement
let dispose: () => void
const requestWeb = vi.fn()
const readWebSession = vi.fn()
const onReady = vi.fn()
const onError = vi.fn()
let post: ReturnType<typeof vi.spyOn>
const base = {
  channel: 'se-plugin-ui',
  version: 1,
  sessionId: webSession.sessionId,
  nonce: webSession.nonce
}
function message(
  data: object,
  source: MessageEventSource | null = iframe.contentWindow,
  origin = 'null'
): void {
  window.dispatchEvent(
    new MessageEvent('message', { data: JSON.stringify({ ...base, ...data }), source, origin })
  )
}
function replies(): Record<string, unknown>[] {
  return post.mock.calls.map(([value]) => JSON.parse(value as string))
}
beforeEach(() => {
  vi.useFakeTimers()
  vi.clearAllMocks()
  iframe = document.createElement('iframe')
  document.body.append(iframe)
  post = vi.spyOn(iframe.contentWindow as Window, 'postMessage').mockImplementation(() => {})
  requestWeb.mockResolvedValue({ success: true, data: { count: 1 } })
  readWebSession.mockResolvedValue({
    success: true,
    data: { sequence: 0, resyncRequired: false, events: [] }
  })
  dispose = createWebBridge({
    iframe,
    session: webSession,
    api: { requestWeb, readWebSession } as unknown as TauriPluginApi,
    onReady,
    onError
  })
})
afterEach(() => {
  dispose()
  iframe.remove()
  document.documentElement.classList.remove('dark')
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe('plugin opaque iframe bridge', () => {
  it('expires a missing handshake after five seconds without forwarding requests', async () => {
    await vi.advanceTimersByTimeAsync(5000)
    expect(onError).toHaveBeenCalledExactlyOnceWith('PLUGIN_UI_HANDSHAKE_TIMEOUT')
    message({ type: 'ready' })
    expect(onReady).not.toHaveBeenCalled()
    expect(readWebSession).not.toHaveBeenCalled()
    expect(requestWeb).not.toHaveBeenCalled()
  })

  it('rejects forged source, nonce, origin, unhandshaken calls and malformed/oversize envelopes', async () => {
    const request = {
      type: 'request',
      requestId: 'one',
      method: 'invoke',
      params: { command: 'hello', args: {} }
    }
    message(request)
    message({ type: 'ready' }, window)
    message({ type: 'ready', nonce: 'f'.repeat(64) })
    message({ type: 'ready' }, iframe.contentWindow, 'https://attacker.example')
    message({ type: 'ready', extra: true })
    window.dispatchEvent(
      new MessageEvent('message', {
        data: { ...base, type: 'ready' },
        source: iframe.contentWindow,
        origin: 'null'
      })
    )
    message({ ...request, params: { body: 'x'.repeat(262145) } })
    expect(onReady).not.toHaveBeenCalled()
    expect(requestWeb).not.toHaveBeenCalled()
    expect(post).not.toHaveBeenCalled()
    message({ type: 'ready' })
    message({ ...request, nonce: 'f'.repeat(64) })
    message(request, window)
    await Promise.resolve()
    expect(onReady).toHaveBeenCalledOnce()
    expect(requestWeb).not.toHaveBeenCalled()
  })

  it('round trips host calls, forwards allow-list denials and ignores duplicate ids', async () => {
    message({ type: 'ready' })
    const request = {
      type: 'request',
      requestId: 'one',
      method: 'invoke',
      params: { command: 'hello', args: {} }
    }
    message(request)
    message(request)
    await Promise.resolve()
    expect(requestWeb).toHaveBeenCalledExactlyOnceWith({
      sessionId: webSession.sessionId,
      requestId: 'one',
      method: 'invoke',
      params: request.params
    })
    expect(replies()).toContainEqual({
      ...base,
      type: 'response',
      requestId: 'one',
      ok: true,
      result: { count: 1 }
    })
    requestWeb.mockResolvedValueOnce({
      success: false,
      code: 'PLUGIN_UI_NOT_ALLOWED',
      error: 'Command is not permitted'
    })
    message({ ...request, requestId: 'two' })
    await Promise.resolve()
    expect(replies()).toContainEqual({
      ...base,
      type: 'response',
      requestId: 'two',
      ok: false,
      error: { code: 'PLUGIN_UI_NOT_ALLOWED', message: 'Command is not permitted' }
    })
    expect(post.mock.calls.every(([, origin]) => origin === '*')).toBe(true)
  })

  it('posts themes and host UI/context/invalidation messages without navigating or reloading', async () => {
    readWebSession.mockResolvedValueOnce({
      success: true,
      data: {
        sequence: 3,
        resyncRequired: true,
        events: [
          { sequence: 1, type: 'message', payload: { kind: 'done' } },
          { sequence: 2, type: 'contextEvent', payload: { subscriptionId: 'sub' } },
          { sequence: 3, type: 'invalidate', payload: { viewId: 'web' } }
        ]
      }
    })
    const src = iframe.src
    message({ type: 'ready' })
    await Promise.resolve()
    expect(replies()).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          type: 'welcome',
          theme: expect.objectContaining({ appearance: 'light' })
        }),
        expect.objectContaining({ type: 'event', event: 'message', payload: { kind: 'done' } }),
        expect.objectContaining({ type: 'event', event: 'contextEvent' }),
        expect.objectContaining({ type: 'event', event: 'resyncRequired' })
      ])
    )
    document.documentElement.classList.add('dark')
    await Promise.resolve()
    expect(replies()).toContainEqual(
      expect.objectContaining({
        type: 'event',
        event: 'themeChanged',
        payload: expect.objectContaining({ appearance: 'dark' })
      })
    )
    expect(iframe.src).toBe(src)
  })

  it('limits pending calls, times out requests and revokes all late messages', async () => {
    requestWeb.mockReturnValue(new Promise(() => {}))
    message({ type: 'ready' })
    for (let n = 0; n < 33; n++)
      message({ type: 'request', requestId: `r${n}`, method: 'config.get', params: {} })
    expect(requestWeb).toHaveBeenCalledTimes(32)
    expect(replies()).toContainEqual(
      expect.objectContaining({
        requestId: 'r32',
        error: expect.objectContaining({ code: 'PLUGIN_UI_RATE_LIMITED' })
      })
    )
    await vi.advanceTimersByTimeAsync(30000)
    expect(replies()).toContainEqual(
      expect.objectContaining({
        requestId: 'r0',
        error: expect.objectContaining({ code: 'PLUGIN_UI_TIMEOUT' })
      })
    )
    dispose()
    const sent = post.mock.calls.length
    message({ type: 'request', requestId: 'late', method: 'config.get', params: {} })
    expect(post).toHaveBeenCalledTimes(sent)
    expect(requestWeb).toHaveBeenCalledTimes(32)
  })

  it('revokes on failed host poll and discards outstanding RPC completions', async () => {
    readWebSession.mockResolvedValueOnce({
      success: false,
      code: 'PLUGIN_UI_SESSION_EXPIRED',
      error: 'expired'
    })
    message({ type: 'ready' })
    await Promise.resolve()
    expect(onError).toHaveBeenCalledWith('PLUGIN_UI_SESSION_EXPIRED')
    const count = replies().length
    message({ type: 'request', requestId: 'late', method: 'config.get', params: {} })
    expect(replies()).toHaveLength(count)
    expect(requestWeb).not.toHaveBeenCalled()
  })
})
