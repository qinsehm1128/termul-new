import type { PluginConnectionChanged, PluginEvent } from '@shared/types/plugin.types'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { PLUGIN_WEB_SANDBOX, PluginWebView } from './PluginWebView'
import { webSession } from './test-fixtures'

const api = vi.hoisted(() => ({
  openWebSession: vi.fn(),
  closeWebSession: vi.fn(),
  onChanged: vi.fn(),
  onConnectionChanged: vi.fn(),
  readWebSession: vi.fn(),
  requestWeb: vi.fn()
}))
vi.mock('../plugin-api', () => ({ pluginApi: api }))
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }))
let connectionChanged: (event: PluginConnectionChanged) => void
let changed: (event: PluginEvent) => void
const props = {
  pluginId: webSession.pluginId,
  viewId: webSession.viewId,
  title: 'Plugin dashboard',
  generation: webSession.generation,
  available: true
}
beforeEach(() => {
  vi.clearAllMocks()
  api.openWebSession.mockImplementation(async (request: { generation: string }) => ({
    success: true,
    data: { ...webSession, generation: request.generation }
  }))
  api.closeWebSession.mockResolvedValue({ success: true, data: { closed: true } })
  api.readWebSession.mockResolvedValue({
    success: true,
    data: { sequence: 0, resyncRequired: false, events: [] }
  })
  api.onChanged.mockImplementation((handler) => {
    changed = handler
    return vi.fn()
  })
  api.onConnectionChanged.mockImplementation((handler) => {
    connectionChanged = handler
    return vi.fn()
  })
})
afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})
function frame(): HTMLIFrameElement {
  return screen.getByTitle(props.title, { selector: 'iframe' }) as HTMLIFrameElement
}

describe('PluginWebView lifecycle and isolation', () => {
  it('waits for lifecycle listener readiness and aborts opens when disconnected during setup', async () => {
    let release!: () => void
    const ready = new Promise<void>((done) => {
      release = done
    })
    api.onConnectionChanged.mockImplementationOnce((handler) => {
      connectionChanged = handler
      return Object.assign(vi.fn(), { ready })
    })
    render(<PluginWebView {...props} />)
    await act(async () => {})
    expect(api.openWebSession).not.toHaveBeenCalled()
    act(() => connectionChanged({ state: 'disconnected' } as PluginConnectionChanged))
    await act(async () => {
      release()
    })
    expect(api.openWebSession).not.toHaveBeenCalled()
    expect(screen.getByRole('alert')).toHaveTextContent('web.revoked')
  })

  it('revokes subsequent frame document navigations instead of reusing a session capability', async () => {
    render(<PluginWebView {...props} />)
    await waitFor(() => expect(frame()).toBeInTheDocument())
    const target = frame()
    fireEvent.load(target)
    fireEvent.load(target)
    expect(api.closeWebSession).toHaveBeenCalledOnce()
    expect(screen.getByRole('alert')).toHaveTextContent('web.revoked')
  })

  it('uses the exact opaque sandbox, session URL and host-owned title/trust/reload chrome', async () => {
    render(<PluginWebView {...props} />)
    await waitFor(() => expect(frame()).toBeInTheDocument())
    expect(frame()).toHaveAttribute('sandbox', PLUGIN_WEB_SANDBOX)
    expect(frame().getAttribute('sandbox')).not.toContain('allow-same-origin')
    expect(frame()).toHaveAttribute('referrerpolicy', 'no-referrer')
    expect(frame().src).toContain(webSession.url)
    expect(new URLSearchParams(new URL(frame().src).hash.slice(1)).get('nonce')).toBe(
      webSession.nonce
    )
    expect(screen.getByRole('heading')).toHaveTextContent(props.title)
    expect(screen.getByText('web.trust')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'web.reload' })).toBeEnabled()
    expect(screen.getByRole('status')).toHaveTextContent('web.loading')
    expect(api.openWebSession).toHaveBeenCalledWith({
      id: props.pluginId,
      viewId: props.viewId,
      generation: props.generation
    })
  })

  it('closes on generation change and unmount, dropping late-generation replies', async () => {
    const view = render(<PluginWebView {...props} />)
    await waitFor(() => expect(frame()).toBeInTheDocument())
    view.rerender(<PluginWebView {...props} generation="33333333-3333-4333-8333-333333333333" />)
    await waitFor(() => expect(api.openWebSession).toHaveBeenCalledTimes(2))
    expect(api.closeWebSession).toHaveBeenCalledWith({ sessionId: webSession.sessionId })
    view.unmount()
    expect(api.closeWebSession).toHaveBeenCalledTimes(2)
  })

  it('closes an in-flight open resolved after unmount or disconnect', async () => {
    let resolve!: (value: unknown) => void
    api.openWebSession.mockReturnValueOnce(
      new Promise((done) => {
        resolve = done
      })
    )
    const view = render(<PluginWebView {...props} />)
    await waitFor(() => expect(api.openWebSession).toHaveBeenCalledOnce())
    view.unmount()
    await act(async () => {
      resolve({ success: true, data: webSession })
    })
    expect(api.closeWebSession).toHaveBeenCalledOnce()
    api.closeWebSession.mockClear()
    api.openWebSession.mockReturnValueOnce(
      new Promise((done) => {
        resolve = done
      })
    )
    render(<PluginWebView {...props} />)
    await waitFor(() => expect(api.openWebSession).toHaveBeenCalledTimes(2))
    act(() => connectionChanged({ state: 'disconnected' } as PluginConnectionChanged))
    await act(async () => {
      resolve({ success: true, data: webSession })
    })
    expect(api.closeWebSession).toHaveBeenCalledOnce()
    expect(screen.queryByTitle(props.title, { selector: 'iframe' })).not.toBeInTheDocument()
  })

  it('revokes the bridge and frame when native authority changes or disconnects', async () => {
    render(<PluginWebView {...props} />)
    await waitFor(() => expect(frame()).toBeInTheDocument())
    act(() => changed({ type: 'registry_changed', revision: 2 } as PluginEvent))
    expect(api.closeWebSession).toHaveBeenCalledOnce()
    expect(screen.getByRole('alert')).toHaveTextContent('web.revoked')
    expect(screen.queryByTitle(props.title, { selector: 'iframe' })).not.toBeInTheDocument()
  })

  it('sends theme changes after handshake without changing frame source', async () => {
    render(<PluginWebView {...props} />)
    await waitFor(() => expect(frame()).toBeInTheDocument())
    const target = frame()
    const src = target.src
    const post = vi
      .spyOn(target.contentWindow as Window, 'postMessage')
      .mockImplementation(() => {})
    act(() =>
      window.dispatchEvent(
        new MessageEvent('message', {
          source: target.contentWindow,
          origin: 'null',
          data: JSON.stringify({
            channel: 'se-plugin-ui',
            version: 1,
            sessionId: webSession.sessionId,
            nonce: webSession.nonce,
            type: 'ready'
          })
        })
      )
    )
    expect(screen.queryByRole('status')).not.toBeInTheDocument()
    await act(async () => {
      document.documentElement.classList.add('dark')
    })
    expect(post.mock.calls.map(([data]) => JSON.parse(data as string))).toContainEqual(
      expect.objectContaining({
        type: 'event',
        event: 'themeChanged',
        payload: expect.objectContaining({ appearance: 'dark' })
      })
    )
    expect(target.src).toBe(src)
    expect(api.openWebSession).toHaveBeenCalledOnce()
    document.documentElement.classList.remove('dark')
  })

  it('reload closes the previous capability and opens a fresh session', async () => {
    render(<PluginWebView {...props} />)
    await waitFor(() => expect(frame()).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'web.reload' }))
    await waitFor(() => expect(api.openWebSession).toHaveBeenCalledTimes(2))
    expect(api.closeWebSession).toHaveBeenCalledOnce()
  })

  it('renders crashed/revoked/open error without mounting an iframe', async () => {
    const view = render(<PluginWebView {...props} available={false} crashed />)
    expect(screen.getByRole('alert')).toHaveTextContent('web.crashed')
    expect(api.openWebSession).not.toHaveBeenCalled()
    view.rerender(<PluginWebView {...props} available={false} />)
    expect(screen.getByRole('alert')).toHaveTextContent('web.revoked')
    api.openWebSession.mockResolvedValueOnce({
      success: false,
      code: 'PLUGIN_NOT_SUPPORTED',
      error: 'Unavailable'
    })
    view.rerender(<PluginWebView {...props} />)
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('web.error'))
    expect(screen.queryByTitle(props.title, { selector: 'iframe' })).not.toBeInTheDocument()
  })
})
