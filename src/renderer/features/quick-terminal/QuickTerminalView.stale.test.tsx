import type { QuickTerminalRecord } from '@shared/types/quick-terminal.types'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const api = vi.hoisted(() => ({
  open: vi.fn(),
  createTerminal: vi.fn()
}))
const persistence = vi.hoisted(() => ({
  writeDebounced: vi.fn(async () => ({ success: true }))
}))
vi.mock('./quick-terminal-api', () => ({ quickTerminalApi: api }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn(async () => undefined) }))
vi.mock('@/lib/persistence-api', () => ({ persistenceApi: persistence }))
vi.mock('./QuickTerminalFilePreview', () => ({ QuickTerminalFilePreview: () => null }))
vi.mock('@/components/terminal/ConnectedTerminal', () => ({
  ConnectedTerminal: (props: {
    terminalId?: string
    isVisible?: boolean
    onInitialGrid?: (cols: number, rows: number) => void
    onExit?: (code: number) => void
  }) => (
    <div data-testid="terminal" data-pty={props.terminalId} data-visible={props.isVisible}>
      {!props.terminalId && (
        <button type="button" onClick={() => props.onInitialGrid?.(137, 43)}>
          Measure
        </button>
      )}
      {props.terminalId && (
        <button type="button" onClick={() => props.onExit?.(0)}>
          Exit {props.terminalId}
        </button>
      )}
    </div>
  )
}))

import { useTerminalStore } from '@/stores/terminal-store'
import { QuickTerminalWorkspace } from './QuickTerminalWorkspace'
import { resetQuickTerminalStore, useQuickTerminalStore } from './quick-terminal-store'
import { resetQuickWorkspaceStore, useQuickWorkspaceStore } from './quick-terminal-workspace-store'

const id = 'stale-reopen-entry'
const record: QuickTerminalRecord = {
  schemaVersion: 2,
  id,
  target: { kind: 'workspace' },
  cwd: '/scratch',
  createdAtUtc: '2026-10-26T00:00:00Z',
  updatedAtUtc: '2026-10-26T00:00:00Z',
  origin: 'created',
  terminalId: 'dead',
  terminalIds: ['dead', 'sibling']
}
const replacement = { ...record, terminalId: 'sibling', terminalIds: ['sibling', 'replacement'] }

function seed(): void {
  useQuickTerminalStore.setState({ records: [record] })
  useQuickWorkspaceStore.setState({
    layouts: {
      [id]: {
        tabs: [
          { id: 'dead-tab', type: 'terminal', terminalId: 'dead' },
          { id: 'sibling-tab', type: 'terminal', terminalId: 'sibling' }
        ],
        activeTabId: 'dead-tab',
        lastTerminalTabId: 'dead-tab'
      }
    }
  })
  for (const ptyId of ['dead', 'sibling']) {
    useTerminalStore
      .getState()
      .adoptQuickTerminal({ quickTerminalId: id, ptyId, name: ptyId, cwd: record.cwd })
  }
  useTerminalStore.getState().adoptQuickTerminal({
    quickTerminalId: 'other-entry',
    ptyId: 'unrelated',
    name: 'other',
    cwd: '/other'
  })
}

function measure(): void {
  const visible = screen.getAllByTestId('terminal').find((node) => node.dataset.visible === 'true')
  const button = visible?.querySelector('button')
  expect(button?.textContent).toBe('Measure')
  fireEvent.click(button as HTMLButtonElement)
}
function expectReplacement(): void {
  expect(useQuickTerminalStore.getState().records[0]).toEqual(replacement)
  expect(useQuickWorkspaceStore.getState().layouts[id]).toEqual({
    tabs: [
      { id: 'dead-tab', type: 'terminal', terminalId: 'replacement' },
      { id: 'sibling-tab', type: 'terminal', terminalId: 'sibling' }
    ],
    activeTabId: 'dead-tab',
    lastTerminalTabId: 'dead-tab'
  })
  expect(
    useTerminalStore
      .getState()
      .terminals.map((item) => item.ptyId)
      .sort()
  ).toEqual(['replacement', 'sibling', 'unrelated'])
  expect(useTerminalStore.getState().findTerminalByPtyId('dead')).toBeUndefined()
  expect(useTerminalStore.getState().findTerminalByPtyId('sibling')).toBeDefined()
  expect(persistence.writeDebounced).toHaveBeenLastCalledWith(
    `quick-terminal/workspaces/${id}`,
    useQuickWorkspaceStore.getState().layouts[id]
  )
}

beforeEach(() => {
  vi.clearAllMocks()
  api.open.mockReset()
  api.createTerminal.mockReset()
  resetQuickTerminalStore()
  resetQuickWorkspaceStore()
  useTerminalStore.setState({ terminals: [], activeTerminalId: '', ptyIdIndex: new Map() })
  seed()
})
afterEach(cleanup)

describe('QuickTerminalView stale and exited targets', () => {
  it('does not spawn on stale restoration; explicit reopen and failed-create retry create a separate measured replacement', async () => {
    api.open.mockResolvedValue({
      success: false,
      code: 'QUICK_TERMINAL_INVALID_TARGET',
      error: 'Target has exited'
    })
    api.createTerminal
      .mockResolvedValueOnce({
        success: false,
        code: 'QUICK_TERMINAL_SPAWN_FAILED',
        error: 'Spawn failed'
      })
      .mockResolvedValueOnce({
        success: true,
        data: { record: replacement, terminalId: 'replacement', spawned: true }
      })
    const mounted = render(<QuickTerminalWorkspace record={record} />)
    await screen.findAllByTestId('terminal')
    expect(api.open).not.toHaveBeenCalled()
    measure()
    expect(await screen.findByRole('alert')).toHaveTextContent('Target has exited')
    expect(api.open).toHaveBeenCalledExactlyOnceWith(id, 137, 43, 'dead')
    expect(api.createTerminal).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole('button', { name: 'Reopen' }))
    expect(api.createTerminal).not.toHaveBeenCalled()
    measure()
    expect(await screen.findByRole('alert')).toHaveTextContent('Spawn failed')
    // A failed replacement must not revert the next retry to the stale ID.
    fireEvent.click(screen.getByRole('button', { name: 'Reopen' }))
    measure()
    await screen.findByRole('button', { name: 'Exit replacement' })
    expect(api.createTerminal).toHaveBeenCalledTimes(2)
    expect(api.createTerminal).toHaveBeenNthCalledWith(1, id, 137, 43)
    expect(api.createTerminal).toHaveBeenNthCalledWith(2, id, 137, 43)
    expect(api.open).toHaveBeenCalledTimes(1)
    expectReplacement()

    // A remount restores the replacement ID rather than creating another shell.
    mounted.unmount()
    api.open.mockResolvedValue({
      success: true,
      data: { record: replacement, terminalId: 'replacement', spawned: false }
    })
    render(<QuickTerminalWorkspace record={replacement} />)
    await screen.findAllByTestId('terminal')
    measure()
    await screen.findByRole('button', { name: 'Exit replacement' })
    expect(api.open).toHaveBeenLastCalledWith(id, 137, 43, 'replacement')
    expect(api.createTerminal).toHaveBeenCalledTimes(2)
  })

  it('reopens an exited terminal with createTerminal, preserving the sibling and tab identity', async () => {
    api.open.mockResolvedValue({
      success: true,
      data: { record, terminalId: 'dead', spawned: false }
    })
    api.createTerminal.mockResolvedValue({
      success: true,
      data: { record: replacement, terminalId: 'replacement', spawned: true }
    })
    render(<QuickTerminalWorkspace record={record} />)
    await screen.findAllByTestId('terminal')
    measure()
    fireEvent.click(await screen.findByRole('button', { name: 'Exit dead' }))
    expect(api.createTerminal).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Reopen' }))
    expect(api.createTerminal).not.toHaveBeenCalled()
    measure()
    await screen.findByRole('button', { name: 'Exit replacement' })
    expect(api.createTerminal).toHaveBeenCalledExactlyOnceWith(id, 137, 43)
    expect(api.open).toHaveBeenCalledExactlyOnceWith(id, 137, 43, 'dead')
    expectReplacement()
  })

  it('retries a transient attach failure on the same explicit target, without creating a shell', async () => {
    api.open
      .mockResolvedValueOnce({ success: false, code: 'IPC_ERROR', error: 'Disconnected' })
      .mockResolvedValueOnce({
        success: true,
        data: { record, terminalId: 'dead', spawned: false }
      })
    render(<QuickTerminalWorkspace record={record} />)
    await screen.findAllByTestId('terminal')
    measure()
    await screen.findByRole('alert')
    fireEvent.click(screen.getByRole('button', { name: 'Reopen' }))
    measure()
    await screen.findByRole('button', { name: 'Exit dead' })
    expect(api.open).toHaveBeenNthCalledWith(2, id, 137, 43, 'dead')
    expect(api.createTerminal).not.toHaveBeenCalled()
  })

  it('joins an explicit replacement still pending across a remount without duplicating a shell', async () => {
    api.open.mockResolvedValue({
      success: false,
      code: 'QUICK_TERMINAL_INVALID_TARGET',
      error: 'Dead'
    })
    let resolveCreate!: (value: unknown) => void
    api.createTerminal.mockReturnValue(
      new Promise((resolve) => {
        resolveCreate = resolve
      })
    )
    const mounted = render(<QuickTerminalWorkspace record={record} />)
    await screen.findAllByTestId('terminal')
    measure()
    await screen.findByRole('alert')
    fireEvent.click(screen.getByRole('button', { name: 'Reopen' }))
    measure()
    await waitFor(() => expect(api.createTerminal).toHaveBeenCalledTimes(1))
    mounted.unmount()
    render(<QuickTerminalWorkspace record={record} />)
    await screen.findAllByTestId('terminal')
    measure()
    await act(async () =>
      resolveCreate({
        success: true,
        data: { record: replacement, terminalId: 'replacement', spawned: true }
      })
    )
    await screen.findByRole('button', { name: 'Exit replacement' })
    expect(api.open).toHaveBeenCalledTimes(1)
    expect(api.createTerminal).toHaveBeenCalledTimes(1)
    expectReplacement()
  })
})
