import { act, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { TimelineItem } from './chat-timeline'

const { mockLoadOlder, mockClearBackfill } = vi.hoisted(() => ({
  mockLoadOlder: vi.fn(() => Promise.resolve()),
  mockClearBackfill: vi.fn()
}))

vi.mock('@/stores/acp-store', () => ({
  useAcpStore: Object.assign((sel: (s: Record<string, unknown>) => unknown) => sel({}), {
    getState: () => ({
      loadOlderMessages: mockLoadOlder,
      clearSessionBackfill: mockClearBackfill
    })
  }),
  useAgentIdentity: () => ({ name: 'Cursor', templateId: 'cursor' })
}))

import { ChatHistoryLoadingStatus, ChatMessageList, useLoadOlderMessages } from './ChatMessageList'

vi.mock('./ChatMessage', () => ({
  ChatMessage: ({
    message
  }: {
    message: { id: string; blocks: Array<{ type: string; text?: string; name?: string }> }
  }) => (
    <div data-testid={`message-${message.id}`}>
      {message.blocks.map((block) => block.text ?? block.name ?? block.type).join('')}
    </div>
  )
}))

const userItem: TimelineItem = {
  kind: 'message',
  key: 'user-1',
  message: {
    id: 'user-1',
    role: 'user',
    blocks: [{ type: 'text', text: 'Please investigate' }],
    streaming: false,
    timestamp: 1_000,
    seq: 1
  }
}

const streamingAgentItem: TimelineItem = {
  kind: 'message',
  key: 'agent-1',
  message: {
    id: 'agent-1',
    role: 'agent',
    blocks: [{ type: 'text', text: 'Working on it' }],
    streaming: true,
    timestamp: 2_000,
    seq: 3
  }
}

const finalAgentItem: TimelineItem = {
  kind: 'message',
  key: 'agent-final',
  message: {
    id: 'agent-final',
    role: 'agent',
    blocks: [{ type: 'text', text: 'Final response' }],
    streaming: false,
    timestamp: 4_200,
    seq: 4
  }
}

const toolItem: TimelineItem = {
  kind: 'tool',
  key: 'tool-1',
  tool: {
    toolCallId: 'tool-1',
    title: 'Reading files',
    status: 'in_progress',
    timestamp: 1_500,
    seq: 2
  }
}

const completedToolItem: TimelineItem = {
  ...toolItem,
  tool: { ...toolItem.tool, status: 'completed' }
}

const failedToolItem: TimelineItem = {
  kind: 'tool',
  key: 'tool-failed',
  tool: {
    toolCallId: 'tool-failed',
    title: 'Run checks',
    status: 'failed',
    timestamp: 2_000,
    seq: 2
  }
}

const thoughtItem: TimelineItem = {
  kind: 'thought-group',
  key: 'thought-1',
  messages: [
    {
      id: 'thought-1',
      role: 'thought',
      blocks: [{ type: 'text', text: 'Considering options' }],
      streaming: true,
      timestamp: 1_200,
      seq: 2
    }
  ]
}

describe('ChatMessageList', () => {
  beforeEach(() => {
    mockLoadOlder.mockReset().mockResolvedValue(undefined)
    mockClearBackfill.mockReset()
  })

  it.each([
    ['before content arrives', [userItem]],
    ['while a thought streams', [userItem, thoughtItem]],
    ['while a tool runs', [userItem, toolItem]],
    ['while an agent response streams', [userItem, streamingAgentItem]]
  ] satisfies Array<[string, TimelineItem[]]>)('shows open Working activity %s', (_, items) => {
    render(
      <ChatMessageList items={items} sessionId="session-1" agentId="agent-1" showRunningIndicator />
    )

    const trigger = screen.getByRole('button', { name: /Working/ })
    expect(trigger).toHaveAttribute('aria-expanded', 'true')
  })

  it('collapses completed activity while keeping the final response visible', async () => {
    const { rerender } = render(
      <ChatMessageList
        items={[userItem, toolItem, streamingAgentItem]}
        sessionId="session-1"
        agentId="agent-1"
        showRunningIndicator
      />
    )

    rerender(
      <ChatMessageList
        items={[userItem, completedToolItem, streamingAgentItem, finalAgentItem]}
        sessionId="session-1"
        agentId="agent-1"
        showRunningIndicator={false}
      />
    )

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Worked for 3s' })).toHaveAttribute(
        'aria-expanded',
        'false'
      )
    })
    expect(screen.getByText('Final response')).toBeInTheDocument()
  })

  it('allows keyboard-accessible reopening after completion', () => {
    render(
      <ChatMessageList
        items={[userItem, completedToolItem, finalAgentItem]}
        sessionId="session-1"
        agentId="agent-1"
        showRunningIndicator={false}
      />
    )

    const trigger = screen.getByRole('button', { name: 'Worked for 3s' })
    expect(trigger).toHaveAttribute('aria-expanded', 'false')
    trigger.focus()
    // jsdom does not synthesize the click a real browser fires on Enter, so
    // dispatch the key sequence then the keyboard-generated click (detail 0).
    fireEvent.keyDown(trigger, { key: 'Enter' })
    fireEvent.keyUp(trigger, { key: 'Enter' })
    fireEvent.click(trigger, { detail: 0 })
    expect(trigger).toHaveAttribute('aria-expanded', 'true')
  })

  it('keeps attachment responses visible when later text and an empty tail arrive', async () => {
    const attachmentItem: TimelineItem = {
      kind: 'message',
      key: 'agent-attachment',
      message: {
        id: 'agent-attachment',
        role: 'agent',
        blocks: [{ type: 'resource_link', name: 'report.pdf', uri: 'file:///tmp/report.pdf' }],
        streaming: false,
        timestamp: 2_200,
        seq: 3
      }
    }
    const emptyTailItem: TimelineItem = {
      kind: 'message',
      key: 'agent-empty',
      message: {
        id: 'agent-empty',
        role: 'agent',
        blocks: [{ type: 'text', text: '  ' }],
        streaming: false,
        timestamp: 4_300,
        seq: 5
      }
    }

    render(
      <ChatMessageList
        items={[userItem, completedToolItem, attachmentItem, finalAgentItem, emptyTailItem]}
        sessionId="session-1"
        agentId="agent-1"
        showRunningIndicator={false}
      />
    )

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Worked for 3s' })).toHaveAttribute(
        'aria-expanded',
        'false'
      )
    })
    expect(screen.getByTestId('message-agent-attachment')).toHaveTextContent('report.pdf')
    expect(screen.getByTestId('message-agent-final')).toHaveTextContent('Final response')
    expect(screen.queryByTestId('message-agent-empty')).not.toBeInTheDocument()
  })

  it('keeps the live response DOM node stable when later activity arrives', () => {
    const { rerender } = render(
      <ChatMessageList
        items={[userItem, streamingAgentItem]}
        sessionId="session-1"
        agentId="agent-1"
        showRunningIndicator
      />
    )
    const liveNode = screen.getByTestId('message-agent-1')

    rerender(
      <ChatMessageList
        items={[userItem, streamingAgentItem, toolItem]}
        sessionId="session-1"
        agentId="agent-1"
        showRunningIndicator
      />
    )

    expect(screen.getByTestId('message-agent-1')).toBe(liveNode)
    expect(screen.getByText('Reading files')).toBeInTheDocument()
  })

  it('lays out a content stream without edge fades or bubble chrome', () => {
    const { container } = render(
      <ChatMessageList
        items={[userItem, completedToolItem, finalAgentItem]}
        sessionId="session-1"
        agentId="agent-1"
        showRunningIndicator={false}
      />
    )

    expect(container.innerHTML).toContain('max-w-3xl')
    expect(container.querySelector('.chat-timeline-row-user')).toBeInTheDocument()
    expect(container.querySelector('.chat-timeline-row-activity')).toBeInTheDocument()
    expect(container.querySelector('.chat-timeline-row-agent')).toBeInTheDocument()
    expect(container.innerHTML).not.toContain('bg-gradient-to-b')
    expect(container.innerHTML).not.toContain('bg-gradient-to-t')
  })

  it('does not show history loading status on a pinned first paint', () => {
    render(
      <ChatMessageList
        items={[userItem]}
        sessionId="session-1"
        agentId="agent-1"
        showRunningIndicator={false}
      />
    )
    expect(screen.queryByTestId('chat-history-loading')).not.toBeInTheDocument()
    expect(mockLoadOlder).not.toHaveBeenCalled()
  })

  it('aligns the empty thread with CHAT_GUTTER_X / max-w-3xl', () => {
    const { container } = render(
      <ChatMessageList
        items={[]}
        sessionId="session-1"
        agentId="agent-1"
        showRunningIndicator={false}
      />
    )
    expect(screen.getByRole('heading', { name: 'Chat with Cursor' })).toBeInTheDocument()
    expect(container.innerHTML).toContain('max-w-3xl')
    expect(container.innerHTML).toContain('px-3')
    expect(container.innerHTML).toContain('@[400px]:px-5')
  })

  it('collapses a failed tool-only turn but flags it for attention', () => {
    render(
      <ChatMessageList
        items={[userItem, failedToolItem]}
        sessionId="session-1"
        agentId="agent-1"
        showRunningIndicator={false}
      />
    )

    const trigger = screen.getByRole('button', { name: /Worked.*needs attention/ })
    expect(trigger).toHaveAttribute('aria-expanded', 'false')
  })
})

describe('ChatHistoryLoadingStatus', () => {
  it('renders a restrained top-of-thread skeleton aligned to the thread column', () => {
    const { container } = render(<ChatHistoryLoadingStatus />)
    const status = screen.getByRole('status')
    expect(status).toHaveAttribute('aria-live', 'polite')
    expect(status).toHaveTextContent('Loading earlier messages')
    expect(container.innerHTML).toContain('max-w-3xl')
    expect(container.innerHTML).toContain('px-3')
    expect(container.innerHTML).toContain('@[400px]:px-5')
    expect(container.innerHTML).toContain('animate-pulse')
    expect(container.innerHTML).toContain('motion-reduce:animate-none')
    expect(container.innerHTML).not.toContain('sticky')
  })
})

describe('useLoadOlderMessages', () => {
  beforeEach(() => {
    mockLoadOlder.mockReset()
    mockClearBackfill.mockReset()
  })

  it('exposes loading while older history is in flight and restores scroll after', async () => {
    let resolveLoad: (() => void) | undefined
    mockLoadOlder.mockReturnValue(
      new Promise<void>((resolve) => {
        resolveLoad = resolve
      })
    )
    const viewport = {
      scrollHeight: 800,
      clientHeight: 200,
      scrollTop: 40
    } as HTMLDivElement

    const { result } = renderHook(() => useLoadOlderMessages('session-1', 3, 0, viewport, false))

    await waitFor(() => expect(result.current).toBe(true))
    expect(mockLoadOlder).toHaveBeenCalledWith('session-1', 50)

    viewport.scrollHeight = 1200
    await act(async () => {
      resolveLoad?.()
    })
    await waitFor(() => expect(result.current).toBe(false))
    await act(async () => {
      await new Promise<void>((resolve) => {
        requestAnimationFrame(() => resolve())
      })
    })
    expect(viewport.scrollTop).toBe(440)
  })

  it('does not load when pinned to the live edge or the viewport is not scrollable', () => {
    const viewport = {
      scrollHeight: 800,
      clientHeight: 200,
      scrollTop: 0
    } as HTMLDivElement
    renderHook(() => useLoadOlderMessages('session-1', 3, 0, viewport, true))
    expect(mockLoadOlder).not.toHaveBeenCalled()

    const shortViewport = {
      scrollHeight: 200,
      clientHeight: 200,
      scrollTop: 0
    } as HTMLDivElement
    renderHook(() => useLoadOlderMessages('session-1', 3, 0, shortViewport, false))
    expect(mockLoadOlder).not.toHaveBeenCalled()
  })
})
