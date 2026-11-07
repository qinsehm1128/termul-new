/**
 * Web vs desktop branch tests for `memoryIndexApi`.
 *
 * The facade is the only place the two transports meet, so this pins that each
 * member reaches the right one and that the web path posts to the route its
 * Tauri twin mirrors.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const invoke = vi.fn()
const isTauriContext = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }))
vi.mock('../tauri-runtime', () => ({ isTauriContext: () => isTauriContext() }))

const fetchMock = vi.fn()

function okResponse(data: unknown) {
  return Promise.resolve({
    ok: true,
    status: 200,
    json: () => Promise.resolve({ success: true, data })
  })
}

const STATUS = {
  projectKey: 'termul-0011223344556677',
  projectLabel: 'termul',
  databasePath: '/state/memory-index/termul-0011223344556677/index.sqlite3',
  exists: true,
  sizeBytes: 1024,
  sessionCount: 2,
  messageCount: 9,
  compactionCount: 1,
  newestFirstMessageAtUtc: '2026-09-01T00:00:00.000Z'
}

describe('memoryIndexApi (web vs desktop branch)', () => {
  beforeEach(() => {
    invoke.mockReset()
    isTauriContext.mockReset()
    fetchMock.mockReset()
    vi.stubGlobal('fetch', fetchMock)
  })

  afterEach(() => {
    vi.unstubAllGlobals()
    vi.resetModules()
  })

  it.each([
    false,
    true
  ])('exposes only memory operations on the facade and both adapters (desktop=%s)', async (desktop) => {
    isTauriContext.mockReturnValue(desktop)
    const { memoryIndexApi, createTauriMemoryIndexApi, webMemoryIndexApi } = await import(
      '../memory-index-api'
    )
    for (const api of [memoryIndexApi, createTauriMemoryIndexApi(), webMemoryIndexApi]) {
      expect(Object.keys(api).sort()).toEqual([
        'build',
        'cancel',
        'getSession',
        'listSessions',
        'search',
        'status'
      ])
      expect(api).not.toHaveProperty('mcpInvocation')
      expect(api).not.toHaveProperty('universalMcpInvocation')
    }
    expect(invoke).not.toHaveBeenCalled()
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it.each([
    false,
    true
  ])('retains build, status, search and session routing (desktop=%s)', async (desktop) => {
    isTauriContext.mockReturnValue(desktop)
    const { memoryIndexApi } = await import('../memory-index-api')
    const scope = { projectRoot: '/repo' }
    const searchArgs = { ...scope, query: 'redirect' }
    const sessionArgs = { ...scope, sessionKey: 'pi:/session.jsonl' }
    const build = { projectKey: STATUS.projectKey, sessionsIndexed: 2, issues: [] }
    const search = { projectKey: STATUS.projectKey, hits: [], compactions: [] }
    const sessions = [{ sessionKey: sessionArgs.sessionKey }]
    const session = { session: sessions[0], messages: [] }
    const cases = [
      { route: 'build', args: scope, data: build, call: () => memoryIndexApi.build(scope) },
      { route: 'status', args: scope, data: STATUS, call: () => memoryIndexApi.status(scope) },
      {
        route: 'search',
        args: searchArgs,
        data: search,
        call: () => memoryIndexApi.search(searchArgs)
      },
      {
        route: 'sessions',
        args: scope,
        data: sessions,
        call: () => memoryIndexApi.listSessions(scope)
      },
      {
        route: 'session',
        args: sessionArgs,
        data: session,
        call: () => memoryIndexApi.getSession(sessionArgs)
      },
      { route: 'cancel', args: scope, data: true, call: () => memoryIndexApi.cancel(scope) }
    ]
    for (const { route, args, data, call } of cases) {
      invoke.mockResolvedValueOnce(data)
      fetchMock.mockReturnValueOnce(okResponse(data))
      await expect(call()).resolves.toEqual(data)
      if (desktop) {
        expect(invoke).toHaveBeenLastCalledWith(`memory_index_${route}_cmd`, { args })
        expect(fetchMock).not.toHaveBeenCalled()
      } else {
        expect(fetchMock).toHaveBeenLastCalledWith(
          expect.stringMatching(new RegExp(`/memory-index/${route}$`)),
          expect.objectContaining({ method: 'POST', body: JSON.stringify(args) })
        )
        expect(invoke).not.toHaveBeenCalled()
      }
    }
  })

  it('desktop: status goes through the Tauri command', async () => {
    isTauriContext.mockReturnValue(true)
    invoke.mockResolvedValue(STATUS)
    const { memoryIndexApi } = await import('../memory-index-api')

    const result = await memoryIndexApi.status({ projectRoot: '/repo' })

    expect(invoke).toHaveBeenCalledWith('memory_index_status_cmd', {
      args: { projectRoot: '/repo' }
    })
    expect(result.projectKey).toBe(STATUS.projectKey)
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it('web: search posts to /memory-index/search', async () => {
    isTauriContext.mockReturnValue(false)
    fetchMock.mockReturnValue(
      okResponse({
        projectKey: STATUS.projectKey,
        query: 'redirect',
        compactions: [],
        hits: [],
        staleHitsOmitted: 0
      })
    )
    const { memoryIndexApi } = await import('../memory-index-api')

    const result = await memoryIndexApi.search({ projectRoot: '/repo', query: 'redirect' })

    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringMatching(/\/memory-index\/search$/),
      expect.objectContaining({ method: 'POST' })
    )
    expect(result.query).toBe('redirect')
    expect(invoke).not.toHaveBeenCalled()
  })

  it('a session that is absent reads as null rather than throwing', async () => {
    isTauriContext.mockReturnValue(true)
    invoke.mockResolvedValue(null)
    const { memoryIndexApi } = await import('../memory-index-api')

    await expect(
      memoryIndexApi.getSession({ projectRoot: '/repo', sessionKey: 'pi:/nope.jsonl' })
    ).resolves.toBeNull()
  })

  it('an invalid payload is rejected rather than passed through', async () => {
    isTauriContext.mockReturnValue(true)
    invoke.mockResolvedValue({ unexpected: true })
    const { memoryIndexApi } = await import('../memory-index-api')

    await expect(memoryIndexApi.status({ projectRoot: '/repo' })).rejects.toThrow(/invalid payload/)
  })
})
