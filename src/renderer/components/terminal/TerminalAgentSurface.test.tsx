import type { AgentChatSnapshot } from '@shared/types/agent-chat.types'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { useEffect } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { TerminalAgentSurface } from './TerminalAgentSurface'

const mocks = vi.hoisted(() => ({
  snapshot: null as AgentChatSnapshot | null,
  error: null as string | null,
  refresh: vi.fn(),
  loadOlder: vi.fn(),
  loadingOlder: false,
  browsingOlder: false,
  returnLatest: vi.fn(),
  send: vi.fn(),
  control: vi.fn(),
  mount: vi.fn(),
  unmount: vi.fn(),
  chatThrows: false
}))
vi.mock('../../hooks/use-terminal-agent-chat', () => ({
  useTerminalAgentChat: () => ({
    snapshot: mocks.snapshot,
    error: mocks.error,
    loadOlder: mocks.loadOlder,
    loadingOlder: mocks.loadingOlder,
    browsingOlder: mocks.browsingOlder,
    returnLatest: mocks.returnLatest,
    refresh: mocks.refresh
  })
}))
vi.mock('../../lib/log-api', () => ({ logFrontendError: vi.fn() }))
vi.mock('../../lib/agent-chat-api', () => ({
  agentChatApi: { send: mocks.send, control: mocks.control }
}))
vi.mock('../../stores/terminal-store', () => ({
  useTerminalStore: (select: (state: unknown) => unknown) => select({ terminals: [] })
}))
vi.mock('../../features/agent-session/chat/ChatMessageList', () => ({
  ChatMessageList: ({ items }: { items: unknown[] }) => {
    if (mocks.chatThrows) throw new Error('chat render failed')
    return <div data-testid="shared-timeline" data-count={items.length} />
  }
}))
function Pty({ visible }: { visible: boolean }) {
  useEffect(() => {
    mocks.mount()
    return () => mocks.unmount()
  }, [])
  return (
    <div data-testid="pty" data-visible={String(visible)}>
      PTY stays alive
    </div>
  )
}
let nextId = 0
function surface(id = `tab-${++nextId}`) {
  return (
    <TerminalAgentSurface terminalId={id} ptyId="host-pty" isVisible>
      {(visible) => <Pty visible={visible} />}
    </TerminalAgentSurface>
  )
}
function openChat() {
  fireEvent.click(screen.getByRole('button', { name: '对话', exact: true }))
}

beforeEach(() => {
  mocks.snapshot = {
    terminal_id: 'host-pty',
    session_id: 's1',
    agent: 'qin-code',
    cursor: 1,
    messages: [],
    tool_calls: [],
    reset: false,
    deliveries: [],
    state: 'ready',
    hook_state: 'idle'
  }
  mocks.error = null
  mocks.loadingOlder = false
  mocks.browsingOlder = false
  mocks.returnLatest.mockReset().mockResolvedValue(undefined)
  mocks.loadOlder.mockReset().mockResolvedValue(undefined)
  for (const mock of [mocks.refresh, mocks.send, mocks.control, mocks.mount, mocks.unmount])
    mock.mockReset()
  mocks.refresh.mockResolvedValue(undefined)
  mocks.send.mockResolvedValue({})
  mocks.control.mockResolvedValue({})
  mocks.chatThrows = false
  localStorage.clear()
})
afterEach(cleanup)

describe('terminal agent surface', () => {
  it('switches views without remounting/unmounting PTY, and restores its visibility', () => {
    render(surface())
    const pty = screen.getByTestId('pty')
    openChat()
    expect(pty).not.toBeVisible()
    expect(pty).toHaveAttribute('data-visible', 'false')
    expect(mocks.mount).toHaveBeenCalledTimes(1)
    expect(mocks.unmount).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: '终端', exact: true }))
    expect(screen.getByTestId('pty')).toBe(pty)
    expect(pty).toBeVisible()
    expect(pty).toHaveAttribute('data-visible', 'true')
    expect(mocks.unmount).not.toHaveBeenCalled()
  })
  it('contains a conversation render error so the terminal survives', () => {
    mocks.chatThrows = true
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(surface())
    const pty = screen.getByTestId('pty')
    openChat()
    expect(screen.getByText('chat render failed')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: '终端', exact: true }))
    expect(screen.getByTestId('pty')).toBe(pty)
    expect(pty).toBeVisible()
    expect(mocks.unmount).not.toHaveBeenCalled()
    consoleError.mockRestore()
  })

  it('remembers view per terminal tab rather than leaking across tabs', () => {
    const first = `remember-${++nextId}`
    const mounted = render(surface(first))
    openChat()
    mounted.unmount()
    const restored = render(surface(first))
    expect(screen.getByRole('button', { name: '对话', exact: true })).toHaveAttribute(
      'aria-pressed',
      'true'
    )
    restored.unmount()
    render(surface())
    expect(screen.getByRole('button', { name: '终端', exact: true })).toHaveAttribute(
      'aria-pressed',
      'true'
    )
  })
  it('offers older-history loading only while a disk cursor exists and disables duplicate requests', () => {
    mocks.snapshot!.older_cursor = 10
    const view = surface()
    const mounted = render(view)
    openChat()
    fireEvent.click(screen.getByRole('button', { name: '加载更早消息' }))
    expect(mocks.loadOlder).toHaveBeenCalledOnce()
    mocks.browsingOlder = true
    mounted.rerender(surface(view.props.terminalId))
    fireEvent.click(screen.getByRole('button', { name: '返回最新消息' }))
    expect(mocks.returnLatest).toHaveBeenCalledOnce()
    mocks.loadingOlder = true
    mounted.rerender(surface(view.props.terminalId))
    expect(screen.getByRole('button', { name: '正在加载…' })).toBeDisabled()
    mocks.snapshot!.older_cursor = null
    mounted.rerender(surface(view.props.terminalId))
    expect(screen.queryByRole('button', { name: '加载更早消息' })).not.toBeInTheDocument()
  })
  it('sends exactly once while pending, preserves multiline text and refreshes delivery state', async () => {
    let resolve: (() => void) | undefined
    mocks.send.mockImplementation(
      () =>
        new Promise<void>((done) => {
          resolve = done
        })
    )
    render(surface())
    openChat()
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'one\ntwo' } })
    const send = screen.getByRole('button', { name: '发送', exact: true })
    fireEvent.click(send)
    fireEvent.click(send)
    expect(mocks.send).toHaveBeenCalledExactlyOnceWith('host-pty', 'one\ntwo')
    await act(async () => resolve?.())
    expect(screen.getByRole('textbox')).toHaveValue('')
    expect(mocks.refresh).toHaveBeenCalledOnce()
  })
  it('exposes interrupt/stop explicitly through the host control contract', async () => {
    render(surface())
    openChat()
    fireEvent.click(screen.getByRole('button', { name: '中断 (Esc)' }))
    await waitFor(() => expect(mocks.control).toHaveBeenCalledWith('host-pty', 'interrupt'))
    await waitFor(() =>
      expect(screen.getByRole('button', { name: '强制停止 (Ctrl-C)' })).toBeEnabled()
    )
    fireEvent.click(screen.getByRole('button', { name: '强制停止 (Ctrl-C)' }))
    await waitFor(() => expect(mocks.control).toHaveBeenCalledWith('host-pty', 'stop'))
  })
  it('renders queue and unconfirmed badges without retrying delivery', () => {
    mocks.snapshot!.deliveries = [
      { id: 'q', text: 'queued text', status: 'queued' },
      { id: 'u', text: 'uncertain text', status: 'unconfirmed' }
    ]
    render(surface())
    openChat()
    expect(screen.getByText('排队中')).toBeInTheDocument()
    expect(screen.getByText('未确认')).toBeInTheDocument()
    expect(screen.getByText('不会自动重发')).toBeInTheDocument()
    expect(mocks.send).not.toHaveBeenCalled()
  })
  it('offers an explicit cold queued override only with the server flag and reuses the receipt', () => {
    mocks.snapshot!.can_force_send = true
    mocks.snapshot!.hook_state = 'unknown'
    mocks.snapshot!.deliveries = [
      { id: 'cold', text: 'original queued raw prompt', status: 'queued' }
    ]
    mocks.send.mockReturnValue(new Promise(() => {}))
    render(surface())
    openChat()
    const action = screen.getByRole('button', { name: '立即发送' })
    fireEvent.click(action)
    fireEvent.click(action)
    expect(mocks.send).toHaveBeenCalledExactlyOnceWith(expect.any(String), '', {
      force: true,
      delivery_id: 'cold'
    })
    expect(action).toBeDisabled()
    expect(screen.getByText('original queued raw prompt')).toBeInTheDocument()
    expect(screen.getByTestId('shared-timeline')).toHaveAttribute('data-count', '0')
  })

  it.each([
    { flag: undefined, hook: 'unknown', status: 'queued', pending: false },
    { flag: false, hook: 'unknown', status: 'queued', pending: false },
    { flag: true, hook: 'busy', status: 'queued', pending: false },
    { flag: true, hook: 'running', status: 'queued', pending: false },
    { flag: true, hook: 'unknown', status: 'queued', pending: true },
    { flag: true, hook: 'unknown', status: 'pending', pending: false },
    { flag: true, hook: 'unknown', status: 'delivered', pending: false },
    { flag: true, hook: 'unknown', status: 'unconfirmed', pending: false }
  ] as const)('hides force send for unavailable/busy/nonqueued state: %j', ({
    flag,
    hook,
    status,
    pending
  }) => {
    mocks.snapshot!.can_force_send = flag
    mocks.snapshot!.hook_state = hook
    mocks.snapshot!.deliveries = [
      { id: 'receipt', text: 'queued', status },
      ...(pending ? [{ id: 'pending', text: 'in flight', status: 'pending' as const }] : [])
    ]
    render(surface())
    openChat()
    expect(screen.queryByRole('button', { name: '立即发送' })).not.toBeInTheDocument()
    expect(mocks.send).not.toHaveBeenCalled()
  })

  it('shows a recoverable degradation state and disables unsafe sends, keeping terminal accessible', () => {
    mocks.snapshot = { ...mocks.snapshot!, state: 'degraded', reason: 'unsupported record' }
    render(surface())
    openChat()
    expect(screen.getByRole('status')).toHaveTextContent('unsupported record')
    expect(screen.getByRole('textbox')).toBeDisabled()
    expect(screen.getByRole('button', { name: '终端', exact: true })).toBeEnabled()
  })
  it('shows uncertain failed sends and does not automatically retry', async () => {
    mocks.send.mockRejectedValue(new Error('network lost'))
    render(surface())
    openChat()
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'hello' } })
    fireEvent.click(screen.getByRole('button', { name: '发送', exact: true }))
    expect(await screen.findByRole('alert')).toHaveTextContent('不要重复发送')
    expect(mocks.send).toHaveBeenCalledTimes(1)
  })
})
