import type { ConversationRecordV2 } from '@shared/types/conversation.types'
import { act, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ChatRoute } from '@/components/ChatRoute'
import { conversationApi } from '@/lib/conversation-api'
import { useAcpStore } from '@/stores/acp-store'
import { useConversationStore } from '@/stores/conversation-store'
import { useWorkspaceStore } from '@/stores/workspace-store'
import { ConversationRoute } from './ConversationRoute'

const {
  mockLoadSessionWorkspace,
  mockAddAgentChatTab,
  mockOpenHistorySession,
  mockLoadSessionIndex
} = vi.hoisted(() => ({
  mockLoadSessionWorkspace: vi.fn(),
  mockAddAgentChatTab: vi.fn(),
  mockOpenHistorySession: vi.fn(),
  mockLoadSessionIndex: vi.fn()
}))

vi.mock('@/lib/conversation-api', () => ({
  conversationApi: {
    openConversation: vi.fn(),
    resolveLegacyConversationId: vi.fn()
  }
}))

vi.mock('@/hooks/use-session-workspace-sync', () => ({
  loadSessionWorkspace: mockLoadSessionWorkspace
}))

vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn() }))

const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
const secondConversationId = '028f7a1c-1b4d-7c8a-9f01-0123456789ab'

const conversation: ConversationRecordV2 = {
  schemaVersion: 2,
  conversationId,
  createdAtUtc: '2026-08-15T09:45:15.123Z',
  creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
  workspaceCwd: '/workspace/conversation',
  executionTarget: { kind: 'workspace' },
  projectAttachment: null,
  lifecycleState: 'ready',
  lastSeq: 2,
  createdBy: 'se-manager'
}

const secondConversation: ConversationRecordV2 = {
  ...conversation,
  conversationId: secondConversationId,
  workspaceCwd: '/workspace/second-conversation',
  lastSeq: 4
}

function deferred<T>(): {
  promise: Promise<T>
  resolve: (value: T) => void
} {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

beforeEach(() => {
  vi.clearAllMocks()
  useConversationStore.getState().reset()
  mockLoadSessionWorkspace.mockResolvedValue(true)
  mockOpenHistorySession.mockResolvedValue(undefined)
  mockLoadSessionIndex.mockResolvedValue(undefined)
  useAcpStore.setState({
    sessions: {},
    activeSessionId: null,
    sessionIndex: [
      {
        id: 'opaque-agent-session',
        conversationId,
        agentId: 'agent-1',
        title: 'Canonical chat',
        cwd: '/workspace/conversation',
        projectId: '',
        createdAt: 1,
        lastActivityAt: 2,
        messageCount: 0,
        status: 'closed'
      }
    ],
    openHistorySession: mockOpenHistorySession,
    loadSessionIndex: mockLoadSessionIndex
  })
  useWorkspaceStore.setState({ addAgentChatTab: mockAddAgentChatTab })
})

function renderCanonical(id = conversationId): void {
  render(
    <MemoryRouter initialEntries={[`/c/${id}`]}>
      <Routes>
        <Route path="/c/:conversationId" element={<ConversationRoute />} />
      </Routes>
    </MemoryRouter>
  )
}

describe('ConversationRoute canonical open', () => {
  it('opens by ConversationId, restores workspace, and uses the opaque binding only internally', async () => {
    vi.mocked(conversationApi.openConversation).mockResolvedValue({
      success: true,
      data: {
        conversation,
        workspace: { status: 'missing', conversationId }
      }
    })

    renderCanonical()

    await waitFor(() => {
      expect(conversationApi.openConversation).toHaveBeenCalledWith(conversationId)
      expect(mockLoadSessionWorkspace).toHaveBeenCalledWith(conversationId, expect.any(Function))
      expect(mockOpenHistorySession).toHaveBeenCalledWith('opaque-agent-session')
      expect(mockAddAgentChatTab).toHaveBeenCalledWith(conversationId, undefined, false)
    })
    expect(useConversationStore.getState().activeConversationId).toBe(conversationId)
  })

  it('suppresses a late A open after B has installed its workspace, binding, and tab', async () => {
    const first = deferred<Awaited<ReturnType<typeof conversationApi.openConversation>>>()
    const second = deferred<Awaited<ReturnType<typeof conversationApi.openConversation>>>()
    vi.mocked(conversationApi.openConversation).mockImplementation((id) =>
      id === conversationId ? first.promise : second.promise
    )
    useAcpStore.setState({
      sessions: {},
      sessionIndex: [
        {
          id: 'session-a',
          conversationId,
          agentId: 'agent-a',
          title: 'A',
          cwd: conversation.workspaceCwd,
          projectId: '',
          createdAt: 1,
          lastActivityAt: 2,
          messageCount: 0,
          status: 'closed'
        },
        {
          id: 'session-b',
          conversationId: secondConversationId,
          agentId: 'agent-b',
          title: 'B',
          cwd: secondConversation.workspaceCwd,
          projectId: '',
          createdAt: 3,
          lastActivityAt: 4,
          messageCount: 0,
          status: 'closed'
        }
      ],
      activeSessionId: null,
      openHistorySession: mockOpenHistorySession,
      loadSessionIndex: mockLoadSessionIndex
    })
    mockLoadSessionWorkspace.mockImplementation(async (id, isCurrent: () => boolean) => {
      if (!isCurrent()) return false
      useWorkspaceStore.setState({
        root: { type: 'leaf', id: `workspace-${id}`, tabs: [], activeTabId: null },
        activePaneId: `workspace-${id}`
      })
      return isCurrent()
    })

    const view = render(<ConversationRoute conversationId={conversationId} />)
    await waitFor(() =>
      expect(conversationApi.openConversation).toHaveBeenCalledWith(conversationId)
    )
    view.rerender(<ConversationRoute conversationId={secondConversationId} />)
    await waitFor(() =>
      expect(conversationApi.openConversation).toHaveBeenCalledWith(secondConversationId)
    )

    await act(async () => {
      second.resolve({
        success: true,
        data: {
          conversation: secondConversation,
          workspace: { status: 'missing', conversationId: secondConversationId }
        }
      })
    })
    await waitFor(() => {
      expect(useConversationStore.getState().activeConversationId).toBe(secondConversationId)
      expect(useAcpStore.getState().activeSessionId).toBe('session-b')
      expect(useWorkspaceStore.getState().root.id).toBe(`workspace-${secondConversationId}`)
      expect(mockAddAgentChatTab).toHaveBeenCalledWith(secondConversationId, undefined, false)
    })

    await act(async () => {
      first.resolve({
        success: true,
        data: {
          conversation,
          workspace: { status: 'missing', conversationId }
        }
      })
      await first.promise
    })

    expect(useConversationStore.getState().activeConversationId).toBe(secondConversationId)
    expect(useAcpStore.getState().activeSessionId).toBe('session-b')
    expect(useWorkspaceStore.getState().root.id).toBe(`workspace-${secondConversationId}`)
    expect(mockLoadSessionWorkspace).toHaveBeenCalledTimes(1)
    expect(mockAddAgentChatTab).not.toHaveBeenCalledWith(conversationId, undefined, false)
  })

  it('suppresses a late A binding open after B becomes the active binding and tab', async () => {
    const staleBinding = deferred<void>()
    vi.mocked(conversationApi.openConversation).mockImplementation(async (id) => ({
      success: true,
      data: {
        conversation: id === conversationId ? conversation : secondConversation,
        workspace: { status: 'missing', conversationId: id }
      }
    }))
    mockOpenHistorySession.mockImplementation((sessionId) =>
      sessionId === 'session-a' ? staleBinding.promise : Promise.resolve()
    )
    useAcpStore.setState({
      sessions: {},
      sessionIndex: [
        {
          id: 'session-a',
          conversationId,
          agentId: 'agent-a',
          title: 'A',
          cwd: conversation.workspaceCwd,
          projectId: '',
          createdAt: 1,
          lastActivityAt: 2,
          messageCount: 0,
          status: 'closed'
        },
        {
          id: 'session-b',
          conversationId: secondConversationId,
          agentId: 'agent-b',
          title: 'B',
          cwd: secondConversation.workspaceCwd,
          projectId: '',
          createdAt: 3,
          lastActivityAt: 4,
          messageCount: 0,
          status: 'closed'
        }
      ],
      activeSessionId: null,
      openHistorySession: mockOpenHistorySession,
      loadSessionIndex: mockLoadSessionIndex
    })
    mockLoadSessionWorkspace.mockImplementation(async (id, isCurrent: () => boolean) => {
      if (!isCurrent()) return false
      useWorkspaceStore.setState({
        root: { type: 'leaf', id: `binding-workspace-${id}`, tabs: [], activeTabId: null },
        activePaneId: `binding-workspace-${id}`
      })
      return isCurrent()
    })

    const view = render(<ConversationRoute conversationId={conversationId} />)
    await waitFor(() => expect(mockOpenHistorySession).toHaveBeenCalledWith('session-a'))

    view.rerender(<ConversationRoute conversationId={secondConversationId} />)
    await waitFor(() => {
      expect(useConversationStore.getState().activeConversationId).toBe(secondConversationId)
      expect(useAcpStore.getState().activeSessionId).toBe('session-b')
      expect(mockAddAgentChatTab).toHaveBeenCalledWith(secondConversationId, undefined, false)
    })

    await act(async () => {
      staleBinding.resolve()
      await staleBinding.promise
    })

    expect(useConversationStore.getState().activeConversationId).toBe(secondConversationId)
    expect(useAcpStore.getState().activeSessionId).toBe('session-b')
    expect(useWorkspaceStore.getState().root.id).toBe(`binding-workspace-${secondConversationId}`)
    expect(mockAddAgentChatTab).not.toHaveBeenCalledWith(conversationId, undefined, false)
    expect(mockLoadSessionWorkspace.mock.calls.map(([id]) => id)).toEqual([
      conversationId,
      secondConversationId
    ])
  })

  it('cancels the latest activation epoch on unmount before a workspace response can apply', async () => {
    const workspaceResponse = deferred<void>()
    vi.mocked(conversationApi.openConversation).mockResolvedValue({
      success: true,
      data: {
        conversation,
        workspace: { status: 'missing', conversationId }
      }
    })
    const originalRoot = useWorkspaceStore.getState().root
    mockLoadSessionWorkspace.mockImplementation(async (id, isCurrent: () => boolean) => {
      await workspaceResponse.promise
      if (!isCurrent()) return false
      useWorkspaceStore.setState({
        root: { type: 'leaf', id: `unmounted-${id}`, tabs: [], activeTabId: null },
        activePaneId: `unmounted-${id}`
      })
      return true
    })

    const view = render(<ConversationRoute conversationId={conversationId} />)
    await waitFor(() => expect(mockLoadSessionWorkspace).toHaveBeenCalledTimes(1))
    view.unmount()

    await act(async () => {
      workspaceResponse.resolve()
      await workspaceResponse.promise
    })

    expect(useWorkspaceStore.getState().root).toBe(originalRoot)
    expect(mockOpenHistorySession).not.toHaveBeenCalled()
    expect(mockAddAgentChatTab).not.toHaveBeenCalled()
  })

  it('renders a stable not-found error and never stores the route value as an ACP id', async () => {
    vi.mocked(conversationApi.openConversation).mockResolvedValue({
      success: false,
      code: 'CONVERSATION_NOT_FOUND',
      error: 'missing'
    })

    renderCanonical()

    expect(await screen.findByRole('alert')).toHaveAttribute(
      'data-error-code',
      'CONVERSATION_NOT_FOUND'
    )
    expect(mockOpenHistorySession).not.toHaveBeenCalled()
  })

  it('renders recovery-required with an explicit retry action', async () => {
    vi.mocked(conversationApi.openConversation).mockResolvedValue({
      success: false,
      code: 'CONVERSATION_RECOVERY_REQUIRED',
      error: 'recover'
    })

    renderCanonical()

    expect(await screen.findByRole('alert')).toHaveAttribute(
      'data-error-code',
      'CONVERSATION_RECOVERY_REQUIRED'
    )
    expect(screen.getByRole('button', { name: 'Try Again' })).toBeInTheDocument()
  })

  it('rejects a non-canonical id before calling the facade', async () => {
    renderCanonical('opaque-agent-session')

    expect(await screen.findByRole('alert')).toHaveAttribute(
      'data-error-code',
      'CONVERSATION_INVALID_ID'
    )
    expect(conversationApi.openConversation).not.toHaveBeenCalled()
  })
})

describe('ChatRoute legacy redirect', () => {
  it('uses the typed read-only resolver and replace-redirects to the canonical route', async () => {
    vi.mocked(conversationApi.resolveLegacyConversationId).mockResolvedValue({
      success: true,
      data: { conversationId, canonicalRoute: `#/c/${conversationId}` }
    })

    render(
      <MemoryRouter initialEntries={['/legacy/history/opaque-history']}>
        <Routes>
          <Route
            path="/legacy/history/:legacyValue"
            element={<ChatRoute sourceKind="legacyChatHistoryId" />}
          />
          <Route path="/c/:conversationId" element={<div>canonical destination</div>} />
        </Routes>
      </MemoryRouter>
    )

    expect(await screen.findByText('canonical destination')).toBeInTheDocument()
    expect(conversationApi.resolveLegacyConversationId).toHaveBeenCalledWith({
      sourceKind: 'legacyChatHistoryId',
      value: 'opaque-history'
    })
  })

  it('resolves a UUID-shaped legacy value instead of treating it as a canonical route key', async () => {
    vi.mocked(conversationApi.resolveLegacyConversationId).mockResolvedValue({
      success: true,
      data: { conversationId, canonicalRoute: `#/c/${conversationId}` }
    })

    render(
      <MemoryRouter initialEntries={[`/legacy/session/${conversationId}`]}>
        <Routes>
          <Route
            path="/legacy/session/:legacyValue"
            element={<ChatRoute sourceKind="legacyAgentSessionId" />}
          />
          <Route path="/c/:conversationId" element={<div>canonical destination</div>} />
        </Routes>
      </MemoryRouter>
    )

    expect(await screen.findByText('canonical destination')).toBeInTheDocument()
    expect(conversationApi.resolveLegacyConversationId).toHaveBeenCalledWith({
      sourceKind: 'legacyAgentSessionId',
      value: conversationId
    })
    expect(useConversationStore.getState().activeConversationId).toBeNull()
  })

  it.each([
    'CONVERSATION_NOT_FOUND',
    'LEGACY_ID_AMBIGUOUS'
  ])('renders stable accessible %s resolver errors', async (code) => {
    vi.mocked(conversationApi.resolveLegacyConversationId).mockResolvedValue({
      success: false,
      code,
      error: code
    })

    render(
      <MemoryRouter initialEntries={['/legacy/storage/legacy-key']}>
        <Routes>
          <Route
            path="/legacy/storage/:legacyValue"
            element={<ChatRoute sourceKind="legacyStorageKey" />}
          />
        </Routes>
      </MemoryRouter>
    )

    expect(await screen.findByRole('alert')).toHaveAttribute('data-error-code', code)
    expect(useConversationStore.getState().summariesById['legacy-key']).toBeUndefined()
  })
})
