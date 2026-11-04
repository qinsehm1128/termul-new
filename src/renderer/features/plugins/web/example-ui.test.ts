import { readFileSync } from 'node:fs'
import { JSDOM } from 'jsdom'
import { afterEach, describe, expect, it, vi } from 'vitest'
import shim from './shim/se-plugin-ui.js?raw'

const ui = '../../../../../src-tauri/crates/se-plugin-example/ui/'
const html = readFileSync(new URL(ui + 'index.html', import.meta.url), 'utf8')
const app = readFileSync(new URL(ui + 'app.js', import.meta.url), 'utf8')
let dom: JSDOM | undefined
const base = {
  channel: 'se-plugin-ui',
  version: 1,
  sessionId: 'a'.repeat(64),
  nonce: 'b'.repeat(64)
}
const theme = {
  revision: 1,
  appearance: 'dark',
  variables: { '--background': '222 16% 8%' },
  fontFamily: 'sans-serif',
  fontSize: '14px'
}
interface Request {
  type: string
  requestId: string
  method: string
  params: Record<string, unknown>
}
afterEach(() => {
  dom?.window.dispatchEvent(new dom.window.Event('pagehide'))
  dom?.window.close()
  dom = undefined
})

describe('packaged Activity Studio page with the real classic shim', () => {
  it('renders cards, native messages, safe sandbox observations and profile CAS notes', async () => {
    let count = 2
    let revision = 0
    let note: unknown = null
    const requests: Request[] = []
    const snapshot = () => ({
      count,
      terminalCount: 3,
      agentTurnCount: 7,
      activity: [{ label: 'Turn', value: 7 }],
      service: { ok: true, port: 43210, pid: 42, uptimeMs: 1000 },
      wakeCount: 4
    })
    const parentOrigin = 'http://tauri.localhost'
    const receive = (data: object): void => {
      dom!.window.dispatchEvent(
        new dom!.window.MessageEvent('message', {
          data: JSON.stringify({ ...base, ...data }),
          source: parent as unknown as Window,
          origin: parentOrigin
        })
      )
    }
    const parent = {
      get document(): never {
        throw new Error('sandbox blocked parent document')
      },
      postMessage: vi.fn((wire: string) => {
        const message = JSON.parse(wire) as Request
        queueMicrotask(() => {
          if (message.type === 'ready') {
            receive({
              type: 'welcome',
              pluginId: 'org.termul.example',
              viewId: 'activity-studio',
              generation: 'c'.repeat(64),
              theme
            })
            return
          }
          if (message.type !== 'request') return
          requests.push(message)
          let result: unknown = null
          switch (message.method) {
            case 'invoke':
              if (message.params.command === 'increment') count++
              if (message.params.command === 'reset') count = 0
              result = snapshot()
              break
            case 'config.get':
              result = { values: { greeting: 'Hello Studio' }, revision: 1 }
              break
            case 'context.snapshot':
              expect(message.params.scope).toBe('activeProject')
              result = {
                contextRevision: 1,
                context: { activeProject: { name: 'Demo' }, terminals: [{ id: 't1' }] }
              }
              break
            case 'storage.get':
              expect(message.params.scope).toBe('profile')
              result = { revision, found: note !== null, value: note }
              break
            case 'storage.set':
              expect(message.params.scope).toBe('profile')
              expect(message.params.expectedRevision).toBe(revision)
              expect(message.params.key).toBe('activity-studio.notes')
              note = message.params.value
              result = { revision: ++revision }
              break
          }
          receive({ type: 'response', requestId: message.requestId, ok: true, result })
        })
      })
    }
    dom = new JSDOM(html, {
      url: `about:blank#${new URLSearchParams({ ...base, version: '1', parentOrigin })}`,
      runScripts: 'outside-only'
    })
    Object.defineProperty(dom.window, 'parent', { value: parent })
    const fetcher = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({ ok: true, pid: 42, uptimeMs: 1234 })
    })
    Object.assign(dom.window, { TextEncoder, fetch: fetcher })
    dom.window.eval(shim)
    dom.window.eval(app)
    const text = (id: string): string => dom!.window.document.getElementById(id)!.textContent ?? ''
    await vi.waitFor(() => expect(text('bridge-status')).toBe('Host connected'))
    await vi.waitFor(() => expect(text('count')).toBe('2'))
    await vi.waitFor(() =>
      expect((dom!.window.document.getElementById('notes') as HTMLTextAreaElement).disabled).toBe(
        false
      )
    )
    expect(text('agent-turns')).toBe('7')
    expect(text('project-label')).toBe('Demo')
    expect(text('service-status')).toBe('Healthy')
    expect(dom.window.document.querySelectorAll('#chart-series rect')).toHaveLength(1)
    expect(text('sandbox-results')).toContain('__TAURI__')
    expect(text('sandbox-results')).toContain('__TAURI_INTERNALS__')
    expect(text('sandbox-results')).toContain('Access denied by browser')
    expect(text('sandbox-results')).toContain('null')
    expect(text('sandbox-results')).toContain('typeof = undefined')
    expect(text('sandbox-results')).toContain('document.origin')
    expect(text('sandbox-results')).toContain('location.origin')
    dom.window.document.getElementById('direct-ping')!.click()
    await vi.waitFor(() => expect(text('direct-ping-result')).toContain('CORS allowed · PID 42'))
    expect(fetcher).toHaveBeenCalledWith(
      'http://127.0.0.1:43210/ping',
      expect.objectContaining({ mode: 'cors', credentials: 'omit', redirect: 'error' })
    )
    dom.window.document.getElementById('increment')!.click()
    await vi.waitFor(() => expect(text('count')).toBe('3'))
    const textarea = dom.window.document.getElementById('notes') as HTMLTextAreaElement
    textarea.value = 'A durable Studio note'
    textarea.dispatchEvent(new dom.window.Event('input'))
    dom.window.document.getElementById('save-note')!.click()
    await vi.waitFor(() => expect(revision).toBe(1))
    expect(note).toEqual({ text: 'A durable Studio note' })
    receive({ type: 'event', sequence: 1, event: 'message', payload: { ...snapshot(), count: 19 } })
    await vi.waitFor(() => expect(text('count')).toBe('19'))
    receive({
      type: 'event',
      sequence: 2,
      event: 'themeChanged',
      payload: { ...theme, revision: 2, appearance: 'light' }
    })
    await vi.waitFor(() =>
      expect(dom!.window.document.documentElement.dataset.seTheme).toBe('light')
    )
    expect(requests.some((r) => r.method === 'config.get')).toBe(true)
  })
})
