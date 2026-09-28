import { beforeEach, describe, expect, it, vi } from 'vitest'

const api = vi.hoisted(() => ({
  list: vi.fn(),
  create: vi.fn(),
  open: vi.fn(),
  rename: vi.fn(),
  remove: vi.fn(),
  onChanged: vi.fn((_handler: () => void) => () => undefined)
}))
vi.mock('./quick-terminal-api', () => ({ quickTerminalApi: api }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn(() => Promise.resolve()) }))

import { useTerminalStore } from '@/stores/terminal-store'
import { resetQuickTerminalStore, useQuickTerminalStore } from './quick-terminal-store'

const id = '11111111-1111-4111-8111-111111111111'
const record = {
  schemaVersion: 1 as const,
  id,
  title: 'build',
  target: { kind: 'project_root' as const, projectId: 'p1', projectRoot: '/work/p1' },
  cwd: '/work/p1',
  createdAtUtc: '2026-09-29T00:00:00.000Z',
  updatedAtUtc: '2026-09-29T00:00:00.000Z',
  origin: 'created' as const
}

function opened(terminalId: string, spawned: boolean) {
  return {
    success: true as const,
    data: {
      record,
      terminalId,
      spawned,
      ...(spawned ? { claim: `claim-${terminalId}` } : {})
    }
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  resetQuickTerminalStore()
  useTerminalStore.setState({ terminals: [], activeTerminalId: '', ptyIdIndex: new Map() })
})

describe('quick terminal store', () => {
  it('registers the shell without a project id so no project surface adopts it', async () => {
    api.open.mockResolvedValue(opened('pty-1', true))

    await useQuickTerminalStore.getState().open(id, 80, 24)

    const [terminal] = useTerminalStore.getState().terminals
    expect(terminal).toMatchObject({
      id: 'pty-1',
      ptyId: 'pty-1',
      quickTerminalId: id,
      name: 'build',
      claim: 'claim-pty-1'
    })
    expect(terminal.projectId).toBeUndefined()
    expect(terminal.conversationId).toBeUndefined()
  })

  it('replaces the record of an exited shell and keeps a reused one', async () => {
    api.open.mockResolvedValueOnce(opened('pty-1', true))
    await useQuickTerminalStore.getState().open(id, 80, 24)
    api.open.mockResolvedValueOnce(opened('pty-1', false))
    await useQuickTerminalStore.getState().open(id, 80, 24)
    expect(useTerminalStore.getState().terminals.map((t) => t.ptyId)).toEqual(['pty-1'])

    api.open.mockResolvedValueOnce(opened('pty-2', true))
    await useQuickTerminalStore.getState().open(id, 80, 24)
    expect(useTerminalStore.getState().terminals.map((t) => t.ptyId)).toEqual(['pty-2'])
    expect(useTerminalStore.getState().findTerminalByPtyId('pty-1')).toBeUndefined()
  })

  it('forgets the shell when the quick terminal is deleted', async () => {
    api.open.mockResolvedValue(opened('pty-1', true))
    api.list.mockResolvedValue({ success: true, data: [record] })
    await useQuickTerminalStore.getState().load()
    await useQuickTerminalStore.getState().open(id, 80, 24)
    api.remove.mockResolvedValue({ success: true, data: undefined })

    await useQuickTerminalStore.getState().remove(id)

    expect(useQuickTerminalStore.getState().records).toEqual([])
    expect(useTerminalStore.getState().terminals).toEqual([])
  })

  it('keeps everything when the host refuses the delete', async () => {
    api.list.mockResolvedValue({ success: true, data: [record] })
    await useQuickTerminalStore.getState().load()
    api.remove.mockResolvedValue({
      success: false,
      error: 'busy',
      code: 'QUICK_TERMINAL_TERMINATE_FAILED'
    })

    const result = await useQuickTerminalStore.getState().remove(id)

    expect(result.success).toBe(false)
    expect(useQuickTerminalStore.getState().records).toHaveLength(1)
  })
})
