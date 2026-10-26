import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

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
vi.mock('./quick-terminal-api', () => ({ quickTerminalApi: api }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn(() => Promise.resolve()) }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => true }))
const persistence = vi.hoisted(() => ({
  read: vi.fn(),
  writeDebounced: vi.fn(async () => ({ success: true })),
  flushPendingWrites: vi.fn(async () => ({ success: true })),
  delete: vi.fn(async () => ({ success: true }))
}))
vi.mock('@/lib/persistence-api', () => ({ persistenceApi: persistence }))

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
      return (
        <div
          data-testid="connected-terminal"
          data-pty={String(props.terminalId)}
          data-visible={String(props.isVisible)}
          data-auto-focus={String(props.autoFocus)}
        />
      )
    }
  }
})

vi.mock('@/components/editor/EditorPanel', () => ({
  EditorPanel: ({ filePath }: { filePath: string }) => (
    <div data-testid="editor-panel" data-path={filePath} />
  )
}))

import { showOpenedFile } from '@/lib/file-open-target'
import { useEditorStore } from '@/stores/editor-store'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import QuickTerminalsPage from './QuickTerminalsPage'
import { resetQuickTerminalStore } from './quick-terminal-store'
import { resetQuickWorkspaceStore } from './quick-terminal-workspace-store'

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
  const navigate = useNavigate()
  return (
    <>
      <div data-testid="location">{useLocation().pathname}</div>
      <button type="button" onClick={() => navigate('/quick-terminals')}>
        Navigate away
      </button>
      <button type="button" onClick={() => navigate(`/quick-terminals/${id}`)}>
        Return to entry
      </button>
    </>
  )
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
  vi.resetAllMocks()
  api.onChanged.mockImplementation(() => () => undefined)
  persistence.writeDebounced.mockResolvedValue({ success: true })
  persistence.flushPendingWrites.mockResolvedValue({ success: true })
  persistence.delete.mockResolvedValue({ success: true })
  terminalProps.current = []
  resetQuickTerminalStore()
  resetQuickWorkspaceStore()
  persistence.read.mockResolvedValue({ success: true, data: null })
  useEditorStore.setState({ openFiles: new Map(), activeFilePath: null })
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
  api.open.mockResolvedValue({
    success: true,
    data: { record: { ...record, terminalId: 'pty-1' }, terminalId: 'pty-1', spawned: true }
  })
})

afterEach(() => cleanup())

describe('QuickTerminalsPage', () => {
  it('shows the selected quick terminal and reopens it after the shell exits', async () => {
    api.open.mockResolvedValueOnce({
      success: true,
      data: {
        record: { ...record, terminalId: 'pty-1' },
        terminalId: 'pty-1',
        claim: 'c1',
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
    api.createTerminal.mockResolvedValue({
      success: true,
      data: { record: { ...record, terminalIds: ['pty-2'] }, terminalId: 'pty-2', spawned: true }
    })
    fireEvent.click(await screen.findByRole('button', { name: 'Reopen' }))

    await waitFor(() => expect(screen.getByTestId('connected-terminal').dataset.pty).toBe('pty-2'))
    expect(api.createTerminal).toHaveBeenCalledWith(id, FITTED.cols, FITTED.rows)
    expect(api.open).toHaveBeenCalledTimes(1)
  })

  it('closes one shell from the toolbar and keeps the entry for a new terminal tab', async () => {
    api.open
      .mockResolvedValueOnce({
        success: true,
        data: { record: { ...record, terminalId: 'pty-1' }, terminalId: 'pty-1', spawned: true }
      })
      .mockResolvedValueOnce({
        success: true,
        data: { record: { ...record, terminalId: 'pty-2' }, terminalId: 'pty-2', spawned: true }
      })
    api.closeTerminal.mockResolvedValue({ success: true, data: record })
    api.close.mockResolvedValue({ success: true, data: record })
    renderAt(`/quick-terminals/${id}`)
    await waitFor(() => expect(screen.getByTestId('connected-terminal').dataset.pty).toBe('pty-1'))

    fireEvent.click(screen.getByRole('button', { name: 'Close terminal' }))

    await waitFor(() => expect(screen.queryByTestId('connected-terminal')).toBeNull())
    expect(api.closeTerminal).toHaveBeenCalledWith(id, 'pty-1')
    expect(api.close).not.toHaveBeenCalled()
    expect(screen.getAllByText('Untitled terminal').length).toBeGreaterThan(0)
    api.createTerminal.mockResolvedValue({
      success: true,
      data: { record: { ...record, terminalIds: ['pty-2'] }, terminalId: 'pty-2', spawned: true }
    })
    fireEvent.click(screen.getByRole('button', { name: 'New terminal tab' }))
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
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Close all terminals' }))

    await waitFor(() => expect(screen.queryByTestId('connected-terminal')).toBeNull())
    expect(api.close).toHaveBeenCalledWith(id)
  })

  it('marks the quick terminals whose shell is running', async () => {
    const stopped = { ...record, id: '44444444-4444-4444-8444-444444444444', title: 'stopped' }
    const exited = { ...record, id: '55555555-5555-4555-8555-555555555555', title: 'exited' }
    api.list.mockResolvedValue({
      success: true,
      data: [{ ...record, title: 'live' }, stopped, exited]
    })
    useTerminalStore.setState({
      terminals: [
        { id: 'pty-live', name: 'live', shell: 'zsh', ptyId: 'pty-live', quickTerminalId: id },
        {
          id: 'pty-exited',
          name: 'exited',
          shell: 'zsh',
          ptyId: 'pty-exited',
          quickTerminalId: exited.id,
          healthStatus: 'exited'
        }
      ]
    })

    renderAt('/quick-terminals')

    const live = await screen.findByText('live')
    expect(live.parentElement?.textContent).toContain('Running')
    expect(screen.getByText('stopped').parentElement?.textContent).not.toContain('Running')
    expect(screen.getByText('exited').parentElement?.textContent).not.toContain('Running')
  })

  it('lists again when the host reports new quick terminals', async () => {
    renderAt('/quick-terminals')
    await screen.findAllByText('Untitled terminal')
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
    await screen.findAllByText('Untitled terminal')

    openMenu(/new quick terminal/i)
    fireEvent.click(await screen.findByRole('menuitem', { name: /in a new folder/i }))

    await waitFor(() =>
      expect(screen.getByTestId('location').textContent).toBe(`/quick-terminals/${created.id}`)
    )
    expect(api.create).toHaveBeenCalledWith({ target: { kind: 'workspace' } })
  })

  it('the new-terminal shortcut creates a quick terminal in its own folder', async () => {
    const created = { ...record, id: '33333333-3333-4333-8333-333333333333' }
    api.create.mockResolvedValue({ success: true, data: created })
    api.open.mockResolvedValue({ success: false, error: 'not in this test', code: 'X' })
    renderAt('/quick-terminals')
    await screen.findAllByText('Untitled terminal')

    fireEvent.keyDown(window, { key: 't', ctrlKey: true })

    await waitFor(() =>
      expect(screen.getByTestId('location').textContent).toBe(`/quick-terminals/${created.id}`)
    )
    expect(api.create).toHaveBeenCalledWith({ target: { kind: 'workspace' } })
  })

  it('opens a file in a workspace tab and returns to the terminal on close', async () => {
    api.open.mockResolvedValue({ success: false, error: 'not in this test', code: 'X' })
    const file = `${record.cwd}/notes.md`
    useEditorStore.setState({
      openFiles: new Map([[file, { filePath: file, isDirty: false } as never]])
    })
    renderAt(`/quick-terminals/${id}`)
    await screen.findAllByText('Untitled terminal')

    act(() => showOpenedFile(file))

    const preview = await screen.findByRole('region', { name: 'Preview of notes.md' })
    // The editor is lazy: the region is up before it is.
    expect((await within(preview).findByTestId('editor-panel')).getAttribute('data-path')).toBe(
      file
    )
    fireEvent.click(screen.getByRole('button', { name: 'Close preview' }))
    await waitFor(() =>
      expect(screen.getByRole('tab', { name: /Terminal 1/ }).getAttribute('aria-selected')).toBe(
        'true'
      )
    )
    expect(screen.queryByRole('region', { name: 'Preview of notes.md' })).toBeNull()
    expect(useEditorStore.getState().openFiles.has(file)).toBe(false)
  })

  it('opens a project quick terminal in its active worktree', async () => {
    api.create.mockResolvedValue({ success: false, error: 'nope', code: 'X' })
    renderAt('/quick-terminals')
    await screen.findAllByText('Untitled terminal')

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
  it('adds a shell inside the selected entry, switches to files and keeps inactive terminals mounted', async () => {
    const first = { ...record, terminalIds: ['pty-1'], terminalId: 'pty-1' }
    const both = { ...first, terminalIds: ['pty-1', 'pty-2'] }
    api.list.mockResolvedValue({ success: true, data: [first] })
    api.open.mockResolvedValue({
      success: true,
      data: { record: first, terminalId: 'pty-1', spawned: false }
    })
    api.createTerminal.mockResolvedValue({
      success: true,
      data: { record: both, terminalId: 'pty-2', spawned: true, claim: 'core-owned-output' }
    })
    renderAt(`/quick-terminals/${id}`)
    await waitFor(() => expect(screen.getByTestId('connected-terminal').dataset.pty).toBe('pty-1'))
    const one = screen.getByTestId('connected-terminal')
    fireEvent.click(screen.getByRole('button', { name: 'New terminal tab' }))
    await waitFor(() =>
      expect(screen.getAllByTestId('connected-terminal').map((item) => item.dataset.pty)).toEqual([
        'pty-1',
        'pty-2'
      ])
    )
    const two = screen
      .getAllByTestId('connected-terminal')
      .find((item) => item.dataset.pty === 'pty-2')!
    expect(api.createTerminal).toHaveBeenCalledWith(id, FITTED.cols, FITTED.rows)
    expect(api.open).toHaveBeenCalledTimes(1)
    expect(one.dataset.visible).toBe('false')
    expect(one.dataset.autoFocus).toBe('false')
    act(() => {
      useEditorStore.setState({
        openFiles: new Map([
          ['/tmp/example.ts', { filePath: '/tmp/example.ts', isDirty: false } as never]
        ])
      })
      showOpenedFile('/tmp/example.ts')
    })
    await screen.findByRole('region', { name: 'Preview of example.ts' })
    expect(screen.getAllByTestId('connected-terminal')).toEqual([one, two])
    expect(two.dataset.visible).toBe('false')
    fireEvent.click(screen.getByRole('tab', { name: 'Terminal 2' }))
    await waitFor(() => expect(useEditorStore.getState().activeFilePath).toBeNull())
    expect(two.dataset.visible).toBe('true')
    expect(screen.getAllByTestId('connected-terminal')).toEqual([one, two])
    fireEvent.click(screen.getByRole('tab', { name: 'example.ts' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Close preview' }))
    await waitFor(() =>
      expect(screen.getByRole('tab', { name: 'Terminal 2' }).getAttribute('aria-selected')).toBe(
        'true'
      )
    )
    expect(api.closeTerminal).not.toHaveBeenCalled()
    expect(api.close).not.toHaveBeenCalled()
  })

  it('new-terminal shortcut adds a selected-entry shell instead of creating a list entry', async () => {
    api.createTerminal.mockResolvedValue({
      success: true,
      data: {
        record: { ...record, terminalIds: ['pty-1', 'pty-2'] },
        terminalId: 'pty-2',
        spawned: true
      }
    })
    renderAt(`/quick-terminals/${id}`)
    await screen.findByTestId('connected-terminal')
    fireEvent.keyDown(window, { key: 't', ctrlKey: true })
    await waitFor(() => expect(api.createTerminal).toHaveBeenCalledOnce())
    expect(api.create).not.toHaveBeenCalled()
  })

  it('blocks dirty file close and preserves the shared buffer', async () => {
    renderAt(`/quick-terminals/${id}`)
    await screen.findByTestId('connected-terminal')
    act(() => {
      useEditorStore.setState({
        openFiles: new Map([
          ['/tmp/dirty.ts', { filePath: '/tmp/dirty.ts', isDirty: false } as never]
        ])
      })
      showOpenedFile('/tmp/dirty.ts')
    })
    await screen.findByRole('region', { name: 'Preview of dirty.ts' })
    act(() =>
      useEditorStore.setState((state) => {
        const openFiles = new Map(state.openFiles)
        const file = openFiles.get('/tmp/dirty.ts')!
        openFiles.set('/tmp/dirty.ts', { ...file, isDirty: true })
        return { openFiles }
      })
    )
    fireEvent.click(screen.getByRole('button', { name: 'Close preview' }))
    expect(screen.getByRole('region', { name: 'Preview of dirty.ts' })).toBeTruthy()
    expect(useEditorStore.getState().openFiles.get('/tmp/dirty.ts')?.isDirty).toBe(true)
  })

  it('restores host terminal tabs without spawning new shells and provides narrow-screen back navigation', async () => {
    const host = { ...record, terminalIds: ['pty-1', 'pty-2'], terminalId: 'pty-1' }
    api.list.mockResolvedValue({ success: true, data: [host] })
    api.open.mockImplementation(async (_id, _cols, _rows, ptyId) => ({
      success: true,
      data: { record: host, terminalId: ptyId, spawned: false }
    }))
    useEditorStore.setState({
      openFiles: new Map([
        ['/tmp/saved.ts', { filePath: '/tmp/saved.ts', isDirty: false } as never]
      ])
    })
    persistence.read.mockResolvedValue({
      success: true,
      data: {
        tabs: [
          { id: 'two', type: 'terminal', terminalId: 'pty-2' },
          { id: 'file', type: 'editor', filePath: '/tmp/saved.ts' },
          { id: 'gone', type: 'terminal', terminalId: 'gone' }
        ],
        activeTabId: 'file',
        lastTerminalTabId: 'two'
      }
    })
    renderAt(`/quick-terminals/${id}`)
    await screen.findByRole('region', { name: 'Preview of saved.ts' })
    await waitFor(() => expect(api.open).toHaveBeenCalledTimes(2))
    expect(api.createTerminal).not.toHaveBeenCalled()
    expect(screen.getByTestId('quick-terminal-list').className).toContain('hidden md:flex')
    expect(screen.getByRole('tablist').className).toContain('overflow-x-auto')
    fireEvent.click(screen.getByRole('button', { name: 'Back to list' }))
    await waitFor(() => expect(screen.queryByRole('tablist')).toBeNull())
    expect(screen.getByTestId('quick-terminal-list').className).not.toContain('hidden')
    expect(api.close).not.toHaveBeenCalled()
    expect(api.closeTerminal).not.toHaveBeenCalled()
  })

  it('keeps a pending spawn attached to its entry across navigation without a duplicate PTY', async () => {
    let resolveOpen!: (value: unknown) => void
    api.open.mockReturnValue(
      new Promise((resolve) => {
        resolveOpen = resolve
      })
    )
    renderAt(`/quick-terminals/${id}`)
    await waitFor(() => expect(api.open).toHaveBeenCalledOnce())
    fireEvent.click(screen.getByRole('button', { name: 'Navigate away' }))
    fireEvent.click(screen.getByRole('button', { name: 'Return to entry' }))
    await act(async () => {
      resolveOpen({
        success: true,
        data: { record: { ...record, terminalIds: ['pty-1'] }, terminalId: 'pty-1', spawned: true }
      })
    })
    await screen.findByTestId('connected-terminal')
    expect(api.open).toHaveBeenCalledOnce()
    expect(api.close).not.toHaveBeenCalled()
    expect(api.closeTerminal).not.toHaveBeenCalled()
  })
  it('captures close-tab shortcuts before project handlers and preserves dirty buffers on navigation', async () => {
    renderAt(`/quick-terminals/${id}`)
    await waitFor(() => expect(screen.getByTestId('connected-terminal').dataset.pty).toBe('pty-1'))
    act(() => {
      useEditorStore.setState({
        openFiles: new Map([
          ['/tmp/shortcut.ts', { filePath: '/tmp/shortcut.ts', isDirty: true } as never]
        ])
      })
      showOpenedFile('/tmp/shortcut.ts')
    })
    await screen.findByRole('region', { name: 'Preview of shortcut.ts' })
    const projectHandler = vi.fn()
    window.addEventListener('keydown', projectHandler)
    fireEvent.keyDown(window, { key: 'w', ctrlKey: true })
    window.removeEventListener('keydown', projectHandler)
    expect(projectHandler).not.toHaveBeenCalled()
    expect(screen.getByRole('region', { name: 'Preview of shortcut.ts' })).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Navigate away' }))
    expect(useEditorStore.getState().openFiles.get('/tmp/shortcut.ts')?.isDirty).toBe(true)
    expect(api.close).not.toHaveBeenCalled()
    expect(api.closeTerminal).not.toHaveBeenCalled()
  })
  it('lets a dead persisted host terminal be explicitly reopened as a new shell', async () => {
    const dead = { ...record, terminalIds: ['dead'], terminalId: 'dead' }
    api.list.mockResolvedValue({ success: true, data: [dead] })
    api.open.mockResolvedValue({
      success: false,
      code: 'QUICK_TERMINAL_INVALID_TARGET',
      error: 'Terminal is no longer alive'
    })
    api.createTerminal.mockResolvedValue({
      success: true,
      data: {
        record: { ...record, terminalIds: ['replacement'] },
        terminalId: 'replacement',
        spawned: true
      }
    })
    renderAt(`/quick-terminals/${id}`)
    await screen.findByRole('alert')
    expect(api.createTerminal).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Reopen' }))
    await waitFor(() =>
      expect(screen.getByTestId('connected-terminal').dataset.pty).toBe('replacement')
    )
    expect(api.open).toHaveBeenCalledWith(id, FITTED.cols, FITTED.rows, 'dead')
    expect(api.createTerminal).toHaveBeenCalledWith(id, FITTED.cols, FITTED.rows)
  })
})
