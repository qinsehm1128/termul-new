import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const api = vi.hoisted(() => ({
  list: vi.fn(),
  create: vi.fn(),
  open: vi.fn(),
  rename: vi.fn(),
  remove: vi.fn(),
  close: vi.fn(),
  onChanged: vi.fn((_handler: () => void) => () => undefined)
}))
vi.mock('./quick-terminal-api', () => ({ quickTerminalApi: api }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn(() => Promise.resolve()) }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => true }))

const terminalProps = vi.hoisted(() => ({ current: [] as Array<Record<string, unknown>> }))
/** The grid the mocked terminal "fits" to before a shell is attached. */
const FITTED = { cols: 132, rows: 41 }
vi.mock('@/components/terminal/ConnectedTerminal', async () => {
  const { useEffect } = await vi.importActual<typeof import('react')>('react')
  return {
    ConnectedTerminal: (props: Record<string, unknown>) => {
      terminalProps.current.push(props)
      const onInitialGrid = props.onInitialGrid as
        | ((cols: number, rows: number) => void)
        | undefined
      useEffect(() => {
        if (!props.terminalId) onInitialGrid?.(FITTED.cols, FITTED.rows)
      }, [])
      return <div data-testid="connected-terminal" data-pty={String(props.terminalId)} />
    }
  }
})

import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import QuickTerminalsPage from './QuickTerminalsPage'
import { resetQuickTerminalStore } from './quick-terminal-store'

const id = '11111111-1111-4111-8111-111111111111'
const record = {
  schemaVersion: 1 as const,
  id,
  target: { kind: 'workspace' as const },
  cwd: '/docs/Se/terminals/2026/09/29/scratch',
  createdAtUtc: '2026-09-29T00:00:00.000Z',
  updatedAtUtc: '2026-09-29T00:00:00.000Z',
  origin: 'created' as const
}

function Location(): React.JSX.Element {
  return <div data-testid="location">{useLocation().pathname}</div>
}

function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/quick-terminals" element={<QuickTerminalsPage />} />
        <Route path="/quick-terminals/:quickTerminalId" element={<QuickTerminalsPage />} />
      </Routes>
      <Location />
    </MemoryRouter>
  )
}

function openMenu(name: RegExp): void {
  const trigger = screen.getByRole('button', { name })
  // Radix opens on pointerdown, not click.
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false, pointerType: 'mouse' })
  fireEvent.click(trigger)
}

beforeEach(() => {
  vi.clearAllMocks()
  terminalProps.current = []
  resetQuickTerminalStore()
  useTerminalStore.setState({ terminals: [], activeTerminalId: '', ptyIdIndex: new Map() })
  useProjectStore.setState({
    projects: [
      {
        id: 'p1',
        name: 'Payments',
        color: 'blue',
        path: '/work/payments',
        worktrees: [{ id: 'w1', name: 'w1', branch: 'feat/x', path: '/work/wt', createdAt: '' }],
        activeWorktreeId: 'w1'
      }
    ]
  } as never)
  api.list.mockResolvedValue({ success: true, data: [record] })
})

afterEach(() => cleanup())

describe('QuickTerminalsPage', () => {
  it('shows the selected quick terminal and reopens it after the shell exits', async () => {
    api.open
      .mockResolvedValueOnce({
        success: true,
        data: {
          record: { ...record, terminalId: 'pty-1' },
          terminalId: 'pty-1',
          claim: 'c1',
          spawned: true
        }
      })
      .mockResolvedValueOnce({
        success: true,
        data: {
          record: { ...record, terminalId: 'pty-2' },
          terminalId: 'pty-2',
          claim: 'c2',
          spawned: true
        }
      })
    renderAt(`/quick-terminals/${id}`)

    await waitFor(() => expect(screen.getByTestId('connected-terminal').dataset.pty).toBe('pty-1'))
    // The shell starts at the grid it is shown at, not at a placeholder size.
    expect(api.open).toHaveBeenNthCalledWith(1, id, FITTED.cols, FITTED.rows)
    const props = terminalProps.current.at(-1) ?? {}
    expect(props).toMatchObject({ terminalId: 'pty-1', storeTerminalId: 'pty-1', autoSpawn: false })

    act(() => {
      ;(props.onExit as (code: number) => void)(0)
    })
    fireEvent.click(await screen.findByRole('button', { name: 'Reopen' }))

    await waitFor(() => expect(screen.getByTestId('connected-terminal').dataset.pty).toBe('pty-2'))
    expect(api.open).toHaveBeenCalledTimes(2)
    expect(api.open).toHaveBeenNthCalledWith(2, id, FITTED.cols, FITTED.rows)
  })

  it('closes the shell from the toolbar and keeps the quick terminal to reopen', async () => {
    api.open
      .mockResolvedValueOnce({
        success: true,
        data: { record: { ...record, terminalId: 'pty-1' }, terminalId: 'pty-1', spawned: true }
      })
      .mockResolvedValueOnce({
        success: true,
        data: { record: { ...record, terminalId: 'pty-2' }, terminalId: 'pty-2', spawned: true }
      })
    api.close.mockResolvedValue({ success: true, data: record })
    renderAt(`/quick-terminals/${id}`)
    await waitFor(() => expect(screen.getByTestId('connected-terminal').dataset.pty).toBe('pty-1'))

    fireEvent.click(screen.getByRole('button', { name: 'Close terminal' }))

    await screen.findByText(/The terminal is closed/)
    expect(api.close).toHaveBeenCalledWith(id)
    expect(screen.queryByTestId('connected-terminal')).toBeNull()
    expect(screen.getByText('Untitled terminal')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Reopen' }))
    await waitFor(() => expect(screen.getByTestId('connected-terminal').dataset.pty).toBe('pty-2'))
  })

  it('closes the shell from the list menu', async () => {
    api.open.mockResolvedValue({
      success: true,
      data: { record: { ...record, terminalId: 'pty-1' }, terminalId: 'pty-1', spawned: true }
    })
    api.close.mockResolvedValue({ success: true, data: record })
    renderAt(`/quick-terminals/${id}`)
    await waitFor(() => expect(screen.getByTestId('connected-terminal').dataset.pty).toBe('pty-1'))

    openMenu(/more actions/i)
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Close terminal' }))

    await screen.findByText(/The terminal is closed/)
    expect(api.close).toHaveBeenCalledWith(id)
  })

  it('lists again when the host reports new quick terminals', async () => {
    renderAt('/quick-terminals')
    await screen.findByText('Untitled terminal')
    const handler = api.onChanged.mock.calls[0]?.[0]
    api.list.mockResolvedValue({
      success: true,
      data: [record, { ...record, id: '33333333-3333-4333-8333-333333333333', title: 'migrated' }]
    })

    act(() => handler?.())

    expect(await screen.findByText('migrated')).toBeTruthy()
  })

  it('creates a quick terminal in its own folder and navigates to it', async () => {
    const created = { ...record, id: '22222222-2222-4222-8222-222222222222' }
    api.create.mockResolvedValue({ success: true, data: created })
    api.open.mockResolvedValue({
      success: true,
      data: {
        record: { ...created, terminalId: 'pty-9' },
        terminalId: 'pty-9',
        spawned: true,
        claim: 'c9'
      }
    })
    renderAt('/quick-terminals')
    await screen.findByText('Untitled terminal')

    openMenu(/new quick terminal/i)
    fireEvent.click(await screen.findByRole('menuitem', { name: /in a new folder/i }))

    await waitFor(() =>
      expect(screen.getByTestId('location').textContent).toBe(`/quick-terminals/${created.id}`)
    )
    expect(api.create).toHaveBeenCalledWith({ target: { kind: 'workspace' } })
  })

  it('opens a project quick terminal in its active worktree', async () => {
    api.create.mockResolvedValue({ success: false, error: 'nope', code: 'X' })
    renderAt('/quick-terminals')
    await screen.findByText('Untitled terminal')

    openMenu(/new quick terminal/i)
    fireEvent.click(await screen.findByRole('menuitem', { name: /payments/i }))

    await waitFor(() => expect(api.create).toHaveBeenCalled())
    expect(api.create).toHaveBeenCalledWith({
      target: {
        kind: 'worktree',
        projectId: 'p1',
        worktreePath: '/work/wt',
        worktreeBranch: 'feat/x'
      }
    })
  })
})
