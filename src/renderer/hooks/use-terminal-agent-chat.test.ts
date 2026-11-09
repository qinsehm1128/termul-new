import type { AgentChatSnapshot } from '@shared/types/agent-chat.types'
import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ChatMessage } from '../features/agent-session/stores/acp-store'
import type { ToolCall } from '../lib/acp-api'
import {
  AGENT_CHAT_MAX_BYTES,
  AGENT_CHAT_MAX_MESSAGES,
  mergeAgentChatSnapshot,
  useTerminalAgentChat
} from './use-terminal-agent-chat'

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

const marker = (
  id: string,
  seq: number,
  toolID: string
): ChatMessage & { seq: number; tool_call_id: string } => ({
  ...message(id, ''),
  seq,
  tool_call_id: toolID
})
const tool = (toolCallId: string, seq: number, output = toolCallId): ToolCall => ({
  toolCallId,
  seq,
  title: toolCallId,
  status: 'completed',
  rawOutput: output
})

describe('terminal agent chat snapshots', () => {
  it('merges realtime updates by message id and retains tool values', () => {
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
  it('merges older-page tools by latest sequence and retains explicit refs beyond the marker seq window', () => {
    const previous = snap({
      cursor: 1000,
      messages: [marker('m10', 10, 'shared')],
      tool_calls: [tool('shared', 100, 'live')]
    })
    const page = snap({
      cursor: 1100,
      older_cursor: 1,
      messages: [marker('m1', 1, 'old'), marker('m2', 2, 'shared')],
      tool_calls: [tool('old', 300, 'old page latest value'), tool('shared', 90, 'stale')]
    })
    let merged = mergeAgentChatSnapshot(previous, page, true)
    expect(merged.tool_calls.map((value) => [value.toolCallId, value.rawOutput])).toEqual([
      ['shared', 'live'],
      ['old', 'old page latest value']
    ])
    expect(merged.cursor).toBe(1000)
    merged = mergeAgentChatSnapshot(merged, snap({ cursor: 1100, messages: [], tool_calls: [] }))
    expect(merged.tool_calls).toHaveLength(2)
    merged = mergeAgentChatSnapshot(
      merged,
      snap({ cursor: 1200, messages: [], tool_calls: [tool('shared', 50, 'stale live')] })
    )
    expect(merged.tool_calls[0].rawOutput).toBe('live')
    merged = mergeAgentChatSnapshot(
      merged,
      { ...page, tool_calls: [tool('shared', 200, 'newer page value')] },
      true
    )
    expect(merged.tool_calls[0].rawOutput).toBe('newer page value')
    expect(
      mergeAgentChatSnapshot(
        merged,
        snap({ cursor: 1300, reset: true, messages: [], tool_calls: [] })
      ).tool_calls
    ).toEqual([])
  })
  it('evicts unreachable tool values when marker messages leave the retained window', () => {
    const markers = Array.from({ length: AGENT_CHAT_MAX_MESSAGES + 1 }, (_, seq) =>
      marker(`m${seq}`, seq, `t${seq}`)
    )
    const tools = markers.map((_, seq) => tool(`t${seq}`, 10000 + seq))
    const recent = mergeAgentChatSnapshot(null, snap({ messages: markers, tool_calls: tools }))
    expect(recent.tool_calls).toHaveLength(AGENT_CHAT_MAX_MESSAGES)
    expect(recent.tool_calls.some((value) => value.toolCallId === 't0')).toBe(false)
    expect(recent.tool_calls.at(-1)?.toolCallId).toBe(`t${AGENT_CHAT_MAX_MESSAGES}`)
    const older = mergeAgentChatSnapshot(
      recent,
      snap({ messages: [marker('m0', 0, 't0')], tool_calls: [tools[0]], older_cursor: null }),
      true
    )
    expect(older.messages[0].id).toBe('m0')
    expect(
      older.tool_calls.some((value) => value.toolCallId === `t${AGENT_CHAT_MAX_MESSAGES}`)
    ).toBe(false)
    expect(older.tool_calls.some((value) => value.toolCallId === 't0')).toBe(true)
  })
  it('bounds combined message/tool bytes while retaining complete oversized marker/tool pairs', () => {
    const payload = 'x'.repeat(AGENT_CHAT_MAX_BYTES / 2 + 1024)
    const input = snap({
      messages: [marker('old', 1, 'old'), marker('new', 2, 'new')],
      tool_calls: [tool('old', 100, payload), tool('new', 200, payload)]
    })
    const recent = mergeAgentChatSnapshot(null, input)
    expect(recent.messages.map((value) => value.id)).toEqual(['new'])
    expect(recent.tool_calls.map((value) => value.toolCallId)).toEqual(['new'])
    expect(recent.older_cursor).toBe(2)
    const older = mergeAgentChatSnapshot(
      recent,
      { ...input, messages: [input.messages[0]], tool_calls: [input.tool_calls[0]] },
      true
    )
    expect(older.messages.map((value) => value.id)).toEqual(['old'])
    expect(older.tool_calls[0].rawOutput).toBe(payload)
    const oversized = 'z'.repeat(AGENT_CHAT_MAX_BYTES + 1)
    const large = mergeAgentChatSnapshot(
      null,
      snap({
        messages: [marker('large', 1, 'large')],
        tool_calls: [tool('large', 1000, oversized)]
      })
    )
    expect(large.messages).toHaveLength(1)
    expect(large.tool_calls[0].rawOutput).toBe(oversized)
  })
  it('bounds legacy tool-only snapshots and uses seq-window compatibility without marker metadata', () => {
    const tools = Array.from({ length: AGENT_CHAT_MAX_MESSAGES + 2 }, (_, seq) =>
      tool(`t${seq}`, seq)
    )
    const bounded = mergeAgentChatSnapshot(null, snap({ messages: [], tool_calls: tools }))
    expect(bounded.tool_calls).toHaveLength(AGENT_CHAT_MAX_MESSAGES)
    const legacy = mergeAgentChatSnapshot(
      null,
      snap({
        messages: [
          { ...message('first'), seq: 2 },
          { ...message('last'), seq: 4 }
        ],
        tool_calls: [tool('outside', 1), tool('inside', 3)]
      })
    )
    expect(legacy.tool_calls.map((value) => value.toolCallId)).toEqual(['inside'])
  })
  it('replaces old session data only on reset or session change', () => {
    for (const next of [
      snap({ reset: true, messages: [] }),
      snap({ session_id: 'new', messages: [] })
    ]) {
      expect(mergeAgentChatSnapshot(snap(), next)).toBe(next)
    }
  })
  it('ignores stale lower live cursors instead of resetting history', () => {
    const previous = snap({ cursor: 10, older_cursor: 2 })
    expect(mergeAgentChatSnapshot(previous, snap({ cursor: 3, messages: [] }))).toBe(previous)
  })
  it('prepends older pages, deduplicates overlap, and preserves live cursor and newer content', () => {
    const previous = snap({ cursor: 100, older_cursor: 10, messages: [message('m10', 'new')] })
    const merged = mergeAgentChatSnapshot(
      previous,
      snap({ cursor: 123, older_cursor: 1, messages: [message('m1'), message('m10', 'old')] }),
      true
    )
    expect(merged.messages.map((item) => item.id)).toEqual(['m1', 'm10'])
    expect(merged.messages[1].blocks).toEqual([{ type: 'text', text: 'new' }])
    expect(merged.cursor).toBe(100)
    expect(merged.older_cursor).toBe(1)
    const live = mergeAgentChatSnapshot(
      merged,
      snap({ cursor: 101, older_cursor: 99, messages: [message('m11')] })
    )
    expect(live.older_cursor).toBe(1)
    expect(live.messages.map((item) => item.id)).toEqual(['m1', 'm10', 'm11'])
    expect(mergeAgentChatSnapshot(live, snap({ session_id: 's2' }), true)).toBe(live)
  })
  it('can traverse beyond the retention cap by evicting newest whole records when browsing', () => {
    const recent = Array.from({ length: AGENT_CHAT_MAX_MESSAGES }, (_, i) => ({
      ...message(`m${i + 100}`),
      seq: i + 100
    }))
    let retained = snap({ messages: recent, cursor: 5000, older_cursor: 100 })
    retained = mergeAgentChatSnapshot(
      retained,
      snap({ cursor: 9999, older_cursor: 1, messages: [message('m1')] }),
      true
    )
    expect(retained.messages).toHaveLength(AGENT_CHAT_MAX_MESSAGES)
    expect(retained.messages[0].id).toBe('m1')
    expect(retained.messages.at(-1)?.id).toBe(`m${AGENT_CHAT_MAX_MESSAGES + 98}`)
    expect(retained.older_cursor).toBe(1)
    expect(retained.cursor).toBe(5000)
    retained = mergeAgentChatSnapshot(
      retained,
      snap({ cursor: 9999, older_cursor: null, messages: [message('m0')] }),
      true
    )
    expect(retained.messages[0].id).toBe('m0')
    expect(retained.older_cursor).toBeNull()
  })
  it('bounds retained full records and leaves a disk reload boundary', () => {
    const messages = Array.from({ length: AGENT_CHAT_MAX_MESSAGES + 2 }, (_, seq) => ({
      ...message(`m${seq}`),
      seq
    }))
    const merged = mergeAgentChatSnapshot(null, snap({ messages }))
    expect(merged.messages).toHaveLength(AGENT_CHAT_MAX_MESSAGES)
    expect(merged.older_cursor).toBe(2)
    const text = 'x'.repeat(17 * 1024 * 1024)
    const oversized = mergeAgentChatSnapshot(null, snap({ messages: [message('large', text)] }))
    expect(oversized.messages[0].blocks).toEqual([{ type: 'text', text }])
  })
  it('loads older pages without replacing the polling cursor and clears history on session reset', async () => {
    mocks.snapshot
      .mockResolvedValueOnce(snap({ cursor: 100, older_cursor: 10, messages: [message('m10')] }))
      .mockResolvedValueOnce(snap({ cursor: 110, older_cursor: 1, messages: [message('m1')] }))
      .mockResolvedValueOnce(snap({ cursor: 101, messages: [message('m11')] }))
      .mockResolvedValueOnce(
        snap({ cursor: 200, reset: true, session_id: 's2', older_cursor: null, messages: [] })
      )
    const { result } = renderHook(() => useTerminalAgentChat('pty'))
    await act(async () => {})
    await act(async () => {
      await result.current.loadOlder()
    })
    expect(mocks.snapshot).toHaveBeenLastCalledWith('pty', undefined, 10)
    expect(result.current.snapshot?.cursor).toBe(100)
    await act(async () => {
      await result.current.refresh()
    })
    expect(mocks.snapshot).toHaveBeenCalledTimes(2)
    expect(result.current.browsingOlder).toBe(true)
    expect(result.current.snapshot?.older_cursor).toBe(1)
    expect(result.current.snapshot?.messages.map((item) => item.id)).toEqual(['m1', 'm10'])
    await act(async () => {
      await result.current.returnLatest()
    })
    expect(mocks.snapshot).toHaveBeenLastCalledWith('pty')
    expect(result.current.browsingOlder).toBe(false)
    expect(result.current.snapshot?.messages.map((item) => item.id)).toEqual(['m11'])
    await act(async () => {
      await result.current.refresh()
    })
    expect(result.current.snapshot?.messages).toEqual([])
    expect(result.current.snapshot?.older_cursor).toBeNull()
  })
  it('adopts a same-session reset-fence older response directly and resumes live polling', async () => {
    mocks.snapshot
      .mockResolvedValueOnce(
        snap({
          cursor: 100,
          older_cursor: 10,
          messages: [marker('old', 10, 'old')],
          tool_calls: [tool('old', 50)]
        })
      )
      .mockResolvedValueOnce(
        snap({
          cursor: 300,
          reset: true,
          older_cursor: 200,
          messages: [marker('new', 200, 'new')],
          tool_calls: [tool('new', 250)]
        })
      )
      .mockResolvedValueOnce(snap({ cursor: 301, messages: [], tool_calls: [] }))
    const { result } = renderHook(() => useTerminalAgentChat('pty'))
    await act(async () => {})
    await act(async () => {
      await result.current.loadOlder()
    })
    expect(mocks.snapshot).toHaveBeenCalledTimes(2)
    expect(mocks.snapshot).toHaveBeenLastCalledWith('pty', undefined, 10)
    expect(result.current.snapshot?.session_id).toBe('s1')
    expect(result.current.snapshot?.cursor).toBe(300)
    expect(result.current.snapshot?.older_cursor).toBe(200)
    expect(result.current.snapshot?.messages.map((value) => value.id)).toEqual(['new'])
    expect(result.current.snapshot?.tool_calls.map((value) => value.toolCallId)).toEqual(['new'])
    expect(result.current.browsingOlder).toBe(false)
    await act(async () => {
      await result.current.refresh()
    })
    expect(mocks.snapshot).toHaveBeenLastCalledWith('pty', 300)
    expect(result.current.snapshot?.cursor).toBe(301)
    expect(result.current.snapshot?.tool_calls.map((value) => value.toolCallId)).toEqual(['new'])
  })
  it('discards an older page arriving after an explicit same-session reset', async () => {
    let finishPage!: (page: Snapshot) => void
    mocks.snapshot
      .mockResolvedValueOnce(snap({ cursor: 10, older_cursor: 5 }))
      .mockImplementationOnce(
        () =>
          new Promise<Snapshot>((resolve) => {
            finishPage = resolve
          })
      )
      .mockResolvedValueOnce(
        snap({ cursor: 11, reset: true, messages: [message('new')], older_cursor: null })
      )
    const { result } = renderHook(() => useTerminalAgentChat('pty'))
    await act(async () => {})
    let request!: Promise<void>
    await act(async () => {
      request = result.current.loadOlder()
    })
    await act(async () => {
      await result.current.returnLatest()
    })
    await act(async () => {
      finishPage(snap({ cursor: 10, messages: [message('old')] }))
      await request
    })
    expect(result.current.snapshot?.messages.map((item) => item.id)).toEqual(['new'])
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
