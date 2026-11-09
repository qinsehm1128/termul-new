import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { RemoteDesktopState } from '@/lib/remote-desktop-api'
import { RemoteDesktopBanner } from './RemoteDesktopBanner'

const mocks = vi.hoisted(() => ({
  subscribeState: vi.fn(),
  emergencyStop: vi.fn(),
  unlisten: vi.fn(),
  desktop: true
}))
vi.mock('@/lib/remote-desktop-api', () => ({ remoteDesktopApi: mocks }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => mocks.desktop }))
const idle: RemoteDesktopState = {
  viewers: 0,
  controllers: 0,
  display: null,
  controlEnabled: false
}

beforeEach(() => {
  vi.resetAllMocks()
  mocks.desktop = true
  mocks.subscribeState.mockResolvedValue(mocks.unlisten)
  mocks.emergencyStop.mockResolvedValue(undefined)
})
afterEach(cleanup)

async function mountBanner() {
  const result = render(<RemoteDesktopBanner />)
  await waitFor(() => expect(mocks.subscribeState).toHaveBeenCalledTimes(1))
  await act(async () => {})
  return result
}
function emit(state: RemoteDesktopState) {
  act(() => mocks.subscribeState.mock.calls[0][0](state))
}

describe('remote desktop local activity banner', () => {
  it('is hidden when idle and shows view/control state and display without settings assumptions', async () => {
    await mountBanner()
    expect(screen.queryByRole('status')).toBeNull()
    emit(idle)
    expect(screen.queryByRole('status')).toBeNull()
    emit({ ...idle, viewers: 1, display: '2' })
    expect(screen.getByRole('status').textContent).toContain('SeRemote 正在查看 · 屏幕 2')
    emit({ ...idle, controllers: 1, display: '3' })
    expect(screen.getByRole('status').textContent).toContain('SeRemote 正在控制 · 屏幕 3')
    emit(idle)
    expect(screen.queryByRole('status')).toBeNull()
  })

  it('does not display a null screen and keeps the banner until the host confirms stopped', async () => {
    await mountBanner()
    emit({ ...idle, viewers: 1 })
    expect(screen.getByRole('status').textContent).not.toContain('屏幕')
    fireEvent.click(screen.getByRole('button', { name: '停止' }))
    await waitFor(() => expect(mocks.emergencyStop).toHaveBeenCalledTimes(1))
    expect(screen.getByRole('status')).toBeTruthy()
    emit(idle)
    expect(screen.queryByRole('status')).toBeNull()
  })

  it('handles the app-local emergency chord even with no active state and ignores unrelated/repeated keys', async () => {
    await mountBanner()
    fireEvent.keyDown(window, { key: '.', code: 'Period', ctrlKey: true, altKey: true })
    fireEvent.keyDown(window, {
      key: '.',
      code: 'Period',
      ctrlKey: true,
      altKey: true,
      metaKey: true,
      repeat: true
    })
    expect(mocks.emergencyStop).not.toHaveBeenCalled()
    const prevented = fireEvent.keyDown(window, {
      key: '.',
      code: 'Period',
      ctrlKey: true,
      altKey: true,
      metaKey: true
    })
    expect(prevented).toBe(false)
    await waitFor(() => expect(mocks.emergencyStop).toHaveBeenCalledTimes(1))
  })

  it('blocks concurrent stops and permits retry after a sanitized error', async () => {
    let fail!: (reason: Error) => void
    mocks.emergencyStop.mockImplementationOnce(
      () =>
        new Promise((_resolve, reject) => {
          fail = reject
        })
    )
    await mountBanner()
    emit({ ...idle, controllers: 1 })
    fireEvent.click(screen.getByRole('button', { name: '停止' }))
    expect(screen.getByRole('button', { name: '停止中…' }).hasAttribute('disabled')).toBe(true)
    fireEvent.keyDown(window, { key: '.', ctrlKey: true, altKey: true, metaKey: true })
    expect(mocks.emergencyStop).toHaveBeenCalledTimes(1)
    await act(async () => fail(new Error('private host token')))
    expect(screen.getByRole('alert').textContent).toBe('停止失败，请重试')
    fireEvent.click(screen.getByRole('button', { name: '停止' }))
    await waitFor(() => expect(mocks.emergencyStop).toHaveBeenCalledTimes(2))
  })

  it('is inert in a browser, including the emergency shortcut', () => {
    mocks.desktop = false
    render(<RemoteDesktopBanner />)
    fireEvent.keyDown(window, { key: '.', ctrlKey: true, altKey: true, metaKey: true })
    expect(screen.queryByRole('status')).toBeNull()
    expect(mocks.subscribeState).not.toHaveBeenCalled()
    expect(mocks.emergencyStop).not.toHaveBeenCalled()
  })

  it('unsubscribes and removes the shortcut on unmount', async () => {
    const { unmount } = await mountBanner()
    unmount()
    expect(mocks.unlisten).toHaveBeenCalledTimes(1)
    fireEvent.keyDown(window, { key: '.', ctrlKey: true, altKey: true, metaKey: true })
    expect(mocks.emergencyStop).not.toHaveBeenCalled()
  })

  it('cleans up a subscription resolving after unmount', async () => {
    let finish!: (cleanup: () => void) => void
    mocks.subscribeState.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        })
    )
    const { unmount } = await mountBanner()
    unmount()
    await act(async () => finish(mocks.unlisten))
    expect(mocks.unlisten).toHaveBeenCalledTimes(1)
  })
})
