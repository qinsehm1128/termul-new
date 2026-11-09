import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { agentChatApi, subscribeAgentChatUpdated } from './agent-chat-api'
import {
  _resetRemoteAccessCredentialForTests,
  setRemoteAccessCredential
} from './remote-access-credential'

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn(), desktop: true }))
vi.mock('@tauri-apps/api/event', () => ({ listen: mocks.listen }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('./tauri-runtime', () => ({ isTauriContext: () => mocks.desktop }))
const snapshot = {
  terminal_id: 'pty/a',
  session_id: 'session',
  agent: 'qin-code',
  cursor: 7,
  older_cursor: 2,
  messages: [{ id: 'm1', role: 'agent', blocks: [{ type: 'text', text: 'hello' }], timestamp: 1 }],
  tool_calls: [],
  reset: false,
  deliveries: [],
  state: 'ready',
  hook_state: 'idle'
}

beforeEach(() => {
  mocks.desktop = true
  mocks.invoke.mockReset()
})
afterEach(() => {
  vi.unstubAllGlobals()
  _resetRemoteAccessCredentialForTests()
})

describe('terminal agent chat facade parity', () => {
  it('subscribes to desktop metadata while browser relies on snapshot polling', async () => {
    const off = vi.fn()
    const received = vi.fn()
    mocks.listen.mockReset().mockResolvedValue(off)
    expect(await subscribeAgentChatUpdated(received)).toBe(off)
    expect(mocks.listen.mock.calls[0][0]).toBe('agent-chat-updated')
    mocks.listen.mock.calls[0][1]({ payload: { terminal_id: 'pty', cursor: 1 } })
    expect(received).toHaveBeenCalledExactlyOnceWith({ terminal_id: 'pty', cursor: 1 })
    mocks.desktop = false
    const browserOff = await subscribeAgentChatUpdated(received)
    browserOff()
    expect(mocks.listen).toHaveBeenCalledTimes(1)
  })
  it('uses exact desktop command names and camelCase arguments', async () => {
    mocks.invoke.mockResolvedValue(snapshot)
    expect(await agentChatApi.snapshot('pty/a')).toEqual(snapshot)
    await agentChatApi.snapshot('pty/a', 5)
    await agentChatApi.snapshot('pty/a', undefined, 2)
    await agentChatApi.send('pty/a', 'hello\nworld')
    await agentChatApi.control('pty/a', 'interrupt')
    await agentChatApi.control('pty/a', 'stop')
    expect(mocks.invoke.mock.calls).toEqual([
      ['agent_chat_snapshot', { terminalId: 'pty/a', after: null, before: null }],
      ['agent_chat_snapshot', { terminalId: 'pty/a', after: 5, before: null }],
      ['agent_chat_snapshot', { terminalId: 'pty/a', after: null, before: 2 }],
      ['agent_chat_send', { terminalId: 'pty/a', text: 'hello\nworld' }],
      ['agent_chat_control', { terminalId: 'pty/a', action: 'interrupt' }],
      ['agent_chat_control', { terminalId: 'pty/a', action: 'stop' }]
    ])
  })

  it('maps a queued force override to camelCase native command args without resending text', async () => {
    mocks.desktop = true
    await agentChatApi.send('pty/a', '', { force: true, delivery_id: 'queued-1' })
    expect(mocks.invoke).toHaveBeenCalledExactlyOnceWith('agent_chat_send', {
      terminalId: 'pty/a',
      text: '',
      force: true,
      deliveryId: 'queued-1'
    })
  })

  it('uses equivalent authenticated HTTP endpoints, incremental cursor and mutation bodies', async () => {
    mocks.desktop = false
    setRemoteAccessCredential('test-agent-chat-token')
    const fetcher = vi
      .fn()
      .mockImplementation(
        async () => new Response(JSON.stringify({ success: true, data: snapshot }), { status: 200 })
      )
    vi.stubGlobal('fetch', fetcher)
    expect(await agentChatApi.snapshot('pty/a')).toEqual(snapshot)
    expect(await agentChatApi.snapshot('pty/a', 5)).toEqual(snapshot)
    expect(await agentChatApi.snapshot('pty/a', undefined, 2)).toEqual(snapshot)
    await agentChatApi.send('pty/a', 'hello\nworld')
    await agentChatApi.control('pty/a', 'interrupt')
    await agentChatApi.control('pty/a', 'stop')
    expect(
      fetcher.mock.calls.map(([url]) => {
        const parsed = new URL(url, 'http://localhost')
        return parsed.pathname + parsed.search
      })
    ).toEqual([
      '/agent-chat/pty%2Fa',
      '/agent-chat/pty%2Fa?after=5',
      '/agent-chat/pty%2Fa?before=2',
      '/agent-chat/pty%2Fa/send',
      '/agent-chat/pty%2Fa/control',
      '/agent-chat/pty%2Fa/control'
    ])
    expect(fetcher.mock.calls.slice(3).map(([, options]) => options.method)).toEqual([
      'POST',
      'POST',
      'POST'
    ])
    expect(fetcher.mock.calls.slice(3).map(([, options]) => JSON.parse(options.body))).toEqual([
      { text: 'hello\nworld' },
      { action: 'interrupt' },
      { action: 'stop' }
    ])
    for (const [, options] of fetcher.mock.calls) {
      expect(new Headers(options.headers).get('authorization')).toBe('Bearer test-agent-chat-token')
    }
    expect(mocks.invoke).not.toHaveBeenCalled()
  })

  it('preserves authorization-denial codes on exclusive-before reads', async () => {
    mocks.desktop = false
    const fetcher = vi.fn(
      async () =>
        new Response(
          JSON.stringify({
            code: 'DEVICE_REVOKED',
            error: 'Device revoked'
          }),
          { status: 403 }
        )
    )
    vi.stubGlobal('fetch', fetcher)
    await expect(agentChatApi.snapshot('pty/a', undefined, 0)).rejects.toMatchObject({
      message: 'Device revoked',
      code: 'DEVICE_REVOKED'
    })
    expect(fetcher).toHaveBeenCalledWith(
      `${window.location.origin}/agent-chat/pty%2Fa?before=0`,
      expect.objectContaining({ method: 'GET', credentials: 'same-origin' })
    )
  })

  it('posts an authenticated force override for the existing receipt and surfaces busy rejection once', async () => {
    mocks.desktop = false
    setRemoteAccessCredential('force-token')
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({ success: true, data: null })))
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ code: 'AGENT_BUSY', error: 'Agent is busy' }), {
          status: 409
        })
      )
    vi.stubGlobal('fetch', fetcher)
    await agentChatApi.send('pty/a', '', { force: true, delivery_id: 'queued-1' })
    expect(fetcher).toHaveBeenNthCalledWith(
      1,
      `${window.location.origin}/agent-chat/pty%2Fa/send`,
      expect.objectContaining({
        method: 'POST',
        credentials: 'same-origin',
        body: JSON.stringify({ text: '', force: true, delivery_id: 'queued-1' })
      })
    )
    expect(new Headers(fetcher.mock.calls[0][1]?.headers).get('authorization')).toBe(
      'Bearer force-token'
    )
    await expect(
      agentChatApi.send('pty/a', '', { force: true, delivery_id: 'queued-1' })
    ).rejects.toMatchObject({ message: 'Agent is busy', code: 'AGENT_BUSY' })
    expect(fetcher).toHaveBeenCalledTimes(2)
  })

  it('surfaces backend and transport failures without retrying delivery', async () => {
    mocks.desktop = false
    const fetcher = vi
      .fn()
      .mockResolvedValue(
        new Response(JSON.stringify({ success: false, error: 'not idle' }), { status: 200 })
      )
    vi.stubGlobal('fetch', fetcher)
    await expect(agentChatApi.send('pty', 'hello')).rejects.toThrow('not idle')
    expect(fetcher).toHaveBeenCalledTimes(1)
    fetcher.mockRejectedValue(new Error('offline'))
    await expect(agentChatApi.snapshot('pty')).rejects.toThrow()
    expect(fetcher).toHaveBeenCalledTimes(2)
  })
})
