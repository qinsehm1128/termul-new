/**
 * Web vs desktop branch tests for cliSessionApi.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const { mockFetch, mockIsTauriContext, mockInvoke } = vi.hoisted(() => ({
  mockFetch: vi.fn(),
  mockIsTauriContext: vi.fn(),
  mockInvoke: vi.fn()
}))

vi.mock('../tauri-runtime', () => ({
  isTauriContext: mockIsTauriContext
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: mockInvoke
}))

import { cliSessionApi } from '../cli-session-api'

const listResult = {
  sessions: [],
  issues: [],
  scannedAt: '2026-01-01T00:00:00.000Z'
}

function jsonResponse(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: status === 200 ? 'OK' : 'Error',
    json: () => Promise.resolve(body)
  } as unknown as Response
}

describe('cliSessionApi (web vs desktop branch)', () => {
  const originalFetch = globalThis.fetch

  beforeEach(() => {
    mockFetch.mockReset()
    mockInvoke.mockReset()
    mockIsTauriContext.mockReset()
    globalThis.fetch = mockFetch
  })

  afterEach(() => {
    globalThis.fetch = originalFetch
  })

  it('web: listSessions posts to /cli-sessions', async () => {
    mockIsTauriContext.mockReturnValue(false)
    mockFetch.mockResolvedValue(jsonResponse({ success: true, data: listResult }))

    const result = await cliSessionApi.listSessions({ scopePaths: ['/repo'] })

    expect(result).toEqual(listResult)
    expect(mockInvoke).not.toHaveBeenCalled()
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/cli-sessions$/),
      expect.objectContaining({ method: 'POST' })
    )
  })

  it('desktop: listSessions invokes list_cli_sessions_cmd', async () => {
    mockIsTauriContext.mockReturnValue(true)
    mockInvoke.mockResolvedValue(listResult)

    const result = await cliSessionApi.listSessions({ scopePaths: ['/repo'] })

    expect(result).toEqual(listResult)
    expect(mockFetch).not.toHaveBeenCalled()
    expect(mockInvoke).toHaveBeenCalledWith('list_cli_sessions_cmd', {
      args: { scopePaths: ['/repo'] }
    })
  })

  it('web: resolveSessions posts to /cli-sessions/resolve', async () => {
    mockIsTauriContext.mockReturnValue(false)
    mockFetch.mockResolvedValue(jsonResponse({ success: true, data: { sessions: [], issues: [] } }))

    const result = await cliSessionApi.resolveSessions({
      files: [{ agentId: 'claude-code', filePath: '/tmp/a.jsonl' }]
    })

    expect(result).toEqual({ sessions: [], issues: [] })
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/cli-sessions\/resolve$/),
      expect.objectContaining({ method: 'POST' })
    )
  })

  it('desktop: resolveSessions invokes resolve_cli_sessions_cmd', async () => {
    mockIsTauriContext.mockReturnValue(true)
    mockInvoke.mockResolvedValue({ sessions: [], issues: [] })

    const result = await cliSessionApi.resolveSessions({
      files: [{ agentId: 'claude-code', filePath: '/tmp/a.jsonl' }]
    })

    expect(result).toEqual({ sessions: [], issues: [] })
    expect(mockInvoke).toHaveBeenCalledWith('resolve_cli_sessions_cmd', {
      args: { files: [{ agentId: 'claude-code', filePath: '/tmp/a.jsonl' }] }
    })
  })
})
