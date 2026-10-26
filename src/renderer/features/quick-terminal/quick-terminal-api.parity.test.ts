import {
  parseQuickTerminalOpened,
  parseQuickTerminalRecord,
  type QuickTerminalApi,
  quickTerminalIds
} from '@shared/types/quick-terminal.types'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

import { createTauriQuickTerminalApi } from './tauri-quick-terminal-api'
import { createWebQuickTerminalApi } from './web-quick-terminal-api'

const legacy = {
  schemaVersion: 1 as const,
  id: '11111111-1111-4111-8111-111111111111',
  target: { kind: 'workspace' as const },
  cwd: '/work/quick',
  createdAtUtc: '2026-09-29T00:00:00.000Z',
  updatedAtUtc: '2026-09-29T00:00:00.000Z',
  terminalId: 'pty-1',
  origin: 'created' as const
}
const record = { ...legacy, schemaVersion: 2 as const, terminalIds: ['pty-1', 'pty-2'] }
const opened = { record, terminalId: 'pty-2', claim: 'test-output-claim', spawned: true }
const fetchMock = vi.fn()

beforeEach(() => {
  vi.clearAllMocks()
  vi.stubGlobal('fetch', fetchMock)
})
afterEach(() => vi.unstubAllGlobals())

describe('quick terminal v2 contracts', () => {
  it('upgrades legacy records without losing their existing shell', () => {
    expect(parseQuickTerminalRecord(legacy)).toEqual({
      ...legacy,
      schemaVersion: 2,
      terminalIds: ['pty-1']
    })
    expect(quickTerminalIds(legacy)).toEqual(['pty-1'])
    expect(parseQuickTerminalRecord({ ...legacy, terminalId: undefined }).terminalIds).toEqual([])
  })

  it('preserves the ordered collection and normalizes its primary alias', () => {
    expect(parseQuickTerminalRecord(record)).toEqual(record)
    expect(parseQuickTerminalRecord({ ...record, terminalId: undefined }).terminalId).toBe('pty-1')
    expect(quickTerminalIds(record)).toEqual(['pty-1', 'pty-2'])
    expect(
      parseQuickTerminalRecord({ ...record, terminalIds: [], terminalId: undefined }).terminalIds
    ).toEqual([])
  })

  it.each([
    { terminalIds: undefined },
    { terminalIds: ['pty-1', 'pty-1'] },
    { terminalIds: ['pty-1', ''] },
    { terminalIds: ['pty-1', 2] },
    { terminalIds: [] },
    { terminalId: 'foreign-pty' },
    { terminalId: '' },
    { schemaVersion: 3 }
  ])('rejects an invalid v2 record: %j', (invalid) => {
    expect(() => parseQuickTerminalRecord({ ...record, ...invalid })).toThrow(TypeError)
  })

  it('only accepts an opened v2 terminal that belongs to its record', () => {
    expect(parseQuickTerminalOpened(opened)).toEqual(opened)
    expect(() => parseQuickTerminalOpened({ ...opened, terminalId: 'foreign-pty' })).toThrow(
      TypeError
    )
  })
})

describe('quick terminal desktop/web transport parity', () => {
  const cases = [
    {
      name: 'create another terminal',
      command: 'quick_terminal_create_terminal',
      path: '/quick-terminals/create-terminal',
      payload: { id: record.id, cols: 100, rows: 30 },
      data: opened,
      call: (api: QuickTerminalApi) => api.createTerminal(record.id, 100, 30)
    },
    {
      name: 'open a specific existing terminal',
      command: 'quick_terminal_open',
      path: '/quick-terminals/open',
      payload: { id: record.id, cols: 100, rows: 30, terminalId: 'pty-2' },
      data: { ...opened, claim: undefined, spawned: false },
      call: (api: QuickTerminalApi) => api.open(record.id, 100, 30, 'pty-2')
    },
    {
      name: 'close only one terminal',
      command: 'quick_terminal_close_terminal',
      path: '/quick-terminals/close-terminal',
      payload: { id: record.id, terminalId: 'pty-2' },
      data: { ...record, terminalIds: ['pty-1'] },
      call: (api: QuickTerminalApi) => api.closeTerminal(record.id, 'pty-2')
    },
    {
      name: 'preserve the legacy open route and payload',
      command: 'quick_terminal_open',
      path: '/quick-terminals/open',
      payload: { id: record.id, cols: 100, rows: 30 },
      data: opened,
      call: (api: QuickTerminalApi) => api.open(record.id, 100, 30)
    },
    {
      name: 'close all terminals while retaining the entry',
      command: 'quick_terminal_close',
      path: '/quick-terminals/close',
      payload: { id: record.id },
      data: { ...record, terminalId: undefined, terminalIds: [] },
      call: (api: QuickTerminalApi) => api.close(record.id)
    }
  ]

  it.each(cases)('$name', async ({ command, path, payload, data, call }) => {
    invokeMock.mockResolvedValue({ success: true, data })
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ success: true, data }), { status: 200 })
    )
    const desktop = await call(createTauriQuickTerminalApi())
    const web = await call(createWebQuickTerminalApi())
    expect(desktop.success).toBe(true)
    expect(web).toEqual(desktop)
    expect(invokeMock).toHaveBeenLastCalledWith(command, { payload })
    const [url, init] = fetchMock.mock.calls.at(-1) as [string, RequestInit]
    expect(new URL(url).pathname).toBe(path)
    expect(init.method).toBe('POST')
    expect(new Headers(init.headers).get('content-type')).toBe('application/json')
    expect(JSON.parse(String(init.body))).toEqual(payload)
  })

  it('preserves ownership rejection in both transports', async () => {
    const failure = {
      success: false,
      error: 'terminal does not belong to quick terminal',
      code: 'QUICK_TERMINAL_INVALID_TARGET'
    }
    invokeMock.mockResolvedValue(failure)
    fetchMock.mockResolvedValue(new Response(JSON.stringify(failure), { status: 400 }))
    expect(await createTauriQuickTerminalApi().closeTerminal(record.id, 'foreign')).toEqual(failure)
    expect(await createWebQuickTerminalApi().closeTerminal(record.id, 'foreign')).toEqual(failure)
  })

  it('rejects a malformed collection from either transport', async () => {
    const data = { ...record, terminalIds: ['pty-1', 'pty-1'] }
    invokeMock.mockResolvedValue({ success: true, data })
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ success: true, data }), { status: 200 })
    )
    expect((await createTauriQuickTerminalApi().closeTerminal(record.id, 'pty-2')).success).toBe(
      false
    )
    expect((await createWebQuickTerminalApi().closeTerminal(record.id, 'pty-2')).success).toBe(
      false
    )
  })
})
