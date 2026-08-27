import type { RemoteStatus } from '@shared/types/ipc.types'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import { RemoteAccessPopover } from './RemoteAccessPopover'

// QR mock: expose the encoded value so tests assert the URL the QR would draw,
// without depending on qrcode.react's SVG internals under jsdom.
vi.mock('qrcode.react', () => ({
  QRCodeSVG: ({ value }: { value: string }) => <div data-testid="qr" data-value={value} />
}))

// Remote status store: `useRemoteStatus` is configurable per test; `getState`
// is used only on the toggle-success path.
const setStatus = vi.fn()
vi.mock('@/stores/remote-status-store', () => ({
  useRemoteStatus: vi.fn((): RemoteStatus | null => null),
  useRemoteRestoreError: vi.fn((): string | null => null),
  useRemoteStatusStore: { getState: () => ({ setStatus, setRestoreError: vi.fn() }) }
}))

const startMock = vi.fn()
const stopMock = vi.fn()
const intentMock = vi.fn()
const setIntentMock = vi.fn()
const rotateMock = vi.fn()
vi.mock('@/lib/api', () => ({
  remoteServerApi: {
    start: (...args: unknown[]) => startMock(...args),
    stop: () => stopMock(),
    status: vi.fn(),
    intent: (...args: unknown[]) => intentMock(...args),
    setIntent: (...args: unknown[]) => setIntentMock(...args),
    rotateCredential: (...args: unknown[]) => rotateMock(...args)
  },
  syncProjects: vi.fn(() => Promise.resolve({ success: true, data: undefined }))
}))

vi.mock('@/stores/project-store', () => ({
  useProjectStore: {
    getState: () => ({ projects: [], activeProjectId: null })
  }
}))

vi.mock('@/stores/acp-store', () => ({
  useAcpStore: {
    getState: () => ({
      loadSessionIndex: vi.fn(async () => {}),
      sessionIndex: [],
      messages: {}
    })
  }
}))

vi.mock('@/hooks/use-projects-persistence', () => ({
  toProjectSummaries: vi.fn(() => []),
  toProjectGroupSummaries: vi.fn(() => [])
}))

vi.mock('@/lib/acp-history-persistence', () => ({
  toPersistedSessionSummaries: vi.fn(() => [])
}))

// Silence sonner toasts in test output.
vi.mock('sonner', () => ({ toast: { error: vi.fn() } }))

import { syncProjects } from '@/lib/api'
import { useRemoteStatus } from '@/stores/remote-status-store'

const RAW_CREDENTIAL = 'secret-bootstrap-credential'
const NEXT_RAW_CREDENTIAL = 'rotated-bootstrap-credential'
const RUNNING: RemoteStatus = {
  running: true,
  url: 'http://127.0.0.1:5123',
  port: 5123,
  bindMode: 'localhost',
  bindHost: '127.0.0.1',
  tunnelUrl: 'https://foo-bar.trycloudflare.com',
  accessUrl: `https://foo-bar.trycloudflare.com/#access_token=${RAW_CREDENTIAL}`
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
const RUNNING_AGAIN: RemoteStatus = {
  ...RUNNING,
  port: 6124,
  url: 'http://127.0.0.1:6124',
  tunnelUrl: 'https://new-generation.trycloudflare.com',
  accessUrl: `https://new-generation.trycloudflare.com/#access_token=${NEXT_RAW_CREDENTIAL}`
}
const clipboardWrite = vi.fn(async () => undefined)

function renderPopover(): ReturnType<typeof render> {
  return render(
    <TooltipProvider>
      <RemoteAccessPopover />
    </TooltipProvider>
  )
}

/** Click the StatusBar trigger to open the Radix popover, then wait for the
 * toggle switch to mount (the content is portalled into document.body). */
async function openPopover(): Promise<HTMLElement> {
  const trigger = screen.getByLabelText('Remote terminal access')
  await fireEvent.click(trigger)
  return screen.findByRole('switch')
}

beforeEach(() => {
  vi.clearAllMocks()
  intentMock.mockResolvedValue({ success: true, data: { wanted: false, publishMode: 'tunnel' } })
  setIntentMock.mockResolvedValue({ success: true, data: { wanted: true, publishMode: 'tunnel' } })
  rotateMock.mockResolvedValue({ success: true, data: RUNNING })
  vi.mocked(useRemoteStatus).mockReturnValue(null)
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: { writeText: clipboardWrite }
  })
})

afterEach(() => {
  vi.mocked(useRemoteStatus).mockReturnValue(null)
})

describe('RemoteAccessPopover', () => {
  it('shows the globe trigger with the toggle off when no status', () => {
    renderPopover()
    const trigger = screen.getByLabelText('Remote terminal access')
    expect(trigger.getAttribute('aria-pressed')).toBe('false')
    // Popover is closed → no QR / copy-link in the DOM yet.
    expect(screen.queryByTestId('qr')).toBeNull()
    expect(screen.queryByText('Copy link')).toBeNull()
  })

  it('uses only the credentialed accessUrl for QR/copy without displaying the raw credential', async () => {
    vi.mocked(useRemoteStatus).mockReturnValue(RUNNING)
    renderPopover()
    await openPopover()

    const qr = screen.getByTestId('qr')
    expect(qr.getAttribute('data-value')).toBe(RUNNING.accessUrl)
    expect(qr.getAttribute('data-value')).not.toBe(RUNNING.tunnelUrl)
    expect(screen.queryByText(RAW_CREDENTIAL)).toBeNull()
    expect(document.body.textContent).not.toContain(RAW_CREDENTIAL)

    const copyButton = screen.getByRole('button', { name: 'Copy access link' })
    await fireEvent.click(copyButton)
    await waitFor(() => expect(clipboardWrite).toHaveBeenCalledWith(RUNNING.accessUrl))

    // The simplify goal: no bind selector, no open-in-browser text row.
    expect(screen.queryByText('Listen on')).toBeNull()
    expect(screen.queryByText('Open in browser')).toBeNull()
    expect(await screen.findByText('Copied')).toBeDefined()
  })

  it('renders a newly generated local Desktop access URL and QR after host generation starts', async () => {
    vi.mocked(useRemoteStatus).mockReturnValue(RUNNING)
    stopMock.mockResolvedValueOnce({ success: true, data: STOPPED })
    startMock.mockResolvedValueOnce({ success: true, data: RUNNING_AGAIN })
    const view = renderPopover()
    let toggle = await openPopover()

    expect(screen.getByTestId('qr').getAttribute('data-value')).toBe(RUNNING.accessUrl)
    await fireEvent.click(toggle)
    await waitFor(() => expect(stopMock).toHaveBeenCalledTimes(1))
    expect(setStatus).toHaveBeenLastCalledWith(STOPPED)

    vi.mocked(useRemoteStatus).mockReturnValue(STOPPED)
    view.rerender(
      <TooltipProvider>
        <RemoteAccessPopover />
      </TooltipProvider>
    )
    expect(screen.queryByTestId('qr')).toBeNull()
    expect(document.body.textContent).not.toContain(RAW_CREDENTIAL)

    toggle = screen.getByRole('switch')
    await fireEvent.click(toggle)
    await waitFor(() => expect(startMock).toHaveBeenCalledTimes(1))
    expect(setStatus).toHaveBeenLastCalledWith(RUNNING_AGAIN)

    vi.mocked(useRemoteStatus).mockReturnValue(RUNNING_AGAIN)
    view.rerender(
      <TooltipProvider>
        <RemoteAccessPopover />
      </TooltipProvider>
    )
    const rotatedQr = screen.getByTestId('qr')
    expect(rotatedQr.getAttribute('data-value')).toBe(RUNNING_AGAIN.accessUrl)
    expect(rotatedQr.getAttribute('data-value')).not.toBe(RUNNING.accessUrl)
    expect(document.body.textContent).not.toContain(RAW_CREDENTIAL)
    expect(document.body.textContent).not.toContain(NEXT_RAW_CREDENTIAL)

    await fireEvent.click(screen.getByRole('button', { name: 'Copy access link' }))
    await waitFor(() => expect(clipboardWrite).toHaveBeenLastCalledWith(RUNNING_AGAIN.accessUrl))
    expect(clipboardWrite).not.toHaveBeenCalledWith(RUNNING.accessUrl)

    const revokedPeerFrame = {
      type: 'reauthentication_required',
      payload: { code: 'REAUTHENTICATION_REQUIRED' }
    }
    expect(JSON.stringify(revokedPeerFrame)).not.toContain(NEXT_RAW_CREDENTIAL)
    expect(rotatedQr.getAttribute('data-value')).toBe(RUNNING_AGAIN.accessUrl)
  })

  it('does not fall back to an uncredentialed tunnel URL when accessUrl is explicitly absent', async () => {
    vi.mocked(useRemoteStatus).mockReturnValue({ ...RUNNING, accessUrl: null })
    renderPopover()
    await openPopover()

    expect(screen.queryByTestId('qr')).toBeNull()
    expect(screen.queryByText('Copy link')).toBeNull()
  })

  it('shows an inline error when start fails (no QR)', async () => {
    startMock.mockResolvedValueOnce({ success: false, error: 'tunnel down' })
    renderPopover()

    const toggle = await openPopover()
    await fireEvent.click(toggle)

    expect(await screen.findByText('tunnel down')).toBeDefined()
    expect(screen.queryByTestId('qr')).toBeNull()
  })

  it('starts remote access with the selected publish bind mode and seeds the web client', async () => {
    startMock.mockResolvedValueOnce({ success: true, data: RUNNING })
    renderPopover()

    const toggle = await openPopover()
    await fireEvent.click(toggle)

    await waitFor(() => {
      expect(startMock).toHaveBeenCalledTimes(1)
    })
    expect(startMock).toHaveBeenCalledWith({ bindMode: 'localhost' })
    expect(setStatus).toHaveBeenCalledWith(RUNNING)
    // Project metadata is seeded; chat history is read directly from the
    // durable Rust provider by the desktop-hosted browser.
    await waitFor(() => {
      expect(syncProjects).toHaveBeenCalledTimes(1)
    })
  })

  it('uses the credentialed LAN URL when local-network publish is selected', async () => {
    const lanAccess = `http://192.168.1.8:5123/#access_token=${RAW_CREDENTIAL}`
    intentMock.mockResolvedValue({ success: true, data: { wanted: true, publishMode: 'lan' } })
    vi.mocked(useRemoteStatus).mockReturnValue({
      ...RUNNING,
      bindMode: 'all',
      bindHost: '0.0.0.0',
      publishMode: 'lan',
      lanUrl: 'http://192.168.1.8:5123',
      lanAccessUrl: lanAccess,
      accessUrl: lanAccess,
      tunnelAccessUrl: RUNNING.accessUrl
    })
    renderPopover()
    await openPopover()
    expect(screen.getByRole('tab', { name: 'Local network' }).getAttribute('aria-selected')).toBe(
      'true'
    )
    expect(screen.getByTestId('qr').getAttribute('data-value')).toBe(lanAccess)
    expect(document.body.textContent).not.toContain(RAW_CREDENTIAL)
    expect(screen.getByText('Same Wi-Fi. HTTP plus the secret in the link.')).toBeDefined()
  })
})
