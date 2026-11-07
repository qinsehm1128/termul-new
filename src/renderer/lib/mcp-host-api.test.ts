import { emptyMcpHostAccessPolicy } from '@shared/types/mcp-host.types'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { callMcpHost, getMcpHostAccess, putMcpHostAccess } from './mcp-host-api'
import { _resetRemoteAccessCredentialForTests } from './remote-access-credential'

const { invoke, tauri, logger } = vi.hoisted(() => ({
  invoke: vi.fn(),
  tauri: vi.fn(() => false),
  logger: vi.fn()
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke }))
vi.mock('./tauri-runtime', () => ({ isTauriContext: tauri }))
vi.mock('./log-api', () => ({ logFrontendError: logger }))
const fetchMock = vi.fn()
const ok = (data: unknown) => ({
  ok: true,
  status: 200,
  json: async () => ({ success: true, data })
})

beforeEach(() => {
  vi.clearAllMocks()
  tauri.mockReturnValue(false)
  vi.stubGlobal('fetch', fetchMock)
  window.history.replaceState(null, '', '/#access_token=test-credential')
  _resetRemoteAccessCredentialForTests()
})

describe('host access facade', () => {
  it('defaults to deny all', () => {
    expect(emptyMcpHostAccessPolicy()).toEqual({
      schemaVersion: 1,
      revision: 0,
      enabled: false,
      workspace: { enabled: false, projectIds: [], includePaths: false },
      ssh: { profiles: {} },
      profileFingerprints: {}
    })
  })
  it('uses frozen desktop commands and argument envelopes', async () => {
    tauri.mockReturnValue(true)
    invoke.mockResolvedValue({ success: true, data: emptyMcpHostAccessPolicy() })
    const policy = emptyMcpHostAccessPolicy()
    await getMcpHostAccess()
    await putMcpHostAccess(7, policy)
    await callMcpHost({
      provider: 'ssh',
      tool: 'job_cancel',
      arguments: { profileId: 'p', jobId: 'j' }
    })
    expect(invoke.mock.calls).toEqual([
      ['mcp_host_access_get', undefined],
      ['mcp_host_access_put', { expectedRevision: 7, policy }],
      [
        'mcp_host_call',
        { provider: 'ssh', tool: 'job_cancel', arguments: { profileId: 'p', jobId: 'j' } }
      ]
    ])
    expect(fetchMock).not.toHaveBeenCalled()
  })
  it('authenticates all web requests without putting credentials into URLs or bodies', async () => {
    const policy = emptyMcpHostAccessPolicy()
    fetchMock.mockResolvedValue(ok(policy))
    await getMcpHostAccess()
    await putMcpHostAccess(2, policy)
    const call = {
      provider: 'ssh' as const,
      tool: 'test_connection',
      arguments: { profileId: 'p' }
    }
    await callMcpHost(call)
    expect(fetchMock.mock.calls.map(([url, init]) => [url, init.method])).toEqual([
      [`${window.location.origin}/mcp/host/access`, 'GET'],
      [`${window.location.origin}/mcp/host/access`, 'PUT'],
      [`${window.location.origin}/mcp/host/call`, 'POST']
    ])
    for (const [url, init] of fetchMock.mock.calls) {
      expect(init.headers.get('authorization')).toBe('Bearer test-credential')
      expect(url).not.toContain('test-credential')
      expect(init.body ?? '').not.toContain('test-credential')
    }
    expect(JSON.parse(fetchMock.mock.calls[1][1].body)).toEqual({ expectedRevision: 2, policy })
    expect(JSON.parse(fetchMock.mock.calls[2][1].body)).toEqual(call)
  })
  it.each([
    [401, 'UNAUTHORIZED'],
    [403, 'FORBIDDEN'],
    [409, 'REVISION_CONFLICT'],
    [500, 'NETWORK_ERROR']
  ])('handles HTTP %s as %s', async (status, code) => {
    fetchMock.mockResolvedValue({ ok: false, status })
    expect(await getMcpHostAccess()).toMatchObject({ success: false, code })
  })
  it('preserves typed IPC conflicts', async () => {
    tauri.mockReturnValue(true)
    invoke.mockResolvedValue({ success: false, code: 'REVISION_CONFLICT', error: 'Conflict' })
    expect(await putMcpHostAccess(0, emptyMcpHostAccessPolicy())).toMatchObject({
      success: false,
      code: 'REVISION_CONFLICT'
    })
  })
  it.each([
    false,
    true
  ])('does not throw or log sensitive exception/arguments (desktop %s)', async (desktop) => {
    tauri.mockReturnValue(desktop)
    invoke.mockRejectedValue(new Error('secret command /private/path'))
    fetchMock.mockRejectedValue(new Error('secret command /private/path'))
    expect(
      await callMcpHost({
        provider: 'ssh',
        tool: 'execute',
        arguments: { command: 'secret command /private/path' }
      })
    ).toMatchObject({ success: false, code: 'NETWORK_ERROR' })
    expect(JSON.stringify(logger.mock.calls)).not.toMatch(/secret|command|private/)
  })
})
