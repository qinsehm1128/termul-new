import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { RemoteDesktopSettings } from './RemoteDesktopSettings'

const mocks = vi.hoisted(() => ({
  getSettings: vi.fn(),
  setSettings: vi.fn(),
  openPrivacyPane: vi.fn(),
  desktop: true
}))
vi.mock('@/lib/remote-desktop-api', () => ({ remoteDesktopApi: mocks }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => mocks.desktop }))
vi.mock('@/lib/macos-permissions-api', () => ({ openPrivacyPane: mocks.openPrivacyPane }))
const defaults = {
  supported: true,
  viewEnabled: true,
  controlEnabled: false,
  permissions: { screenRecording: true, accessibility: false }
}

beforeEach(() => {
  vi.resetAllMocks()
  mocks.desktop = true
  mocks.getSettings.mockResolvedValue(defaults)
  mocks.setSettings.mockImplementation(async (update) => ({ ...defaults, ...update }))
  mocks.openPrivacyPane.mockResolvedValue(undefined)
})
afterEach(cleanup)

async function loaded() {
  await screen.findByText('屏幕录制 · 已授权')
  await waitFor(() =>
    expect(screen.getByRole('switch', { name: '允许查看桌面' }).hasAttribute('disabled')).toBe(
      false
    )
  )
}

describe('remote desktop local consent settings', () => {
  it('defaults to view on and control off, shows boolean permission rows and does not mutate on mount', async () => {
    render(<RemoteDesktopSettings />)
    await loaded()
    expect(screen.getByRole('switch', { name: '允许查看桌面' }).getAttribute('aria-checked')).toBe(
      'true'
    )
    expect(
      screen.getByRole('switch', { name: '允许控制鼠标键盘' }).getAttribute('aria-checked')
    ).toBe('false')
    expect(screen.getByText('辅助功能 · 未授权')).toBeTruthy()
    expect(screen.queryByText('已配对设备可操作这台电脑')).toBeNull()
    expect(mocks.setSettings).not.toHaveBeenCalled()
    expect(mocks.openPrivacyPane).not.toHaveBeenCalled()
  })

  it('saves each switch independently and displays the enabled-control hint', async () => {
    render(<RemoteDesktopSettings />)
    await loaded()
    fireEvent.click(screen.getByRole('switch', { name: '允许控制鼠标键盘' }))
    await screen.findByText('已配对设备可操作这台电脑')
    expect(mocks.setSettings).toHaveBeenLastCalledWith({ viewEnabled: true, controlEnabled: true })
    fireEvent.click(screen.getByRole('switch', { name: '允许查看桌面' }))
    await waitFor(() =>
      expect(mocks.setSettings).toHaveBeenLastCalledWith({
        viewEnabled: false,
        controlEnabled: true
      })
    )
  })

  it('uses the existing privacy facade only on explicit clicks and refreshes on focus', async () => {
    render(<RemoteDesktopSettings />)
    await loaded()
    fireEvent.click(screen.getByRole('button', { name: '打开屏幕录制系统设置' }))
    expect(mocks.openPrivacyPane).toHaveBeenLastCalledWith('screenRecording')
    fireEvent.click(screen.getByRole('button', { name: '打开辅助功能系统设置' }))
    expect(mocks.openPrivacyPane).toHaveBeenLastCalledWith('accessibility')
    mocks.getSettings.mockResolvedValue({
      ...defaults,
      permissions: { screenRecording: true, accessibility: true }
    })
    fireEvent(window, new Event('focus'))
    await screen.findByText('辅助功能 · 已授权')
    expect(mocks.getSettings).toHaveBeenCalledTimes(2)
  })

  it('makes browser settings explicitly unsupported and never calls host facades', () => {
    mocks.desktop = false
    render(<RemoteDesktopSettings />)
    expect(screen.getByText('请在桌面客户端设置远程桌面，浏览器不支持此功能')).toBeTruthy()
    expect(screen.queryByRole('switch')).toBeNull()
    expect(mocks.getSettings).not.toHaveBeenCalled()
    expect(mocks.setSettings).not.toHaveBeenCalled()
    expect(mocks.openPrivacyPane).not.toHaveBeenCalled()
  })

  it('displays an unsupported platform without exposing switches', async () => {
    mocks.getSettings.mockResolvedValue({ ...defaults, supported: false })
    render(<RemoteDesktopSettings />)
    await screen.findByText('此设备不支持远程桌面')
    expect(screen.queryByRole('switch')).toBeNull()
  })

  it('preserves the previous value on save failure and offers retry after read failures', async () => {
    mocks.setSettings.mockRejectedValue(new Error('secret error body'))
    render(<RemoteDesktopSettings />)
    await loaded()
    fireEvent.click(screen.getByRole('switch', { name: '允许控制鼠标键盘' }))
    await screen.findByRole('alert')
    expect(screen.getByRole('alert').textContent).toBe('无法保存远程桌面设置，请重试')
    expect(
      screen.getByRole('switch', { name: '允许控制鼠标键盘' }).getAttribute('aria-checked')
    ).toBe('false')
    mocks.getSettings.mockRejectedValueOnce(new Error('read failed'))
    fireEvent.click(screen.getByRole('button', { name: '重新检查权限' }))
    await screen.findByText('无法读取远程桌面设置，请重试')
    fireEvent.click(screen.getByRole('button', { name: '重新检查权限' }))
    await waitFor(() => expect(screen.queryByRole('alert')).toBeNull())
  })

  it('prevents concurrent mutations and cleans up the focus listener', async () => {
    let finish!: (value: typeof defaults) => void
    mocks.setSettings.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        })
    )
    const { unmount } = render(<RemoteDesktopSettings />)
    await loaded()
    fireEvent.click(screen.getByRole('switch', { name: '允许控制鼠标键盘' }))
    expect(screen.getByRole('switch', { name: '允许查看桌面' }).hasAttribute('disabled')).toBe(true)
    fireEvent.click(screen.getByRole('switch', { name: '允许查看桌面' }))
    expect(mocks.setSettings).toHaveBeenCalledTimes(1)
    await act(async () => finish({ ...defaults, controlEnabled: true }))
    const reads = mocks.getSettings.mock.calls.length
    unmount()
    fireEvent(window, new Event('focus'))
    expect(mocks.getSettings).toHaveBeenCalledTimes(reads)
  })
})
