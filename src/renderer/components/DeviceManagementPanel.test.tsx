import { REMOTE_DEVICE_SCOPES, type RemoteDevice } from '@shared/remote-devices'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { DeviceManagementPanel } from './DeviceManagementPanel'

const api = vi.hoisted(() => ({
  list: vi.fn(),
  rename: vi.fn(),
  setPermissions: vi.fn(),
  grantAll: vi.fn(),
  disconnect: vi.fn(),
  revoke: vi.fn(),
  disconnectAll: vi.fn(),
  audit: vi.fn()
}))
vi.mock('@/lib/remote-devices-api', () => ({ remoteDevicesApi: api }))
vi.mock('@/stores/project-store', () => ({
  useProjectStore: (select: (state: { activeProjectId: string }) => unknown) =>
    select({ activeProjectId: 'p1' })
}))
const phone: RemoteDevice = {
  id: 'phone',
  name: '手机',
  platform: 'ios',
  createdAt: 1794210000,
  lastSeen: 1794210020,
  scopes: ['view'],
  revokedAt: null,
  legacy: false,
  projectId: 'p1',
  connectionCount: 2
}
const legacy: RemoteDevice = {
  ...phone,
  id: 'legacy-host',
  name: '旧版主机',
  legacy: true,
  projectId: null
}
beforeEach(() => {
  vi.clearAllMocks()
  api.list.mockResolvedValue({
    devices: [phone, { ...phone, id: 'other', name: '其他项目设备', projectId: 'p2' }, legacy],
    revision: 1
  })
  for (const name of [
    'rename',
    'setPermissions',
    'grantAll',
    'disconnect',
    'revoke',
    'disconnectAll'
  ] as const)
    api[name].mockResolvedValue(undefined)
  api.audit.mockResolvedValue([{ at: 1794210020, event: 'device_disconnect', deviceId: 'phone' }])
})

describe('DeviceManagementPanel', () => {
  it('filters by current project and aggregates sockets into a device row', async () => {
    render(<DeviceManagementPanel />)
    const row = await screen.findByRole('listitem', { name: '手机' })
    expect(within(row).getByText('在线 · 2 个连接')).toBeDefined()
    expect(screen.queryByText('其他项目设备')).toBeNull()
    fireEvent.click(screen.getByLabelText('全部设备'))
    expect(screen.getByText('其他项目设备')).toBeDefined()
    const old = screen.getByRole('listitem', { name: '旧版主机' })
    expect(within(old).getByText('旧版主机凭据 · 不可单独撤销')).toBeDefined()
    expect(within(old).queryByRole('button', { name: '撤销配对' })).toBeNull()
    expect(within(old).queryByRole('button', { name: '全部授权' })).toBeNull()
  })
  it('updates a single scope, grants all separately, renames and reloads', async () => {
    render(<DeviceManagementPanel />)
    const row = await screen.findByRole('listitem', { name: '手机' })
    fireEvent.click(within(row).getByText('设备与权限'))
    expect(within(row).getAllByRole('checkbox')).toHaveLength(REMOTE_DEVICE_SCOPES.length)
    fireEvent.click(within(row).getByLabelText('终端输入'))
    await waitFor(() =>
      expect(api.setPermissions).toHaveBeenCalledWith('phone', ['view', 'terminal.input'])
    )
    await waitFor(() =>
      expect(
        (within(row).getByRole('button', { name: '全部授权' }) as HTMLButtonElement).disabled
      ).toBe(false)
    )
    fireEvent.click(within(row).getByRole('button', { name: '全部授权' }))
    await waitFor(() => expect(api.grantAll).toHaveBeenCalledWith('phone'))
    await waitFor(() =>
      expect((within(row).getByLabelText('手机 名称') as HTMLInputElement).disabled).toBe(false)
    )
    fireEvent.change(within(row).getByLabelText('手机 名称'), { target: { value: '我的手机' } })
    fireEvent.click(within(row).getByRole('button', { name: '保存' }))
    await waitFor(() => expect(api.rename).toHaveBeenCalledWith('phone', '我的手机'))
    expect(REMOTE_DEVICE_SCOPES).not.toContain('manage')
  })
  it('keeps disconnect socket separate from credential revocation', async () => {
    render(<DeviceManagementPanel />)
    const row = await screen.findByRole('listitem', { name: '手机' })
    expect(within(row).getByRole('button', { name: '断开连接' }).getAttribute('title')).toContain(
      '可重新连接'
    )
    fireEvent.click(within(row).getByRole('button', { name: '断开连接' }))
    await waitFor(() => expect(api.disconnect).toHaveBeenCalledWith('phone'))
    expect(api.revoke).not.toHaveBeenCalled()
    await waitFor(() =>
      expect(
        (within(row).getByRole('button', { name: '撤销配对' }) as HTMLButtonElement).disabled
      ).toBe(false)
    )
    fireEvent.click(within(row).getByRole('button', { name: '撤销配对' }))
    await waitFor(() => expect(api.revoke).toHaveBeenCalledWith('phone'))
  })
  it('requires confirmation before force-revoking every device', async () => {
    render(<DeviceManagementPanel />)
    await screen.findByText('手机')
    fireEvent.click(screen.getByRole('button', { name: '强制断开全部' }))
    expect(api.disconnectAll).not.toHaveBeenCalled()
    expect(
      await screen.findByText('撤销全部设备及旧版主机凭据，所有设备必须重新配对。')
    ).toBeDefined()
    fireEvent.click(screen.getByRole('button', { name: '取消' }))
    expect(api.disconnectAll).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: '强制断开全部' }))
    fireEvent.click(await screen.findByRole('button', { name: '撤销全部并断开' }))
    await waitFor(() => expect(api.disconnectAll).toHaveBeenCalledTimes(1))
  })
  it('renders safe errors, audit and UNIX-second times', async () => {
    render(<DeviceManagementPanel />)
    await screen.findByText('手机')
    fireEvent.click(screen.getByRole('button', { name: '操作记录' }))
    const audit = await screen.findByRole('list', { name: '操作记录' })
    expect(audit.textContent).toContain(new Date(1794210020 * 1000).toLocaleString())
    expect(audit.textContent).toContain('手机')
    api.revoke.mockRejectedValue(new Error('SECRET'))
    fireEvent.click(screen.getByRole('button', { name: '撤销配对' }))
    expect(await screen.findByRole('alert')).toHaveProperty('textContent', '设备操作失败，请重试')
    expect(document.body.textContent).not.toContain('SECRET')
  })
})
