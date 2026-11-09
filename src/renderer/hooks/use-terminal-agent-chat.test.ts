import type { AgentChatSnapshot } from '@shared/types/agent-chat.types'
import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ChatMessage } from '../features/agent-session/stores/acp-store'
import type { ToolCall } from '../lib/acp-api'
import { mergeAgentChatSnapshot, useTerminalAgentChat } from './use-terminal-agent-chat'

const mocks = vi.hoisted(() => ({ snapshot: vi.fn(), desktop: false, listen: vi.fn() }))
vi.mock('../lib/log-api', () => ({ logFrontendError: vi.fn() }))
vi.mock('../lib/agent-chat-api', () => ({
  agentChatApi: { snapshot: mocks.snapshot },
  subscribeAgentChatUpdated: mocks.listen
}))
vi.mock('../lib/tauri-runtime', () => ({ isTauriContext: () => mocks.desktop }))
type Snapshot = AgentChatSnapshot<ChatMessage, ToolCall>
const message = (id: string, text = id): ChatMessage => ({
  id,
  role: 'agent',
  blocks: [{ type: 'text', text }],
  timestamp: 1
})
const snap = (overrides: Partial<Snapshot> = {}): Snapshot => ({
  terminal_id: 'pty',
  session_id: 's1',
  agent: 'qin-code',
  cursor: 1,
  messages: [message('m1')],
  tool_calls: [],
  reset: false,
  deliveries: [],
  state: 'ready',
  hook_state: 'idle',
  ...overrides
})

beforeEach(() => {
  vi.useFakeTimers()
  mocks.snapshot.mockReset()
  mocks.listen.mockReset()
  mocks.desktop = false
})
afterEach(() => {
  vi.useRealTimers()
})

describe('terminal agent chat snapshots', () => {
  it('merges realtime updates by message id and replaces tool calls', () => {
    const result = mergeAgentChatSnapshot(
      snap(),
      snap({ cursor: 2, messages: [message('m1', 'updated'), message('m2')] })
    )
    expect(result.messages.map((item) => item.id)).toEqual(['m1', 'm2'])
    expect(result.messages[0].blocks).toEqual([{ type: 'text', text: 'updated' }])
    expect(mergeAgentChatSnapshot(result, snap({ cursor: 2, messages: [] })).messages).toEqual(
      result.messages
    )
  })
  it('replaces old session data on reset, session change or cursor rollback', () => {
    for (const next of [
      snap({ reset: true, messages: [] }),
      snap({ session_id: 'new', messages: [] }),
      snap({ cursor: 0, messages: [] })
    ]) {
      expect(mergeAgentChatSnapshot(snap(), next)).toBe(next)
    }
  })
  it('loads history and polls browser incrementals with the acknowledged cursor', async () => {
    mocks.snapshot
      .mockResolvedValueOnce(snap())
      .mockResolvedValue(snap({ cursor: 2, messages: [message('m2')] }))
    const { result, unmount } = renderHook(() => useTerminalAgentChat('pty'))
    await act(async () => {})
    expect(mocks.snapshot).toHaveBeenLastCalledWith('pty', undefined)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000)
    })
    expect(mocks.snapshot).toHaveBeenLastCalledWith('pty', 1)
    expect(result.current.snapshot?.messages.map((item) => item.id)).toEqual(['m1', 'm2'])
    unmount()
    await vi.advanceTimersByTimeAsync(4000)
    expect(mocks.snapshot).toHaveBeenCalledTimes(2)
  })
  it('uses metadata push only for the matching terminal and unsubscribes', async () => {
    mocks.desktop = true
    mocks.snapshot.mockResolvedValue(snap())
    const off = vi.fn()
    mocks.listen.mockResolvedValue(off)
    const { unmount } = renderHook(() => useTerminalAgentChat('pty'))
    await act(async () => {})
    const callback = mocks.listen.mock.calls[0][0]
    await act(async () => callback({ terminal_id: 'other', cursor: 2 }))
    expect(mocks.snapshot).toHaveBeenCalledTimes(1)
    await act(async () => callback({ terminal_id: 'pty', cursor: 2 }))
    expect(mocks.snapshot).toHaveBeenCalledTimes(2)
    unmount()
    expect(off).toHaveBeenCalledOnce()
  })
  it('exposes parse failures as degradation and recovers without a spinner', async () => {
    mocks.snapshot.mockRejectedValueOnce(new Error('unsupported journal')).mockResolvedValue(snap())
    const { result } = renderHook(() => useTerminalAgentChat('pty'))
    await act(async () => {})
    expect(result.current.error).toBe('unsupported journal')
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000)
    })
    expect(result.current.error).toBeNull()
    expect(result.current.snapshot?.state).toBe('ready')
  })
})
