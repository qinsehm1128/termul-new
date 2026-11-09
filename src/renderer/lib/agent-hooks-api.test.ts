import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { agentHooksApi } from './agent-hooks-api'

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), desktop: true }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('./tauri-runtime', () => ({ isTauriContext: () => mocks.desktop }))
const statuses = [
  {
    agent: 'pi',
    state: 'installed',
    config_path: '/host/.pi/agent/extensions/se.js',
    detail: 'Ready'
  }
]
const events = [
  {
    sequence: 2,
    terminal_id: 'pty',
    project_id: 'p',
    agent: 'pi',
    session_id: 's',
    kind: 'idle',
    at: 3
  }
]

beforeEach(() => {
  mocks.desktop = true
  mocks.invoke.mockReset()
})
afterEach(() => vi.unstubAllGlobals())
describe('hook API runtime parity', () => {
  it('invokes exact host commands and camelCase argument keys', async () => {
    mocks.invoke
      .mockResolvedValueOnce(statuses)
      .mockResolvedValueOnce(statuses[0])
      .mockResolvedValueOnce(statuses[0])
      .mockResolvedValueOnce(events)
    expect(await agentHooksApi.status()).toEqual(statuses)
    await agentHooksApi.install('pi')
    await agentHooksApi.uninstall('pi')
    expect(await agentHooksApi.events(1)).toEqual(events)
    expect(mocks.invoke.mock.calls).toEqual([
      ['agent_hooks_status'],
      ['agent_hooks_install', { agent: 'pi' }],
      ['agent_hooks_uninstall', { agent: 'pi' }],
      ['agent_hook_events', { after: 1 }]
    ])
  })
  it('uses equivalent web endpoints, mutation bodies and cursor, preserving snake_case payloads', async () => {
    mocks.desktop = false
    const fetcher = vi.fn()
    vi.stubGlobal('fetch', fetcher)
    for (const data of [statuses, statuses[0], statuses[0], events, []])
      fetcher.mockResolvedValueOnce(
        new Response(JSON.stringify({ success: true, data }), { status: 200 })
      )
    expect(await agentHooksApi.status()).toEqual(statuses)
    await agentHooksApi.install('pi')
    await agentHooksApi.uninstall('pi')
    expect(await agentHooksApi.events(1)).toEqual(events)
    await agentHooksApi.events()
    expect(
      fetcher.mock.calls.map(
        ([url]) =>
          new URL(url, 'http://localhost').pathname + new URL(url, 'http://localhost').search
      )
    ).toEqual([
      '/agent-hooks/status',
      '/agent-hooks/install',
      '/agent-hooks/uninstall',
      '/agent-hooks/events?after=1',
      '/agent-hooks/events'
    ])
    expect(JSON.parse(fetcher.mock.calls[1][1].body)).toEqual({ agent: 'pi' })
    expect(fetcher.mock.calls[1][1].method).toBe('POST')
    expect(JSON.parse(fetcher.mock.calls[2][1].body)).toEqual({ agent: 'pi' })
  })
  it('returns the same exact preview diff over desktop and read-only web transports', async () => {
    const diff = '--- /host/config\n+++ /host/config\n+ managed-hook'
    mocks.invoke.mockResolvedValue(diff)
    expect(await agentHooksApi.preview('qin-code')).toBe(diff)
    expect(mocks.invoke).toHaveBeenCalledExactlyOnceWith('agent_hooks_preview', {
      agent: 'qin-code'
    })
    mocks.desktop = false
    const fetcher = vi
      .fn()
      .mockResolvedValue(
        new Response(JSON.stringify({ success: true, data: diff }), { status: 200 })
      )
    vi.stubGlobal('fetch', fetcher)
    expect(await agentHooksApi.preview('qin-code')).toBe(diff)
    expect(
      new URL(fetcher.mock.calls[0][0], 'http://localhost').pathname +
        new URL(fetcher.mock.calls[0][0], 'http://localhost').search
    ).toBe('/agent-hooks/preview?agent=qin-code')
    expect(fetcher.mock.calls[0][1]?.method ?? 'GET').toBe('GET')
  })
  it('rejects failed remote mutations rather than displaying a successful installation', async () => {
    mocks.desktop = false
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValue(
          new Response(
            JSON.stringify({ success: false, error: 'cannot write config', code: 'IO_ERROR' }),
            { status: 200 }
          )
        )
    )
    await expect(agentHooksApi.install('qin-code')).rejects.toThrow('cannot write config')
  })
})
