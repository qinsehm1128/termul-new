import type { RemoteStatus } from '@shared/types/ipc.types'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { RemoteAccessPopover } from './RemoteAccessPopover'

const mocks = vi.hoisted(() => ({
  status: vi.fn(),
  setStatus: vi.fn(),
  start: vi.fn(),
  stop: vi.fn(),
  intent: vi.fn(),
  setIntent: vi.fn(),
  sync: vi.fn(),
  pairing: vi.fn(),
  clipboard: vi.fn()
}))
vi.mock('qrcode.react', () => ({
  QRCodeSVG: ({ value }: { value: string }) => <div data-testid="qr" data-value={value} />
}))
vi.mock('@/components/DeviceManagementPanel', () => ({
  DeviceManagementPanel: () => <div>设备管理</div>
}))
vi.mock('@/stores/remote-status-store', () => ({
  useRemoteStatus: mocks.status,
  useRemoteRestoreError: () => null,
  useRemoteStatusStore: {
    getState: () => ({ setStatus: mocks.setStatus, setRestoreError: vi.fn() })
  }
}))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn().mockResolvedValue(undefined) }))
vi.mock('@/lib/remote-devices-api', () => ({ remoteDevicesApi: { pairingCode: mocks.pairing } }))
vi.mock('@/lib/api', () => ({
  remoteServerApi: {
    start: mocks.start,
    stop: mocks.stop,
    intent: mocks.intent,
    setIntent: mocks.setIntent
  },
  syncProjects: mocks.sync
}))
vi.mock('@/stores/project-store', () => ({
  useProjectStore: { getState: () => ({ projects: [], groups: [], activeProjectId: 'p1' }) }
}))
vi.mock('@/hooks/use-projects-persistence', () => ({
  toProjectSummaries: () => [],
  toProjectGroupSummaries: () => []
}))
vi.mock('sonner', () => ({ toast: { error: vi.fn() } }))

const SECRET = 'secret-bootstrap-credential'
const RUNNING: RemoteStatus = {
  running: true,
  url: 'http://127.0.0.1:5123',
  port: 5123,
  bindMode: 'localhost',
  bindHost: '127.0.0.1',
  tunnelUrl: 'https://foo.trycloudflare.com',
  accessUrl: `https://foo.trycloudflare.com/#access_token=${SECRET}`
}
const STOPPED: RemoteStatus = {
  running: false,
  url: null,
  port: null,
  bindMode: null,
  bindHost: null,
  tunnelUrl: null,
  accessUrl: null
}
async function open(): Promise<HTMLElement> {
  fireEvent.click(screen.getByLabelText('Remote terminal access'))
  return screen.findByRole('switch')
}
beforeEach(() => {
  vi.clearAllMocks()
  mocks.status.mockReturnValue(null)
  mocks.intent.mockResolvedValue({ success: true, data: { wanted: false, publishMode: 'tunnel' } })
  mocks.setIntent.mockResolvedValue({ success: true })
  mocks.sync.mockResolvedValue({ success: true })
  mocks.pairing.mockImplementation(async () => ({
    code: 'ONE_TIME_CODE',
    expiresAt: Math.floor(Date.now() / 1000) + 60
  }))
  mocks.clipboard.mockResolvedValue(undefined)
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: { writeText: mocks.clipboard }
  })
})
afterEach(() => vi.useRealTimers())

describe('RemoteAccessPopover', () => {
  it('keeps the closed trigger inert without requesting a pairing secret', () => {
    render(<RemoteAccessPopover />)
    expect(screen.getByLabelText('Remote terminal access').getAttribute('aria-pressed')).toBe(
      'false'
    )
    expect(mocks.pairing).not.toHaveBeenCalled()
  })
  it('issues one-time pairing QR/copy links, never a host bearer', async () => {
    mocks.status.mockReturnValue(RUNNING)
    render(<RemoteAccessPopover />)
    await open()
    const qr = await screen.findByTestId('qr')
    expect(qr.getAttribute('data-value')).toBe('https://foo.trycloudflare.com/#pair=ONE_TIME_CODE')
    fireEvent.click(screen.getByRole('button', { name: 'Copy access link' }))
    await waitFor(() =>
      expect(mocks.clipboard).toHaveBeenCalledWith(
        'https://foo.trycloudflare.com/#pair=ONE_TIME_CODE'
      )
    )
    expect(document.body.textContent).not.toContain(SECRET)
    expect(document.body.innerHTML).not.toContain('access_token')
    expect(screen.getByText('设备管理')).toBeDefined()
  })
  it('builds a pairing link from a public origin even when legacy accessUrl is absent', async () => {
    mocks.status.mockReturnValue({ ...RUNNING, accessUrl: null })
    render(<RemoteAccessPopover />)
    await open()
    expect((await screen.findByTestId('qr')).getAttribute('data-value')).toContain('#pair=')
  })
  it('does not show a legacy credential when pairing-code issuance fails', async () => {
    mocks.status.mockReturnValue(RUNNING)
    mocks.pairing.mockRejectedValue(new Error(SECRET))
    render(<RemoteAccessPopover />)
    await open()
    expect(await screen.findByText('配对码生成失败 · 重试')).toBeDefined()
    expect(screen.queryByTestId('qr')).toBeNull()
    expect(document.body.innerHTML).not.toContain(SECRET)
  })
  it('starts with the selected bind mode and seeds projects', async () => {
    mocks.start.mockResolvedValue({ success: true, data: RUNNING })
    render(<RemoteAccessPopover />)
    fireEvent.click(await open())
    await waitFor(() => expect(mocks.sync).toHaveBeenCalledTimes(1))
    expect(mocks.start).toHaveBeenCalledWith({ bindMode: 'localhost' })
    expect(mocks.setStatus).toHaveBeenCalledWith(RUNNING)
  })
  it('shows a safe inline error when the start fails', async () => {
    mocks.start.mockResolvedValue({ success: false, error: SECRET })
    render(<RemoteAccessPopover />)
    fireEvent.click(await open())
    expect(await screen.findByRole('alert')).toHaveProperty('textContent', '远程服务切换失败')
    expect(screen.queryByTestId('qr')).toBeNull()
  })
  it('selects LAN and strips host credentials from its pairing URL', async () => {
    mocks.status.mockReturnValue({
      ...RUNNING,
      publishMode: 'lan',
      bindMode: 'all',
      lanAccessUrl: `http://192.168.1.8:5123/?token=${SECRET}#access_token=${SECRET}`,
      lanUrl: null
    })
    render(<RemoteAccessPopover />)
    await open()
    expect((await screen.findByTestId('qr')).getAttribute('data-value')).toBe(
      'http://192.168.1.8:5123/#pair=ONE_TIME_CODE'
    )
    expect(screen.getByRole('tab', { name: '局域网' }).getAttribute('aria-selected')).toBe('true')
    expect(document.body.innerHTML).not.toContain(SECRET)
  })
  it('discards a pairing response from a previous host generation', async () => {
    let resolve!: (value: { code: string; expiresAt: number }) => void
    mocks.status.mockReturnValue(RUNNING)
    mocks.pairing.mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done
        })
    )
    const view = render(<RemoteAccessPopover />)
    await open()
    mocks.status.mockReturnValue({ ...RUNNING, tunnelUrl: 'https://next.trycloudflare.com' })
    view.rerender(<RemoteAccessPopover />)
    await screen.findByTestId('qr')
    await act(async () => resolve({ code: 'OLD_CODE', expiresAt: Date.now() / 1000 + 60 }))
    expect(screen.getByTestId('qr').getAttribute('data-value')).toBe(
      'https://next.trycloudflare.com/#pair=ONE_TIME_CODE'
    )
  })
  it('refreshes on the 60-second TTL and removes the expired QR while issuing a new code', async () => {
    mocks.status.mockReturnValue(RUNNING)
    render(<RemoteAccessPopover />)
    await open()
    await screen.findByTestId('qr')
    vi.useFakeTimers()
    mocks.pairing.mockImplementationOnce(() => new Promise(() => {}))
    // Reissue so the refresh timer is registered under fake timers.
    fireEvent.click(screen.getByRole('button', { name: '刷新' }))
    await act(async () => {})
    expect(screen.queryByTestId('qr')).toBeNull()
    mocks.pairing.mockImplementation(async () => ({
      code: 'NEW_CODE',
      expiresAt: Date.now() / 1000 + 60
    }))
    fireEvent.click(screen.getByLabelText('Remote terminal access'))
    fireEvent.click(screen.getByLabelText('Remote terminal access'))
    await act(async () => {})
    expect(screen.getByTestId('qr').getAttribute('data-value')).toContain('NEW_CODE')
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60001)
    })
    expect(mocks.pairing.mock.calls.length).toBeGreaterThanOrEqual(4)
  })
  it('rejects already expired pairing codes instead of drawing them', async () => {
    mocks.status.mockReturnValue(RUNNING)
    mocks.pairing.mockResolvedValue({
      code: 'EXPIRED',
      expiresAt: Math.floor(Date.now() / 1000) - 1
    })
    render(<RemoteAccessPopover />)
    await open()
    await screen.findByText('配对码生成失败 · 重试')
    expect(screen.queryByTestId('qr')).toBeNull()
  })
  it('never reuses an issued code when the public host origin changes', async () => {
    mocks.status.mockReturnValue(RUNNING)
    const view = render(<RemoteAccessPopover />)
    await open()
    await screen.findByTestId('qr')
    mocks.pairing.mockImplementationOnce(() => new Promise(() => {}))
    mocks.status.mockReturnValue({ ...RUNNING, tunnelUrl: 'https://replacement.trycloudflare.com' })
    view.rerender(<RemoteAccessPopover />)
    expect(screen.queryByTestId('qr')).toBeNull()
  })
  it('persists the selected publish mode and restarts an active host with its bind mode', async () => {
    mocks.status.mockReturnValue(RUNNING)
    mocks.stop.mockResolvedValue({ success: true, data: STOPPED })
    mocks.start.mockResolvedValue({
      success: true,
      data: { ...RUNNING, bindMode: 'all', publishMode: 'lan' }
    })
    render(<RemoteAccessPopover />)
    await open()
    fireEvent.click(screen.getByRole('tab', { name: '局域网' }))
    await waitFor(() => expect(mocks.sync).toHaveBeenCalledTimes(1))
    expect(mocks.setIntent).toHaveBeenCalledWith({ publishMode: 'lan' })
    expect(mocks.stop).toHaveBeenCalledTimes(1)
    expect(mocks.start).toHaveBeenCalledWith({ bindMode: 'all' })
  })
  it('stops service and removes pairing QR when status stops', async () => {
    mocks.status.mockReturnValue(RUNNING)
    mocks.stop.mockResolvedValue({ success: true, data: STOPPED })
    const view = render(<RemoteAccessPopover />)
    const toggle = await open()
    await screen.findByTestId('qr')
    fireEvent.click(toggle)
    await waitFor(() => expect(mocks.setStatus).toHaveBeenCalledWith(STOPPED))
    mocks.status.mockReturnValue(STOPPED)
    view.rerender(<RemoteAccessPopover />)
    expect(screen.queryByTestId('qr')).toBeNull()
  })
})
