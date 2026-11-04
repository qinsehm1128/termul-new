import { parseWebBridgeEnvelope } from '@shared/types/plugin-web.types'
import { JSDOM } from 'jsdom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import source from './shim/se-plugin-ui.js?raw'
import { webSession } from './test-fixtures'

interface ShimApi {
  ready: Promise<void>
  invoke(command: string): Promise<unknown>
  view: { dispatch(action: string): Promise<unknown> }
  config: { get(): Promise<unknown>; set(params: object): Promise<unknown> }
  storage: {
    get(params: object): Promise<unknown>
    set(params: object): Promise<unknown>
    delete(params: object): Promise<unknown>
  }
  context: {
    snapshot(params: object): Promise<unknown>
    onEvent(id: string, fn: (value: unknown) => void): () => void
  }
  feedback: { notify(params: object): Promise<unknown>; progress(params: object): Promise<unknown> }
  ui: { onMessage(fn: (value: unknown) => void): () => void }
  theme: { current: unknown; onChange(fn: (value: unknown) => void): () => void }
}
let dom: JSDOM
let api: ShimApi
const postMessage = vi.fn()
const parent = { postMessage }
const origin = 'http://tauri.localhost'
const base = {
  channel: 'se-plugin-ui',
  version: 1,
  sessionId: webSession.sessionId,
  nonce: webSession.nonce
}
const theme = {
  revision: 1,
  appearance: 'dark',
  variables: { '--background': '#101010' },
  fontFamily: 'Test font',
  fontSize: '15px'
}
function receive(data: object, source: object = parent, eventOrigin = origin): void {
  dom.window.dispatchEvent(
    new dom.window.MessageEvent('message', {
      data: JSON.stringify({ ...base, ...data }),
      source: source as Window,
      origin: eventOrigin
    })
  )
}
function welcome(): void {
  receive({
    type: 'welcome',
    pluginId: webSession.pluginId,
    viewId: 'web',
    generation: webSession.generation,
    theme
  })
}
function realm(value: object): object {
  return dom.window.JSON.parse(JSON.stringify(value))
}
function last(): Record<string, unknown> {
  return JSON.parse(postMessage.mock.calls.at(-1)?.[0] as string)
}
beforeEach(() => {
  vi.clearAllMocks()
  dom = new JSDOM('<!doctype html><html><body></body></html>', {
    url: `https://plugin.invalid/#${new URLSearchParams({ sessionId: webSession.sessionId, nonce: webSession.nonce, parentOrigin: origin })}`,
    runScripts: 'outside-only'
  })
  Object.defineProperty(dom.window, 'parent', { value: parent })
  Object.assign(dom.window, { TextEncoder })
  dom.window.eval(source)
  api = (dom.window as unknown as { se: ShimApi }).se
})
afterEach(() => {
  dom.window.dispatchEvent(new dom.window.Event('pagehide'))
  dom.window.close()
})

describe('classic se-plugin-ui shim in jsdom with fake parent', () => {
  it('authenticates its parent handshake and performs an invoke round trip', async () => {
    expect(last()).toEqual({ ...base, type: 'ready' })
    expect(postMessage).toHaveBeenCalledWith(expect.any(String), origin)
    const operation = api.invoke('hello')
    receive(
      {
        type: 'welcome',
        pluginId: webSession.pluginId,
        viewId: 'web',
        generation: webSession.generation,
        theme
      },
      {},
      origin
    )
    receive(
      {
        type: 'welcome',
        pluginId: webSession.pluginId,
        viewId: 'web',
        generation: webSession.generation,
        theme
      },
      parent,
      'https://evil.invalid'
    )
    receive({
      type: 'welcome',
      pluginId: webSession.pluginId,
      viewId: 'web',
      generation: webSession.generation,
      theme,
      nonce: 'f'.repeat(32)
    })
    await Promise.resolve()
    expect(postMessage).toHaveBeenCalledTimes(1)
    welcome()
    await api.ready
    await Promise.resolve()
    const request = last()
    expect(request).toMatchObject({
      ...base,
      type: 'request',
      method: 'invoke',
      params: { command: 'hello', args: {} }
    })
    receive({
      type: 'response',
      requestId: request.requestId,
      ok: true,
      result: { hello: 'world' }
    })
    await expect(operation).resolves.toEqual({ hello: 'world' })
  })

  it('exposes each contracted facade method and propagates host denials as coded errors', async () => {
    welcome()
    await api.ready
    const calls: [string, () => Promise<unknown>][] = [
      ['view.dispatch', () => api.view.dispatch('save')],
      ['config.get', () => api.config.get()],
      ['config.set', () => api.config.set(realm({ expectedRevision: 1, values: {} }))],
      ['context.snapshot', () => api.context.snapshot(realm({ include: ['activeProject'] }))],
      ['storage.get', () => api.storage.get(realm({ scope: 'global', key: 'key' }))],
      ['storage.set', () => api.storage.set(realm({ scope: 'global', key: 'key', value: 1 }))],
      ['storage.delete', () => api.storage.delete(realm({ scope: 'global', key: 'key' }))],
      [
        'feedback.notification.upsert',
        () => api.feedback.notify(realm({ level: 'info', message: 'Done' }))
      ],
      [
        'feedback.progress.upsert',
        () => api.feedback.progress(realm({ id: 'task', state: 'running' }))
      ]
    ]
    for (const [method, call] of calls) {
      const result = call()
      await Promise.resolve()
      const request = last()
      expect(parseWebBridgeEnvelope(postMessage.mock.calls.at(-1)?.[0])).toMatchObject({
        type: 'request',
        method
      })
      expect(request.method).toBe(method)
      if (method === 'storage.set' || method === 'storage.delete')
        expect(request.params).toMatchObject({ requestId: expect.stringMatching(/^[0-9a-f]{32}$/) })
      receive({
        type: 'response',
        requestId: request.requestId,
        ok: false,
        error: { code: 'PLUGIN_UI_NOT_ALLOWED', message: 'Denied' }
      })
      await expect(result).rejects.toMatchObject({
        code: 'PLUGIN_UI_NOT_ALLOWED',
        message: 'Denied'
      })
    }
  })

  it('applies theme variables, delivers events and supports unsubscribe without a reload', async () => {
    const changed = vi.fn()
    const ui = vi.fn()
    const context = vi.fn()
    api.theme.onChange(changed)
    const off = api.ui.onMessage(ui)
    api.context.onEvent('sub', context)
    welcome()
    await api.ready
    expect(api.theme.current).toEqual(theme)
    expect(dom.window.document.documentElement.style.getPropertyValue('--background')).toBe(
      '#101010'
    )
    expect(dom.window.document.documentElement.style.getPropertyValue('--se-font-family')).toBe(
      'Test font'
    )
    receive({
      type: 'event',
      sequence: 1,
      event: 'themeChanged',
      payload: { ...theme, revision: 2, appearance: 'light' }
    })
    receive({ type: 'event', sequence: 2, event: 'message', payload: { kind: 'done' } })
    receive({
      type: 'event',
      sequence: 3,
      event: 'contextEvent',
      payload: { subscriptionId: 'other' }
    })
    receive({
      type: 'event',
      sequence: 4,
      event: 'contextEvent',
      payload: { subscriptionId: 'sub', event: 'focus' }
    })
    expect(changed).toHaveBeenCalledTimes(2)
    expect(ui).toHaveBeenCalledExactlyOnceWith({ kind: 'done' })
    expect(context).toHaveBeenCalledExactlyOnceWith({ subscriptionId: 'sub', event: 'focus' })
    off()
    receive({ type: 'event', sequence: 5, event: 'message', payload: {} })
    expect(ui).toHaveBeenCalledOnce()
    expect(dom.window.document.documentElement.dataset.seTheme).toBe('light')
  })

  it('rejects pending calls on revocation and never retries mutations', async () => {
    welcome()
    await api.ready
    const result = api.config.get()
    await Promise.resolve()
    const requests = postMessage.mock.calls.length
    receive({
      type: 'event',
      sequence: 1,
      event: 'invalidate',
      payload: { code: 'PLUGIN_UI_SESSION_EXPIRED' }
    })
    await expect(result).rejects.toMatchObject({ code: 'PLUGIN_UI_SESSION_EXPIRED' })
    await expect(api.config.get()).rejects.toMatchObject({ code: 'PLUGIN_UI_SESSION_EXPIRED' })
    expect(postMessage).toHaveBeenCalledTimes(requests)
  })
})
