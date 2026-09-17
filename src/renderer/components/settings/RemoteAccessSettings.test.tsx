import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const { getMock, setMock, listHostsMock, isTauriRef } = vi.hoisted(() => ({
  getMock: vi.fn(),
  setMock: vi.fn(),
  listHostsMock: vi.fn(),
  isTauriRef: { current: true }
}))

vi.mock('@/lib/api', () => ({
  tunnelConfigApi: {
    get: (...args: unknown[]) => getMock(...args),
    set: (...args: unknown[]) => setMock(...args),
    listSshHosts: (...args: unknown[]) => listHostsMock(...args)
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
  frpPublicHttps: false,
  frpTokenSet: false,
  sshHost: null,
  sshPort: null,
  sshUser: null,
  sshRemotePort: null,
  sshPublicHostname: null,
  sshIdentityFile: null,
  sshPublicHttps: false,
  sshPrivateKeySet: false,
  sshPasswordSet: false
}

describe('RemoteAccessSettings', () => {
  beforeEach(() => {
    getMock.mockReset()
    setMock.mockReset()
    listHostsMock.mockReset()
    isTauriRef.current = true
    getMock.mockResolvedValue({ success: true, data: VIEW })
    listHostsMock.mockResolvedValue({ success: true, data: [] })
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

  it('imports an OpenSSH config host into the reverse-tunnel form', async () => {
    getMock.mockResolvedValue({
      success: true,
      data: { ...VIEW, provider: 'sshReverse' }
    })
    listHostsMock.mockResolvedValue({
      success: true,
      data: [
        {
          name: 'vps',
          host: '1.2.3.4',
          port: 22,
          username: 'ubuntu',
          authMethod: 'key',
          privateKeyPath: '/home/u/.ssh/id_ed25519'
        }
      ]
    })
    render(<RemoteAccessSettings />)
    const picker = await screen.findByLabelText('remoteAccess.sshConfigHost')
    fireEvent.change(picker, { target: { value: 'vps' } })
    expect((screen.getByLabelText('remoteAccess.sshHost') as HTMLInputElement).value).toBe(
      '1.2.3.4'
    )
    expect((screen.getByLabelText('remoteAccess.sshUser') as HTMLInputElement).value).toBe('ubuntu')
    expect(screen.queryByText('remoteAccess.sshPublicHttps')).toBeNull()
    expect(screen.getByLabelText('remoteAccess.sshPassword')).toBeDefined()
  })
})
