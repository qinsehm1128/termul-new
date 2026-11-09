import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  _resetRemoteAccessCredentialForTests,
  setRemoteAccessCredential
} from './remote-access-credential'
import { webServerFilesystem, webServerMcpServers } from './web-server-api'

vi.mock('./tauri-runtime', () => ({ isTauriContext: () => false }))

describe('paired-device common HTTP transport', () => {
  const fetchMock = vi.fn()
  beforeEach(() => {
    _resetRemoteAccessCredentialForTests()
    setRemoteAccessCredential('paired-device-test-credential')
    fetchMock.mockReset().mockResolvedValue({
      ok: true,
      json: async () => ({ success: true, data: null })
    })
    vi.stubGlobal('fetch', fetchMock)
  })
  afterEach(() => {
    vi.unstubAllGlobals()
    _resetRemoteAccessCredentialForTests()
  })

  it('sends the redeemed bearer on GET, POST and PUT without changing JSON content type', async () => {
    await webServerFilesystem.readFile('/project/file')
    await webServerFilesystem.writeFile('/project/file', 'hello')
    await webServerMcpServers.put({ servers: [] })
    expect(fetchMock).toHaveBeenCalledTimes(3)
    expect(fetchMock.mock.calls.map(([, options]) => options.method)).toEqual([
      'GET',
      'POST',
      'PUT'
    ])
    for (const [, options] of fetchMock.mock.calls) {
      expect(new Headers(options.headers).get('authorization')).toBe(
        'Bearer paired-device-test-credential'
      )
    }
    expect(new Headers(fetchMock.mock.calls[1][1].headers).get('content-type')).toBe(
      'application/json'
    )
  })

  it.each([
    [403, { code: 'FORBIDDEN', capability: 'files.write' }, 'FORBIDDEN'],
    [401, { code: 'DEVICE_REVOKED', error: 'pair again' }, 'DEVICE_REVOKED'],
    [401, { code: 'INVALID_CREDENTIAL' }, 'INVALID_CREDENTIAL'],
    [429, { code: 'RATE_LIMITED' }, 'RATE_LIMITED']
  ])('preserves stable permission failures (%s)', async (status, body, code) => {
    fetchMock.mockResolvedValueOnce({ ok: false, status, json: async () => body })
    const result = await webServerFilesystem.writeFile('/project/file', 'hello')
    expect(result.success).toBe(false)
    if (!result.success) expect(result.code).toBe(code)
  })

  it('keeps unrelated HTTP failures mapped to the existing network error contract', async () => {
    fetchMock.mockResolvedValueOnce({
      ok: false,
      status: 500,
      statusText: 'Internal Server Error',
      json: async () => {
        throw new Error('not JSON')
      }
    })
    expect(await webServerFilesystem.readFile('/project/file')).toEqual({
      success: false,
      code: 'NETWORK_ERROR',
      error: 'HTTP 500 Internal Server Error'
    })
  })
})
