import type { IpcResult } from '@shared/types/ipc.types'
import type { PluginConnectionChanged, PluginEvent, ViewSnapshot } from '@shared/types/plugin.types'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { PluginViewPanel } from './PluginViewPanel'
import { pluginApi } from './plugin-api'

vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn().mockResolvedValue(undefined) }))
vi.mock('./plugin-api', () => ({
  pluginApi: {
    getView: vi.fn(),
    dispatchAction: vi.fn(),
    onChanged: vi.fn(),
    onConnectionChanged: vi.fn()
  }
}))

const generation = '11111111-1111-4111-8111-111111111111'
const nextGeneration = '22222222-2222-4222-8222-222222222222'
const bootId = '33333333-3333-4333-8333-333333333333'
function snapshot(revision = 1, gen = generation, text = 'Count 0'): ViewSnapshot {
  return {
    pluginId: 'org.termul.example',
    generation: gen,
    revision,
    configRevision: 1,
    tree: {
      schemaVersion: 1,
      viewId: 'counter',
      root: {
        id: 'root',
        type: 'column',
        props: {},
        children: [
          { id: 'text', type: 'text', props: { text } },
          { id: 'increment', type: 'button', props: { label: 'Increment', actionId: 'add' } },
          { id: 'name', type: 'input', props: { label: 'Name', value: '', actionId: 'rename' } }
        ]
      }
    }
  }
}
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}
let changed: (event: PluginEvent) => void
let connection: (event: PluginConnectionChanged) => void
const unsubscribe = vi.fn()
const disconnect = vi.fn()
const props = { pluginId: 'org.termul.example', viewId: 'counter' }
function emit(overrides: Partial<PluginEvent> = {}): void {
  act(() => changed({ schemaVersion: 1, bootId, seq: 1, type: 'view_changed', ...overrides }))
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(pluginApi.getView).mockResolvedValue({ success: true, data: snapshot() })
  vi.mocked(pluginApi.dispatchAction).mockResolvedValue({
    success: true,
    data: snapshot(2, generation, 'Count 1')
  })
  vi.mocked(pluginApi.onChanged).mockImplementation((handler) => {
    changed = handler
    return unsubscribe
  })
  vi.mocked(pluginApi.onConnectionChanged).mockImplementation((handler) => {
    connection = handler
    return disconnect
  })
})
afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('PluginViewPanel', () => {
  it('subscribes before fetching and replaces the tree from an action reply', async () => {
    render(<PluginViewPanel {...props} />)
    expect(await screen.findByText('Count 0')).toBeInTheDocument()
    expect(vi.mocked(pluginApi.onChanged).mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(pluginApi.getView).mock.invocationCallOrder[0]
    )
    fireEvent.click(screen.getByRole('button', { name: 'Increment' }))
    expect(await screen.findByText('Count 1')).toBeInTheDocument()
    expect(pluginApi.dispatchAction).toHaveBeenCalledWith({
      schemaVersion: 1,
      eventId: expect.stringMatching(/^[\da-f-]{36}$/),
      ...props,
      generation,
      viewRevision: 1,
      nodeId: 'increment',
      actionId: 'add',
      kind: 'click',
      value: null
    })
    expect(screen.queryByText('Count 0')).not.toBeInTheDocument()
  })

  it('serializes queued events and sends the latest returned revision', async () => {
    const first = deferred<IpcResult<ViewSnapshot>>()
    vi.mocked(pluginApi.dispatchAction).mockReturnValueOnce(first.promise)
    render(<PluginViewPanel {...props} />)
    const button = await screen.findByRole('button', { name: 'Increment' })
    fireEvent.click(button)
    fireEvent.click(button)
    expect(pluginApi.dispatchAction).toHaveBeenCalledTimes(1)
    await act(async () => first.resolve({ success: true, data: snapshot(4) }))
    await waitFor(() => expect(pluginApi.dispatchAction).toHaveBeenCalledTimes(2))
    expect(vi.mocked(pluginApi.dispatchAction).mock.calls[1][0].viewRevision).toBe(4)
    expect(vi.mocked(pluginApi.dispatchAction).mock.calls[0][0].eventId).not.toBe(
      vi.mocked(pluginApi.dispatchAction).mock.calls[1][0].eventId
    )
  })

  it('drops a queued click if the preceding reply removed the control', async () => {
    const first = deferred<IpcResult<ViewSnapshot>>()
    vi.mocked(pluginApi.dispatchAction).mockReturnValueOnce(first.promise)
    render(<PluginViewPanel {...props} />)
    const button = await screen.findByRole('button', { name: 'Increment' })
    fireEvent.click(button)
    fireEvent.click(button)
    const withoutButton = snapshot(2)
    if (withoutButton.tree.root.type === 'column') {
      withoutButton.tree.root.children = withoutButton.tree.root.children.filter(
        (node) => node.id !== 'increment'
      )
    }
    await act(async () => first.resolve({ success: true, data: withoutButton }))
    expect(pluginApi.dispatchAction).toHaveBeenCalledTimes(1)
    expect(screen.queryByRole('button', { name: 'Increment' })).not.toBeInTheDocument()
  })

  it('debounces text changes for 150 ms without losing the local draft', async () => {
    render(<PluginViewPanel {...props} />)
    const input = await screen.findByRole('textbox', { name: 'Name' })
    vi.useFakeTimers()
    fireEvent.change(input, { target: { value: 'a' } })
    fireEvent.change(input, { target: { value: 'abc' } })
    expect(input).toHaveValue('abc')
    await act(async () => vi.advanceTimersByTime(149))
    expect(pluginApi.dispatchAction).not.toHaveBeenCalled()
    await act(async () => vi.advanceTimersByTime(1))
    expect(pluginApi.dispatchAction).toHaveBeenCalledTimes(1)
    expect(pluginApi.dispatchAction).toHaveBeenCalledWith(
      expect.objectContaining({ kind: 'change', value: 'abc', nodeId: 'name' })
    )
  })

  it('preserves newer typed text when an older input reply arrives', async () => {
    const first = deferred<IpcResult<ViewSnapshot>>()
    vi.mocked(pluginApi.dispatchAction).mockReturnValueOnce(first.promise)
    render(<PluginViewPanel {...props} />)
    const input = await screen.findByRole('textbox', { name: 'Name' })
    vi.useFakeTimers()
    fireEvent.change(input, { target: { value: 'a' } })
    await act(async () => vi.advanceTimersByTime(150))
    fireEvent.change(input, { target: { value: 'ab' } })
    const reply = snapshot(2)
    if (reply.tree.root.type === 'column') {
      const node = reply.tree.root.children.find((node) => node.id === 'name')
      if (node?.type === 'input') node.props.value = 'a'
    }
    await act(async () => first.resolve({ success: true, data: reply }))
    expect(input).toHaveValue('ab')
    fireEvent.change(input, { target: { value: 'abc' } })
    await act(async () => vi.advanceTimersByTime(150))
    expect(vi.mocked(pluginApi.dispatchAction).mock.calls[1][0]).toEqual(
      expect.objectContaining({ value: 'abc', viewRevision: 2 })
    )
  })

  it('preserves a rapid toggle reversal while its first action is pending', async () => {
    const first = deferred<IpcResult<ViewSnapshot>>()
    const initial = snapshot()
    if (initial.tree.root.type === 'column') {
      initial.tree.root.children.push({
        id: 'toggle',
        type: 'toggle',
        props: { label: 'Enabled', checked: false, actionId: 'set-enabled' }
      })
    }
    vi.mocked(pluginApi.getView).mockResolvedValue({ success: true, data: initial })
    const second = deferred<IpcResult<ViewSnapshot>>()
    vi.mocked(pluginApi.dispatchAction)
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise)
    render(<PluginViewPanel {...props} />)
    const toggle = await screen.findByRole('switch', { name: 'Enabled' })
    fireEvent.click(toggle)
    fireEvent.click(toggle)
    expect(toggle).not.toBeChecked()
    const reply = structuredClone(initial)
    reply.revision = 2
    if (reply.tree.root.type === 'column') {
      const node = reply.tree.root.children.find((node) => node.id === 'toggle')
      if (node?.type === 'toggle') node.props.checked = true
    }
    await act(async () => first.resolve({ success: true, data: reply }))
    expect(vi.mocked(pluginApi.dispatchAction).mock.calls[1][0]).toEqual(
      expect.objectContaining({ value: false, viewRevision: 2 })
    )
  })

  it('ignores an old generation response and drops its queued actions', async () => {
    const old = deferred<IpcResult<ViewSnapshot>>()
    vi.mocked(pluginApi.dispatchAction).mockReturnValueOnce(old.promise)
    render(<PluginViewPanel {...props} />)
    const button = await screen.findByRole('button', { name: 'Increment' })
    fireEvent.click(button)
    fireEvent.click(button)
    vi.mocked(pluginApi.getView).mockResolvedValue({
      success: true,
      data: snapshot(1, nextGeneration, 'New generation')
    })
    emit({ type: 'state_changed', pluginId: props.pluginId, generation: nextGeneration })
    expect(await screen.findByText('New generation')).toBeInTheDocument()
    await act(async () =>
      old.resolve({ success: true, data: snapshot(2, generation, 'Stale response') })
    )
    expect(screen.queryByText('Stale response')).not.toBeInTheDocument()
    expect(pluginApi.dispatchAction).toHaveBeenCalledTimes(1)
    vi.mocked(pluginApi.dispatchAction).mockResolvedValueOnce({
      success: true,
      data: snapshot(2, nextGeneration, 'Current response')
    })
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Increment' })))
    expect(vi.mocked(pluginApi.dispatchAction).mock.calls[1][0].generation).toBe(nextGeneration)
    expect(screen.getByText('Current response')).toBeInTheDocument()
  })

  it('ignores old reads after a newer lifecycle notification', async () => {
    const old = deferred<IpcResult<ViewSnapshot>>()
    vi.mocked(pluginApi.getView).mockReturnValueOnce(old.promise)
    render(<PluginViewPanel {...props} />)
    vi.mocked(pluginApi.getView).mockResolvedValue({
      success: true,
      data: snapshot(1, nextGeneration, 'Fresh tree')
    })
    emit({ type: 'state_changed', pluginId: props.pluginId, generation: nextGeneration })
    expect(await screen.findByText('Fresh tree')).toBeInTheDocument()
    await act(async () => old.resolve({ success: true, data: snapshot() }))
    expect(screen.queryByText('Count 0')).not.toBeInTheDocument()
  })

  it('refreshes only relevant views and fully refreshes after sequence gaps', async () => {
    render(<PluginViewPanel {...props} />)
    await screen.findByText('Count 0')
    emit({ pluginId: 'org.other.plugin' })
    expect(pluginApi.getView).toHaveBeenCalledTimes(1)
    emit({ seq: 2, pluginId: props.pluginId, viewId: 'other' })
    expect(pluginApi.getView).toHaveBeenCalledTimes(1)
    emit({ seq: 4, pluginId: 'org.other.plugin' })
    await waitFor(() => expect(pluginApi.getView).toHaveBeenCalledTimes(2))
  })

  it('disconnects, discards pending edits, and fetches fresh state on reconnect', async () => {
    render(<PluginViewPanel {...props} />)
    const input = await screen.findByRole('textbox', { name: 'Name' })
    vi.useFakeTimers()
    fireEvent.change(input, { target: { value: 'Do not replay' } })
    act(() => connection({ state: 'disconnected', bootId: null }))
    expect(screen.getByText('state.disconnected')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Increment' })).not.toBeInTheDocument()
    await act(async () => vi.advanceTimersByTime(150))
    expect(pluginApi.dispatchAction).not.toHaveBeenCalled()
    vi.mocked(pluginApi.getView).mockResolvedValue({
      success: true,
      data: snapshot(1, nextGeneration, 'Reconnected')
    })
    await act(async () => connection({ state: 'connected', bootId }))
    expect(screen.getByText('Reconnected')).toBeInTheDocument()
    expect(pluginApi.dispatchAction).not.toHaveBeenCalled()
  })

  it('unsubscribes both event listeners and cancels debounces on unmount', async () => {
    const { unmount } = render(<PluginViewPanel {...props} />)
    const input = await screen.findByRole('textbox', { name: 'Name' })
    vi.useFakeTimers()
    fireEvent.change(input, { target: { value: 'pending' } })
    unmount()
    await act(async () => vi.advanceTimersByTime(200))
    expect(unsubscribe).toHaveBeenCalledTimes(1)
    expect(disconnect).toHaveBeenCalledTimes(1)
    expect(pluginApi.dispatchAction).not.toHaveBeenCalled()
  })

  it('shows offline on core unavailable and offers an explicit retry', async () => {
    vi.mocked(pluginApi.getView).mockResolvedValueOnce({
      success: false,
      error: 'unavailable',
      code: 'PLUGIN_CORE_UNAVAILABLE'
    })
    render(<PluginViewPanel {...props} />)
    expect(await screen.findByText('state.disconnected')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'view.retry' }))
    expect(await screen.findByText('Count 0')).toBeInTheDocument()
  })

  it('reports invalid UI and unexpected failures with no raw plugin errors', async () => {
    vi.mocked(pluginApi.getView).mockResolvedValueOnce({
      success: false,
      code: 'PLUGIN_INVALID_UI',
      error: '<private plugin error>'
    })
    render(<PluginViewPanel {...props} />)
    expect(await screen.findByRole('alert')).toHaveTextContent('view.invalid')
    expect(screen.queryByText('<private plugin error>')).not.toBeInTheDocument()
    vi.mocked(pluginApi.getView).mockRejectedValueOnce(new Error('private'))
    fireEvent.click(screen.getByRole('button', { name: 'view.retry' }))
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('view.unavailable'))
  })

  it('removes all previous controls when the next tree is invalid', async () => {
    vi.mocked(pluginApi.dispatchAction).mockResolvedValueOnce({
      success: false,
      code: 'PLUGIN_INVALID_UI',
      error: 'invalid'
    })
    render(<PluginViewPanel {...props} />)
    fireEvent.click(await screen.findByRole('button', { name: 'Increment' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('view.invalid')
    expect(screen.queryByRole('button', { name: 'Increment' })).not.toBeInTheDocument()
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument()
  })

  it('does not replay stale actions after a host stale-view error', async () => {
    vi.mocked(pluginApi.dispatchAction).mockResolvedValueOnce({
      success: false,
      code: 'PLUGIN_STALE_VIEW',
      error: 'stale'
    })
    render(<PluginViewPanel {...props} />)
    const button = await screen.findByRole('button', { name: 'Increment' })
    fireEvent.click(button)
    fireEvent.click(button)
    await waitFor(() => expect(pluginApi.getView).toHaveBeenCalledTimes(2))
    expect(pluginApi.dispatchAction).toHaveBeenCalledTimes(1)
  })
})
