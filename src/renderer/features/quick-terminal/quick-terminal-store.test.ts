import { beforeEach, describe, expect, it, vi } from 'vitest'

const api = vi.hoisted(() => ({
  list: vi.fn(),
  create: vi.fn(),
  open: vi.fn(),
  createTerminal: vi.fn(),
  closeTerminal: vi.fn(),
  rename: vi.fn(),
  remove: vi.fn(),
  close: vi.fn(),
  onChanged: vi.fn((_handler: () => void) => () => undefined)
}))
const terminalApiMocks = vi.hoisted(() => ({ attach: vi.fn() }))
vi.mock('./quick-terminal-api', () => ({ quickTerminalApi: api }))
vi.mock('@/lib/terminal-api', () => ({ terminalApi: { attach: terminalApiMocks.attach } }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn(() => Promise.resolve()) }))
vi.mock('@/lib/persistence-api', () => ({
  persistenceApi: {
    read: vi.fn(async () => ({ success: true, data: null })),
    writeDebounced: vi.fn(async () => ({ success: true })),
    flushPendingWrites: vi.fn(async () => ({ success: true })),
    delete: vi.fn(async () => ({ success: true }))
  }
}))

import { logFrontendError } from '@/lib/log-api'
import { useTerminalStore } from '@/stores/terminal-store'
import {
  quickTerminalName,
  resetQuickTerminalStore,
  useQuickTerminalStore
} from './quick-terminal-store'

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
  vi.resetAllMocks()
  terminalApiMocks.attach.mockResolvedValue({ success: true, data: {} })
  resetQuickTerminalStore()
  useTerminalStore.setState({ terminals: [], activeTerminalId: '', ptyIdIndex: new Map() })
})

describe('quick terminal store', () => {
  it('attaches a spawned shell with its claim and keeps the credential in memory', async () => {
    api.open.mockResolvedValue(opened('pty-1', true))

    await useQuickTerminalStore.getState().open(id, 80, 24)

    // The host spawned the shell but had no renderer channel: attach installs
    // the live stream with the spawn-issued claim before the record is adopted.
    expect(terminalApiMocks.attach).toHaveBeenCalledWith('pty-1', 'claim-pty-1', 0)
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

  it('falls back to the claim-less watch path when attach is refused', async () => {
    api.open.mockResolvedValue(opened('pty-1', true))
    terminalApiMocks.attach.mockResolvedValue({
      success: false,
      error: 'Unauthorized',
      code: 'UNAUTHORIZED'
    })

    await useQuickTerminalStore.getState().open(id, 80, 24)

    const [terminal] = useTerminalStore.getState().terminals
    expect(terminal.claim).toBeUndefined()
    expect(logFrontendError).toHaveBeenCalledWith(
      expect.objectContaining({ message: 'operation=attach code=UNAUTHORIZED' })
    )
  })

  it('does not attach or claim a reused shell', async () => {
    api.open.mockResolvedValue(opened('pty-1', false))

    await useQuickTerminalStore.getState().open(id, 80, 24)

    expect(terminalApiMocks.attach).not.toHaveBeenCalled()
    expect(useTerminalStore.getState().terminals[0].claim).toBeUndefined()
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

  it('closing drops the shell but keeps the quick terminal', async () => {
    api.open.mockResolvedValue(opened('pty-1', true))
    api.list.mockResolvedValue({ success: true, data: [record] })
    await useQuickTerminalStore.getState().load()
    await useQuickTerminalStore.getState().open(id, 80, 24)
    const closed = { ...record, updatedAtUtc: '2026-09-29T01:00:00.000Z' }
    api.close.mockResolvedValue({ success: true, data: closed })

    await useQuickTerminalStore.getState().close(id)

    expect(useQuickTerminalStore.getState().records).toEqual([closed])
    expect(useTerminalStore.getState().terminals).toEqual([])
  })

  it('keeps the shell when the host refuses to close it', async () => {
    api.open.mockResolvedValue(opened('pty-1', true))
    api.list.mockResolvedValue({ success: true, data: [record] })
    await useQuickTerminalStore.getState().load()
    await useQuickTerminalStore.getState().open(id, 80, 24)
    api.close.mockResolvedValue({
      success: false,
      error: 'busy',
      code: 'QUICK_TERMINAL_TERMINATE_FAILED'
    })

    const result = await useQuickTerminalStore.getState().close(id)

    expect(result.success).toBe(false)
    expect(useTerminalStore.getState().terminals).toHaveLength(1)
  })

  it('names an untitled private folder instead of showing its id', () => {
    const untitled = { ...record, title: undefined, target: { kind: 'workspace' as const } }
    expect(quickTerminalName(untitled)).toBe('Untitled terminal')
    expect(quickTerminalName({ ...record, title: undefined })).toBe('p1')
    expect(quickTerminalName(record)).toBe('build')
  })
  it('registers new shells, attaches spawned claims, preserves siblings, closes one versus all', async () => {
    const host = {
      ...record,
      schemaVersion: 2 as const,
      terminalIds: ['pty-1', 'pty-2'],
      terminalId: 'pty-1'
    }
    api.open.mockResolvedValue({
      success: true,
      data: { record: host, terminalId: 'pty-1', spawned: false }
    })
    await useQuickTerminalStore.getState().open(id, 90, 30, 'pty-1')
    api.createTerminal.mockResolvedValue({
      success: true,
      data: { record: host, terminalId: 'pty-2', spawned: true, claim: 'OUTPUT-CLAIM' }
    })
    await useQuickTerminalStore.getState().createTerminal(id, 90, 30)
    expect(useTerminalStore.getState().terminals.map((item) => item.ptyId)).toEqual([
      'pty-1',
      'pty-2'
    ])
    expect(terminalApiMocks.attach).toHaveBeenCalledWith('pty-2', 'OUTPUT-CLAIM', 0)
    expect(useTerminalStore.getState().findTerminalByPtyId('pty-1')?.claim).toBeUndefined()
    expect(useTerminalStore.getState().findTerminalByPtyId('pty-2')?.claim).toBe('OUTPUT-CLAIM')
    expect(useTerminalStore.getState().terminals.every((item) => !item.projectId)).toBe(true)
    expect(api.open).toHaveBeenCalledWith(id, 90, 30, 'pty-1')
    api.closeTerminal.mockResolvedValue({
      success: true,
      data: { ...host, terminalIds: ['pty-2'], terminalId: 'pty-2' }
    })
    await useQuickTerminalStore.getState().closeTerminal(id, 'pty-1')
    expect(useTerminalStore.getState().terminals.map((item) => item.ptyId)).toEqual(['pty-2'])
    expect(useTerminalStore.getState().findTerminalByPtyId('pty-1')).toBeUndefined()
    expect(useTerminalStore.getState().findTerminalByPtyId('pty-2')).toBeDefined()
    expect(api.close).not.toHaveBeenCalled()
    api.close.mockResolvedValue({
      success: true,
      data: { ...host, terminalIds: [], terminalId: undefined }
    })
    await useQuickTerminalStore.getState().close(id)
    expect(useTerminalStore.getState().terminals).toEqual([])
  })

  it('orders requests for one entry but lets other entries proceed independently', async () => {
    let resolveOpen!: (value: unknown) => void
    api.open.mockReturnValue(
      new Promise((resolve) => {
        resolveOpen = resolve
      })
    )
    api.createTerminal.mockResolvedValue({
      success: true,
      data: { record: { ...record, terminalIds: ['one', 'two'] }, terminalId: 'two', spawned: true }
    })
    const opening = useQuickTerminalStore.getState().open(id, 80, 24)
    const creating = useQuickTerminalStore.getState().createTerminal(id, 80, 24)
    await vi.waitFor(() => expect(api.open).toHaveBeenCalledOnce())
    expect(api.createTerminal).not.toHaveBeenCalled()
    resolveOpen({
      success: true,
      data: { record: { ...record, terminalIds: ['one'] }, terminalId: 'one', spawned: true }
    })
    await Promise.all([opening, creating])
    expect(useQuickTerminalStore.getState().records[0].terminalIds).toEqual(['one', 'two'])
    expect(useTerminalStore.getState().terminals.map((item) => item.ptyId)).toEqual(['one', 'two'])
  })

  it('ignores a stale list response that predates a newer terminal create', async () => {
    let resolveList!: (value: unknown) => void
    api.list.mockReturnValue(
      new Promise((resolve) => {
        resolveList = resolve
      })
    )
    const listing = useQuickTerminalStore.getState().load()
    api.createTerminal.mockResolvedValue({
      success: true,
      data: { record: { ...record, terminalIds: ['new'] }, terminalId: 'new', spawned: true }
    })
    await useQuickTerminalStore.getState().createTerminal(id, 80, 24)
    resolveList({ success: true, data: [record] })
    await listing
    expect(useQuickTerminalStore.getState().records[0].terminalIds).toEqual(['new'])
  })
})
