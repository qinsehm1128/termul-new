import type { QuickTerminalRecord } from '@shared/types/quick-terminal.types'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  reconcileQuickWorkspace,
  resetQuickWorkspaceStore,
  useQuickWorkspaceStore
} from './quick-terminal-workspace-store'

const persistence = vi.hoisted(() => ({
  read: vi.fn(),
  writeDebounced: vi.fn(),
  flushPendingWrites: vi.fn(),
  delete: vi.fn()
}))
const log = vi.hoisted(() => vi.fn(async () => ({ success: true })))
vi.mock('@/lib/persistence-api', () => ({ persistenceApi: persistence }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: log }))
const record: QuickTerminalRecord = {
  schemaVersion: 2,
  id: 'qt-a',
  target: { kind: 'workspace' },
  cwd: '/scratch',
  createdAtUtc: '2026-10-26',
  updatedAtUtc: '2026-10-26',
  terminalIds: ['one', 'two']
}
beforeEach(() => {
  vi.resetAllMocks()
  resetQuickWorkspaceStore()
  persistence.read.mockResolvedValue({ success: true, data: null })
  persistence.writeDebounced.mockResolvedValue({ success: true })
  persistence.flushPendingWrites.mockResolvedValue({ success: true })
  persistence.delete.mockResolvedValue({ success: true })
})
describe('quick workspace persistence', () => {
  it('restores order, file paths and selection while dropping dead PTYs and adding host shells', async () => {
    persistence.read.mockResolvedValue({
      success: true,
      data: {
        tabs: [
          { id: 'second', type: 'terminal', terminalId: 'two' },
          { id: 'file', type: 'editor', filePath: '/a.md' },
          { id: 'dead', type: 'terminal', terminalId: 'dead' }
        ],
        activeTabId: 'file',
        lastTerminalTabId: 'second',
        claim: 'DO-NOT-RESTORE'
      }
    })
    await useQuickWorkspaceStore.getState().load(record)
    const layout = useQuickWorkspaceStore.getState().layouts[record.id]
    expect(layout.tabs.map((tab) => tab.id)).toEqual(['second', 'file', 'terminal:one'])
    expect(layout.activeTabId).toBe('file')
    expect(layout.lastTerminalTabId).toBe('second')
    expect(JSON.stringify(layout)).not.toContain('claim')
    expect(persistence.read).toHaveBeenCalledWith('quick-terminal/workspaces/qt-a')
  })
  it('falls back to the last live terminal when the selected host PTY has disappeared', () => {
    const result = reconcileQuickWorkspace(
      {
        tabs: [
          { id: 'one', type: 'terminal', terminalId: 'one' },
          { id: 'dead', type: 'terminal', terminalId: 'dead' }
        ],
        activeTabId: 'dead',
        lastTerminalTabId: 'one'
      },
      record
    )
    expect(result.activeTabId).toBe('one')
    expect(result.tabs).toHaveLength(2)
  })
  it('does not recreate absent terminals from a saved empty or dead layout', async () => {
    persistence.read.mockResolvedValue({
      success: true,
      data: { tabs: [{ id: 'dead', type: 'terminal', terminalId: 'gone' }], activeTabId: 'dead' }
    })
    await useQuickWorkspaceStore.getState().load({ ...record, terminalIds: [] })
    expect(useQuickWorkspaceStore.getState().layouts[record.id].tabs).toEqual([])
  })
  it('persists only durable tab data and keeps separate entry layouts', async () => {
    await useQuickWorkspaceStore.getState().load(record)
    await useQuickWorkspaceStore.getState().load({ ...record, id: 'qt-b' })
    useQuickWorkspaceStore.getState().update(record.id, (layout) => ({
      ...layout,
      tabs: [
        ...layout.tabs,
        { id: 'pending', type: 'terminal', create: true },
        { id: 'file', type: 'editor', filePath: '/secret-name.md' }
      ],
      activeTabId: 'file'
    }))
    const [, saved] = persistence.writeDebounced.mock.calls.at(-1) ?? []
    expect(saved.tabs).toHaveLength(3)
    expect(JSON.stringify(saved)).not.toContain('create')
    expect(useQuickWorkspaceStore.getState().layouts['qt-b'].activeTabId).toBe('terminal:one')
  })
  it('does not let a late load resurrect a deleted entry and flushes writes before delete', async () => {
    let resolveRead!: (value: unknown) => void
    persistence.read.mockReturnValue(
      new Promise((resolve) => {
        resolveRead = resolve
      })
    )
    const loading = useQuickWorkspaceStore.getState().load(record)
    await useQuickWorkspaceStore.getState().remove(record.id)
    resolveRead({ success: true, data: null })
    await loading
    expect(useQuickWorkspaceStore.getState().layouts[record.id]).toBeUndefined()
    expect(persistence.flushPendingWrites.mock.invocationCallOrder[0]).toBeLessThan(
      persistence.delete.mock.invocationCallOrder[0]
    )
  })
  it('logs failed read and persistence without file paths, claims or raw error details', async () => {
    persistence.read.mockRejectedValue(new Error('SECRET'))
    persistence.writeDebounced.mockResolvedValue({ success: false, error: 'SECRET' })
    await useQuickWorkspaceStore.getState().load(record)
    useQuickWorkspaceStore.getState().update(record.id, (layout) => layout)
    await Promise.resolve()
    expect(log).toHaveBeenCalled()
    expect(JSON.stringify(log.mock.calls)).not.toContain('SECRET')
    expect(JSON.stringify(log.mock.calls)).not.toContain(record.cwd)
  })
})
