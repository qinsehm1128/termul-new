import { waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  auth: vi.fn(),
  render: vi.fn(),
  instrumentation: vi.fn(),
  i18n: vi.fn(),
  idle: vi.fn()
}))
vi.mock('./lib/remote-auth', () => ({ initializeRemoteAuth: mocks.auth }))
vi.mock('./boot-instrumentation', () => ({ installBootInstrumentation: mocks.instrumentation }))
vi.mock('@/i18n/bootstrap', () => ({ bootstrapI18n: mocks.i18n }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => false }))
vi.mock('react-dom/client', () => ({ createRoot: () => ({ render: mocks.render }) }))
vi.mock('./App', () => ({
  default: function BrowserApp() {
    return null
  }
}))

beforeEach(() => {
  vi.resetModules()
  vi.clearAllMocks()
  mocks.i18n.mockResolvedValue(undefined)
  document.body.innerHTML = '<div id="root"></div>'
  vi.stubGlobal('requestIdleCallback', mocks.idle)
})

describe('web entry authentication gate', () => {
  it('does not install HTTP diagnostics, mount App or run idle work before exchange readiness', async () => {
    let finish!: () => void
    mocks.auth.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve
        })
    )
    await import('./main')
    expect(mocks.auth).toHaveBeenCalledTimes(1)
    expect(mocks.instrumentation).not.toHaveBeenCalled()
    expect(mocks.render).not.toHaveBeenCalled()
    expect(mocks.idle).not.toHaveBeenCalled()
    finish()
    await waitFor(() => expect(mocks.render).toHaveBeenCalledTimes(1))
    expect(mocks.instrumentation).toHaveBeenCalledTimes(1)
    expect(mocks.render.mock.calls[0][0].type.name).toBe('BrowserApp')
    expect(mocks.idle).toHaveBeenCalledTimes(1)
  })
  it('fails closed without mounting App when pairing is rejected', async () => {
    mocks.auth.mockRejectedValueOnce(new Error('secret-code-and-token'))
    await import('./main')
    await waitFor(() => expect(mocks.render).toHaveBeenCalledTimes(1))
    const failure = mocks.render.mock.calls[0][0]
    expect(failure.type).toBe('div')
    expect(failure.props.role).toBe('alert')
    expect(failure.props.children).toBe('配对失败，请重新扫码')
    expect(mocks.instrumentation).not.toHaveBeenCalled()
    expect(mocks.idle).not.toHaveBeenCalled()
  })
})
