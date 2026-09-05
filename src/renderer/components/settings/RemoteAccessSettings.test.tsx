import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const { getMock, setMock, isTauriRef } = vi.hoisted(() => ({
  getMock: vi.fn(),
  setMock: vi.fn(),
  isTauriRef: { current: true }
}))

vi.mock('@/lib/api', () => ({
  tunnelConfigApi: {
    get: (...args: unknown[]) => getMock(...args),
    set: (...args: unknown[]) => setMock(...args)
  }
}))

vi.mock('@/lib/tauri-runtime', () => ({
  isTauriContext: () => isTauriRef.current
}))

vi.mock('sonner', () => ({
  toast: { success: vi.fn(), error: vi.fn() }
}))

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key })
}))

import { RemoteAccessSettings } from './RemoteAccessSettings'

const VIEW = {
  provider: 'cloudflareQuick' as const,
  cloudflareNamedHostname: null,
  cloudflareNamedLocalPort: null,
  cloudflareNamedTokenSet: false,
  frpServerAddr: null,
  frpServerPort: null,
  frpCustomDomain: null,
  frpRemotePort: null,
  frpPublicHttps: true,
  frpTokenSet: false,
  sshHost: null,
  sshPort: null,
  sshUser: null,
  sshRemotePort: null,
  sshPublicHostname: null,
  sshPublicHttps: true,
  sshPrivateKeySet: false
}

describe('RemoteAccessSettings', () => {
  beforeEach(() => {
    getMock.mockReset()
    setMock.mockReset()
    isTauriRef.current = true
    getMock.mockResolvedValue({ success: true, data: VIEW })
    setMock.mockResolvedValue({
      success: true,
      data: { ...VIEW, provider: 'cloudflareNamed', cloudflareNamedHostname: 'se.example.com' }
    })
  })

  it('shows a desktop-only notice in the web client', () => {
    isTauriRef.current = false
    render(<RemoteAccessSettings />)
    expect(screen.getByText('remoteAccess.desktopOnly')).toBeDefined()
    expect(getMock).not.toHaveBeenCalled()
  })

  it('loads the current provider and persists a named-tunnel switch', async () => {
    render(<RemoteAccessSettings />)
    const select = await screen.findByLabelText('remoteAccess.provider')
    fireEvent.change(select, { target: { value: 'cloudflareNamed' } })
    await waitFor(() => {
      expect(setMock).toHaveBeenCalledWith(expect.objectContaining({ provider: 'cloudflareNamed' }))
    })
  })
})
