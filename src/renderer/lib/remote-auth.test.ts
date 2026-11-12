import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  _resetRemoteAccessCredentialForTests,
  getRemoteAccessCredential,
  remoteAccessHeaders,
  setRemoteAccessCredential
} from './remote-access-credential'
import { initializeRemoteAuth } from './remote-auth'

const log = vi.hoisted(() => vi.fn())
vi.mock('./log-api', () => ({ logFrontendError: log }))
const fetchMock = vi.fn()
beforeEach(() => {
  vi.clearAllMocks()
  _resetRemoteAccessCredentialForTests()
  window.history.replaceState(null, '', '/')
  vi.stubGlobal('fetch', fetchMock)
  vi.spyOn(navigator, 'platform', 'get').mockReturnValue('MacIntel')
  log.mockResolvedValue(undefined)
})
afterEach(() => {
  vi.restoreAllMocks()
  vi.useRealTimers()
})
const reply = (): Response =>
  new Response(
    JSON.stringify({ deviceToken: 'DEVICE_TOKEN', deviceId: 'device-1', scopes: ['view'] })
  )

describe('browser pairing bootstrap', () => {
  it('strips #pair synchronously, redeems it without auth and installs token before consumers run', async () => {
    window.history.replaceState(null, '', '/?project=p1#pair=ONE_TIME_CODE')
    const storage = vi.spyOn(Storage.prototype, 'setItem')
    let finish!: (response: Response) => void
    fetchMock.mockImplementationOnce(
      () =>
        new Promise<Response>((resolve) => {
          finish = resolve
        })
    )
    const ready = vi.fn(() => remoteAccessHeaders().get('authorization'))
    const pending = initializeRemoteAuth().then(ready)
    expect(window.location.hash).toBe('')
    expect(window.location.search).toBe('?project=p1')
    expect(ready).not.toHaveBeenCalled()
    expect(getRemoteAccessCredential()).toBe('')
    expect(fetchMock).toHaveBeenCalledWith(`${window.location.origin}/pair`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ code: 'ONE_TIME_CODE', name: 'MacIntel 浏览器', platform: 'web' }),
      signal: expect.any(AbortSignal)
    })
    finish(reply())
    await pending
    expect(ready).toHaveReturnedWith('Bearer DEVICE_TOKEN')
    expect(storage).not.toHaveBeenCalled()
    expect(window.location.href).not.toContain('DEVICE_TOKEN')
    expect(window.location.href).not.toContain('ONE_TIME_CODE')
    expect(JSON.stringify(log.mock.calls)).not.toContain('DEVICE_TOKEN')
    expect(JSON.stringify(log.mock.calls)).not.toContain('ONE_TIME_CODE')
  })
  it('uses UA platform as a short default name when navigator.platform is unavailable', async () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('')
    vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue('Mozilla Android Chrome')
    window.history.replaceState(null, '', '/#pair=CODE')
    fetchMock.mockResolvedValueOnce(reply())
    await initializeRemoteAuth()
    expect(JSON.parse(fetchMock.mock.calls[0][1].body)).toEqual({
      code: 'CODE',
      name: 'Android 浏览器',
      platform: 'web'
    })
  })
  it.each([
    'access_token',
    'token'
  ])('keeps legacy #%s links working without a pairing request', async (key) => {
    window.history.replaceState(null, '', `/#${key}=LEGACY_TOKEN`)
    await initializeRemoteAuth()
    expect(getRemoteAccessCredential()).toBe('LEGACY_TOKEN')
    expect(window.location.hash).toBe('')
    expect(fetchMock).not.toHaveBeenCalled()
  })
  it('opens the requested in-app page after consuming a native client credential', async () => {
    window.history.replaceState(null, '', '/#access_token=DEVICE_TOKEN&route=/plugins')
    await initializeRemoteAuth()
    expect(getRemoteAccessCredential()).toBe('DEVICE_TOKEN')
    expect(window.location.hash).toBe('#/plugins')
    expect(window.location.href).not.toContain('DEVICE_TOKEN')
  })
  it.each([
    'https://evil.example',
    '//evil.example',
    '/plugins?x=1',
    '/plugins#a',
    '/../x',
    'plugins'
  ])('drops unsafe landing route %s', async (route) => {
    window.history.replaceState(null, '', `/#access_token=T&route=${encodeURIComponent(route)}`)
    await initializeRemoteAuth()
    expect(window.location.hash).toBe('')
  })
  it('keeps an existing in-memory credential on normal navigation', async () => {
    setRemoteAccessCredential('EXISTING')
    window.history.replaceState(null, '', '/#section')
    await initializeRemoteAuth()
    expect(remoteAccessHeaders().get('authorization')).toBe('Bearer EXISTING')
    expect(window.location.hash).toBe('#section')
    expect(fetchMock).not.toHaveBeenCalled()
  })
  it.each([
    400, 401, 429, 500
  ])('fails closed on HTTP %s and never falls back to a host token', async (status) => {
    setRemoteAccessCredential('OLD_HOST_TOKEN')
    window.history.replaceState(null, '', '/#pair=SECRET_CODE&access_token=OLD_HOST_TOKEN')
    fetchMock.mockResolvedValueOnce(new Response('SECRET_RESPONSE', { status }))
    await expect(initializeRemoteAuth()).rejects.toThrow('配对失败，请重新扫码')
    expect(getRemoteAccessCredential()).toBe('')
    expect(window.location.hash).toBe('')
    expect(log).toHaveBeenCalledWith({ source: 'remote-auth', message: 'browser pairing failed' })
    expect(JSON.stringify(log.mock.calls)).not.toMatch(/SECRET|OLD_HOST/)
  })
  it.each([
    null,
    {},
    { deviceToken: 42 },
    { deviceToken: '' }
  ])('rejects malformed exchange responses %j', async (payload) => {
    window.history.replaceState(null, '', '/#pair=CODE')
    fetchMock.mockResolvedValueOnce(new Response(JSON.stringify(payload)))
    await expect(initializeRemoteAuth()).rejects.toThrow('配对失败，请重新扫码')
    expect(getRemoteAccessCredential()).toBe('')
  })
  it('redacts network errors and rejects empty codes without a request', async () => {
    window.history.replaceState(null, '', '/#pair=CODE')
    fetchMock.mockRejectedValueOnce(new Error('CODE DEVICE_TOKEN'))
    await expect(initializeRemoteAuth()).rejects.toThrow('配对失败，请重新扫码')
    window.history.replaceState(null, '', '/#pair=')
    await expect(initializeRemoteAuth()).rejects.toThrow('配对失败，请重新扫码')
    expect(fetchMock).toHaveBeenCalledTimes(1)
    expect(JSON.stringify(log.mock.calls)).not.toMatch(/CODE|DEVICE_TOKEN/)
  })
  it('bounds exchange time and fails closed on timeout', async () => {
    vi.useFakeTimers()
    window.history.replaceState(null, '', '/#pair=CODE')
    fetchMock.mockImplementationOnce(
      (_url, options: RequestInit) =>
        new Promise((_resolve, reject) => {
          options.signal?.addEventListener('abort', () => reject(new Error('timeout')))
        })
    )
    const pending = expect(initializeRemoteAuth()).rejects.toThrow('配对失败，请重新扫码')
    await vi.advanceTimersByTimeAsync(15000)
    await pending
    expect(getRemoteAccessCredential()).toBe('')
    expect(vi.getTimerCount()).toBe(0)
  })
})
