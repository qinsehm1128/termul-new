import { beforeEach, describe, expect, it, vi } from 'vitest'

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

import { parseQuickTerminalRecord } from '@shared/types/quick-terminal.types'
import { createTauriQuickTerminalApi } from './tauri-quick-terminal-api'

const record = {
  schemaVersion: 1,
  id: '11111111-1111-4111-8111-111111111111',
  target: { kind: 'project_root', projectId: 'p1', projectRoot: '/work/p1' },
  cwd: '/work/p1',
  createdAtUtc: '2026-09-29T00:00:00.000Z',
  updatedAtUtc: '2026-09-29T00:00:00.000Z',
  origin: 'created'
}

beforeEach(() => invokeMock.mockReset())

describe('tauri quick terminal api', () => {
  it('sends each call to its desktop command with a payload', async () => {
    const api = createTauriQuickTerminalApi()
    invokeMock.mockResolvedValue({ success: true, data: [record] })
    expect((await api.list()).success).toBe(true)
    expect(invokeMock).toHaveBeenLastCalledWith('quick_terminal_list', undefined)

    invokeMock.mockResolvedValue({ success: true, data: record })
    await api.create({ target: { kind: 'workspace' } })
    expect(invokeMock).toHaveBeenLastCalledWith('quick_terminal_create', {
      payload: { target: { kind: 'workspace' }, title: null }
    })

    invokeMock.mockResolvedValue({
      success: true,
      data: { record, terminalId: 'pty-1', claim: 'claim-1', spawned: true }
    })
    const opened = await api.open(record.id, 80, 24)
    expect(invokeMock).toHaveBeenLastCalledWith('quick_terminal_open', {
      payload: { id: record.id, cols: 80, rows: 24 }
    })
    expect(opened.success && opened.data.claim).toBe('claim-1')

    invokeMock.mockResolvedValue({ success: true, data: record })
    await api.rename(record.id, 'build')
    expect(invokeMock).toHaveBeenLastCalledWith('quick_terminal_rename', {
      payload: { id: record.id, title: 'build' }
    })

    invokeMock.mockResolvedValue({ success: true })
    await api.remove(record.id)
    expect(invokeMock).toHaveBeenLastCalledWith('quick_terminal_delete', {
      payload: { id: record.id }
    })

    invokeMock.mockResolvedValue({ success: true, data: record })
    const closed = await api.close(record.id)
    expect(invokeMock).toHaveBeenLastCalledWith('quick_terminal_close', {
      payload: { id: record.id }
    })
    expect(closed.success && closed.data.id).toBe(record.id)
  })

  it('passes host error codes through unchanged', async () => {
    invokeMock.mockResolvedValue({
      success: false,
      error: 'quick terminal not found',
      code: 'QUICK_TERMINAL_NOT_FOUND'
    })
    const result = await createTauriQuickTerminalApi().open(record.id, 80, 24)
    expect(result).toEqual({
      success: false,
      error: 'quick terminal not found',
      code: 'QUICK_TERMINAL_NOT_FOUND'
    })
  })

  it('rejects malformed records instead of trusting them', () => {
    expect(parseQuickTerminalRecord(record).target).toEqual(record.target)
    expect(() => parseQuickTerminalRecord({ ...record, target: { kind: 'elsewhere' } })).toThrow(
      TypeError
    )
    expect(() => parseQuickTerminalRecord({ ...record, schemaVersion: 2 })).toThrow(TypeError)
  })
})
