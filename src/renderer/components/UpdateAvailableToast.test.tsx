/**
 * Unit tests for UpdateAvailableToast download/install error surfacing.
 *
 * Regression coverage for the silent-failure bug: clicking Download or
 * Restart in the toast must surface store errors via toast.error instead of
 * doing nothing visible. Success paths must NOT show an error toast.
 */

import type { UpdateComponentPolicy } from '@shared/types/updater.types'
import { legacyUpdateComponentPolicy } from '@shared/types/updater.types'
import type { ReactElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { UpdateChannel } from '@/lib/tauri-updater-api'

vi.mock('sonner', () => ({
  toast: Object.assign(vi.fn(), {
    success: vi.fn(),
    error: vi.fn(),
    info: vi.fn(),
    loading: vi.fn(),
    dismiss: vi.fn()
  })
}))

const downloadUpdate = vi.fn(async () => {})
const installAndRestart = vi.fn(async () => {})
let storeError: string | null = null
let storeChannel: UpdateChannel = 'stable'
let storePolicy: UpdateComponentPolicy | null = null

function policyWith(
  actions: Partial<
    Record<
      keyof UpdateComponentPolicy['components'],
      UpdateComponentPolicy['components']['renderer']['action']
    >
  >
): UpdateComponentPolicy {
  const base = legacyUpdateComponentPolicy('1.2.3')
  const components = { ...base.components }
  for (const [component, action] of Object.entries(actions) as [
    keyof UpdateComponentPolicy['components'],
    UpdateComponentPolicy['components']['renderer']['action']
  ][]) {
    components[component] = { buildId: `build-${component}`, action }
  }
  return { ...base, metadataState: 'declared', components }
}

vi.mock('@/stores/updater-store', () => ({
  updaterStore: {
    // `showUpdateToast` reads `updateChannel` from the store to pick the
    // channel prefix + manual-download action label. Without `updateChannel`
    // here it reads `undefined` and produces "A new undefined build...".
    getState: () => ({
      downloadUpdate,
      installAndRestart,
      error: storeError,
      updateChannel: storeChannel,
      componentPolicy: storePolicy
    })
  },
  // Hooks are unused by the functions under test but imported by the module.
  useUpdaterState: vi.fn(),
  useUpdaterActions: vi.fn(),
  useUpdateVersion: vi.fn(),
  useUpdateDownloaded: vi.fn(),
  useIsDownloading: vi.fn(),
  useDownloadProgress: vi.fn()
}))

vi.mock('@/lib/tauri-updater-api', () => ({
  isAurUpdateMode: vi.fn(() => false)
}))

const confirmMock = vi.fn(async (_message: string, _options?: unknown) => true)
vi.mock('@tauri-apps/plugin-dialog', () => ({
  confirm: (message: string, options?: unknown) => confirmMock(message, options)
}))

const hasActiveTerminalSessions = vi.fn(() => false)
vi.mock('@/lib/tauri-safe-update', () => ({
  hasActiveTerminalSessions: () => hasActiveTerminalSessions()
}))

import { toast } from 'sonner'
import { isAurUpdateMode } from '@/lib/tauri-updater-api'
import { showUpdateDownloadedToast, showUpdateToast } from './UpdateAvailableToast'

type ToastAction = { onClick: () => void | Promise<void> }

function lastToastAction(mockFn: ReturnType<typeof vi.fn>): ToastAction {
  const calls = mockFn.mock.calls
  const opts = calls[calls.length - 1][1] as { action: ToastAction }
  return opts.action
}

describe('UpdateAvailableToast error surfacing', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    storeError = null
    storeChannel = 'stable'
    storePolicy = null
    downloadUpdate.mockResolvedValue(undefined)
    installAndRestart.mockResolvedValue(undefined)
    confirmMock.mockResolvedValue(false)
    hasActiveTerminalSessions.mockReturnValue(false)
    vi.mocked(isAurUpdateMode).mockReturnValue(false)
  })

  it('does not show an error toast when download succeeds', async () => {
    showUpdateToast('0.3.8')
    const action = lastToastAction(vi.mocked(toast.success))

    await action.onClick()

    expect(downloadUpdate).toHaveBeenCalledTimes(1)
    expect(vi.mocked(toast.error)).not.toHaveBeenCalled()
  })

  it('shows an error toast when download fails (store error set)', async () => {
    storeError = 'signature verification failed'
    showUpdateToast('0.3.8')
    const action = lastToastAction(vi.mocked(toast.success))

    await action.onClick()

    expect(downloadUpdate).toHaveBeenCalledTimes(1)
    expect(vi.mocked(toast.error)).toHaveBeenCalledWith(
      'Update download failed',
      expect.objectContaining({ description: 'signature verification failed' })
    )
  })

  it('shows an error toast when install/restart fails (store error set)', async () => {
    confirmMock.mockResolvedValue(true)
    storeError = 'relaunch failed'
    showUpdateDownloadedToast('0.3.8')
    const action = lastToastAction(vi.mocked(toast.success))

    await action.onClick()

    expect(confirmMock).toHaveBeenCalledTimes(1)
    expect(installAndRestart).toHaveBeenCalledTimes(1)
    expect(vi.mocked(toast.error)).toHaveBeenCalledWith(
      'Update install failed',
      expect.objectContaining({ description: 'relaunch failed' })
    )
  })

  it('does not show an error toast when install/restart succeeds', async () => {
    confirmMock.mockResolvedValue(true)
    showUpdateDownloadedToast('0.3.8')
    const action = lastToastAction(vi.mocked(toast.success))

    await action.onClick()

    expect(confirmMock).toHaveBeenCalledTimes(1)
    expect(installAndRestart).toHaveBeenCalledTimes(1)
    expect(vi.mocked(toast.error)).not.toHaveBeenCalled()
  })

  it('does not install when the user cancels the confirmation dialog', async () => {
    confirmMock.mockResolvedValue(false)
    showUpdateDownloadedToast('0.3.8')
    const action = lastToastAction(vi.mocked(toast.success))

    await action.onClick()

    expect(confirmMock).toHaveBeenCalledTimes(1)
    expect(installAndRestart).not.toHaveBeenCalled()
    expect(vi.mocked(toast.error)).not.toHaveBeenCalled()
  })

  it('does not treat active terminals as a Core defer when metadata is missing', async () => {
    hasActiveTerminalSessions.mockReturnValue(true)
    confirmMock.mockResolvedValue(false)
    showUpdateDownloadedToast('0.3.8')
    const action = lastToastAction(vi.mocked(toast.success))

    await action.onClick()

    const message = confirmMock.mock.calls[0][0] as string
    expect(message).toContain('The app window always restarts.')
    expect(message).toContain(
      'Restarting the app window itself does not stop terminals owned by Terminal Core.'
    )
    expect(message).toContain('Component metadata is unavailable')
    expect(message).not.toContain('Replacement waits while a terminal is active.')
    expect(message).not.toContain('terminal sessions will be closed')
    expect(installAndRestart).not.toHaveBeenCalled()
  })

  it('keeps a matching Core connected and defers only a mismatched active Terminal', async () => {
    hasActiveTerminalSessions.mockReturnValue(true)
    storePolicy = policyWith({
      renderer: 'restart',
      guiNative: 'restart',
      acpCore: 'preserve',
      terminalCore: 'preserve'
    })
    showUpdateDownloadedToast('1.2.3')
    let action = lastToastAction(vi.mocked(toast.success))
    await action.onClick()

    const matching = confirmMock.mock.calls[0][0] as string
    expect(matching).toContain(
      'Kept and reconnected because the build matches: ACP Core, Terminal Core.'
    )
    expect(matching).not.toContain('Replacement waits while a terminal is active.')
    expect(matching).toContain('This is not a zero-interruption swap.')
    expect(installAndRestart).not.toHaveBeenCalled()

    confirmMock.mockClear()
    storePolicy = policyWith({
      acpCore: 'preserve',
      terminalCore: 'defer-if-active'
    })
    showUpdateDownloadedToast('1.2.3')
    action = lastToastAction(vi.mocked(toast.success))
    await action.onClick()

    const deferred = confirmMock.mock.calls[0][0] as string
    expect(deferred).toContain(
      'Terminal Core build identity is checked. If it differs, replacement waits while a terminal is active.'
    )
    expect(deferred).toContain('Kept and reconnected because the build matches: ACP Core.')
    expect(deferred).not.toContain('No terminal is active')
  })

  it('replaces an idle mismatched Terminal instead of deferring it', async () => {
    hasActiveTerminalSessions.mockReturnValue(false)
    storePolicy = policyWith({ terminalCore: 'defer-if-active', acpCore: 'restart' })
    showUpdateDownloadedToast('1.2.3')
    const action = lastToastAction(vi.mocked(toast.success))

    await action.onClick()

    const message = confirmMock.mock.calls[0][0] as string
    expect(message).toContain('no terminal is active, so it can be replaced')
    expect(message).toContain(
      'Build identity is checked for ACP Core. A Core is replaced only if its identity differs'
    )
    expect(message).not.toContain('Replacement waits while a terminal is active.')
  })

  it('filters preserve out of the impact lines and does not promise a zero-interruption swap', () => {
    storePolicy = policyWith({
      renderer: 'restart',
      guiNative: 'preserve',
      acpCore: 'preserve',
      terminalCore: 'restart'
    })
    showUpdateToast('1.2.3')
    const calls = vi.mocked(toast.success).mock.calls
    const description = (calls[calls.length - 1][1] as { description: string }).description

    expect(description).toContain('The app window always restarts.')
    expect(description).toContain('Kept and reconnected because the build matches: ACP Core.')
    expect(description).toContain('Renderer restarts with the app window.')
    expect(description).toContain('Terminal Core is checked by build identity')
    expect(description).not.toContain('Desktop runtime restarts')
    expect(description).not.toContain('build-acpCore')
    expect(description).toContain('not a zero-interruption swap')
  })
})

describe('UpdateAvailableToast channel-aware labeling', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    storeChannel = 'stable'
    storeError = null
    storePolicy = null
    vi.mocked(isAurUpdateMode).mockReturnValue(false)
  })

  it('uses the stable title, description, and Download action label by default', () => {
    showUpdateToast('0.4.8')
    const calls = vi.mocked(toast.success).mock.calls
    const [title, opts] = calls[calls.length - 1] as [
      string,
      { description: string; action: { label: ReactElement } }
    ]
    expect(title).toBe('Update available: version 0.4.8')
    expect(opts.description).toContain('A new version is available for download.')
    expect(renderToStaticMarkup(opts.action.label)).toContain('Download')
    // The stable action label must NOT show the manual-download CTA.
    expect(renderToStaticMarkup(opts.action.label)).not.toContain('Open Download Page')
  })

  it('prefixes the title with "Insider" and uses the manual-download action label for insider', () => {
    storeChannel = 'insider'
    showUpdateToast('0.5.0-rc.1')
    const calls = vi.mocked(toast.success).mock.calls
    const [title, opts] = calls[calls.length - 1] as [
      string,
      { description: string; action: { label: ReactElement } }
    ]
    expect(title).toBe('Insider Update available: version 0.5.0-rc.1')
    expect(opts.description).toContain(
      'A new insider build is available. Open the download page to install it manually.'
    )
    expect(renderToStaticMarkup(opts.action.label)).toContain('Open Download Page')
  })

  it('prefixes the title with "Nightly" for the nightly channel', () => {
    storeChannel = 'nightly'
    showUpdateToast('0.0.0-nightly.20260808.abc')
    const calls = vi.mocked(toast.success).mock.calls
    const [title, opts] = calls[calls.length - 1] as [string, { description: string }]
    expect(title).toBe('Nightly Update available: version 0.0.0-nightly.20260808.abc')
    expect(opts.description).toContain(
      'A new nightly build is available. Open the download page to install it manually.'
    )
  })

  it('keeps the AUR instruction and yay action unchanged', async () => {
    vi.mocked(isAurUpdateMode).mockReturnValue(true)
    showUpdateToast('1.2.3')

    const calls = vi.mocked(toast.success).mock.calls
    const [, opts] = calls[calls.length - 1] as [
      string,
      { description: string; action: { label: ReactElement; onClick: () => Promise<void> } }
    ]
    expect(opts.description).toContain('A new version is available. Update with yay.')
    expect(renderToStaticMarkup(opts.action.label)).toContain('Use yay')
    expect(opts.description).not.toContain('Open the download page')

    await opts.action.onClick()
    expect(vi.mocked(toast.info)).toHaveBeenCalledWith('Run in terminal', {
      description: 'yay -S termul-manager'
    })
    expect(downloadUpdate).not.toHaveBeenCalled()
  })
})
