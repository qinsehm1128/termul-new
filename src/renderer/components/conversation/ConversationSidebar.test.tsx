import type { ConversationRecordV2 } from '@shared/types/conversation.types'
import { fireEvent, render, screen, within } from '@testing-library/react'
import { MemoryRouter, useLocation } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { i18n } from '@/i18n'
import { useAcpStore } from '@/stores/acp-store'
import { useConversationStore } from '@/stores/conversation-store'
import { useProjectStore } from '@/stores/project-store'
import { ConversationSidebar } from './ConversationSidebar'

vi.mock('@/components/chat/ChatHistoryEntryRow', () => ({
  ConversationLifecycleActions: ({ title }: { title: string }) => (
    <button type="button" aria-label={`Lifecycle ${title}`} />
  )
}))

const projectlessId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
const attachedId = '028f7a1c-1b4d-7c8a-9f01-0123456789ab'

function summary(
  conversationId: string,
  workspaceCwd: string,
  projectId: string | null
): ConversationRecordV2 {
  return {
    schemaVersion: 2,
    conversationId,
    createdAtUtc: '2026-08-15T09:45:15.123Z',
    creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
    workspaceCwd,
    executionTarget: { kind: 'workspace' },
    projectAttachment: projectId
      ? {
          schemaVersion: 1,
          projectId,
          attachedAtUtc: '2026-08-15T09:45:15.123Z',
          projectPathSnapshot: '/projects/demo',
          worktreePath: null,
          worktreeBranch: null
        }
      : null,
    lifecycleState: 'ready',
    lastSeq: 3,
    createdBy: 'termul'
  }
}

beforeEach(() => {
  useAcpStore.setState({
    sessions: {},
    sessionIndex: [],
    pendingPermissions: {},
    pendingQuestions: {}
  })
  useConversationStore.getState().reset()
  useConversationStore
    .getState()
    .replaceSummaries([
      summary(projectlessId, '/workspaces/Projectless chat', null),
      summary(attachedId, '/workspaces/Attached chat', 'p1')
    ])
  useConversationStore.getState().setRecoveryItems([
    {
      recoveryId: 'a'.repeat(64),
      kind: 'ambiguous_workspace_manifest',
      severity: 'warning',
      sourcePaths: ['legacy/shared.json'],
      conversationIds: [projectlessId],
      sourceSha256: ['b'.repeat(64)],
      candidateFacts: [],
      provenance: [],
      status: 'unresolved',
      suggestedActions: ['inspect'],
      revision: 1,
      associationDecisions: []
    }
  ])
  useProjectStore.setState({
    projects: [{ id: 'p1', name: 'Demo project', color: 'blue' }],
    activeProjectId: ''
  })
})

afterEach(async () => {
  if (i18n.language !== 'en') await i18n.changeLanguage('en')
})

function LocationProbe(): React.JSX.Element {
  return <output data-testid="location-probe">{useLocation().pathname}</output>
}

function renderSidebar(onNewChat = vi.fn()): void {
  render(
    <MemoryRouter>
      <ConversationSidebar onNewChat={onNewChat} />
      <LocationProbe />
    </MemoryRouter>
  )
}

describe('ConversationSidebar global navigation', () => {
  it('renders project-less and attached Conversations together with context labels', () => {
    renderSidebar()

    expect(screen.getByText('Projectless chat')).toBeInTheDocument()
    expect(screen.getByText('Attached chat')).toBeInTheDocument()
    expect(screen.getByRole('option', { name: 'No project' })).toBeInTheDocument()
    expect(screen.getByRole('option', { name: 'Demo project' })).toBeInTheDocument()
    expect(screen.getAllByText('Idle')).toHaveLength(2)

    const attached = screen.getByText('Attached chat').closest('[data-conversation-id]')
    expect(attached).toBeTruthy()
    fireEvent.click(within(attached as HTMLElement).getByLabelText('Show conversation details'))
    expect(within(attached as HTMLElement).getByText('Demo project')).toBeInTheDocument()
  })

  it('searches the full global list and clears on Escape', () => {
    renderSidebar()
    const search = screen.getByLabelText('Search conversations')

    fireEvent.change(search, { target: { value: 'projectless' } })
    expect(screen.getByText('Projectless chat')).toBeInTheDocument()
    expect(screen.queryByText('Attached chat')).not.toBeInTheDocument()

    fireEvent.keyDown(search, { key: 'Escape' })
    expect(screen.getByText('Attached chat')).toBeInTheDocument()
  })

  it('filters by optional project without losing the project-less canonical row', () => {
    renderSidebar()
    fireEvent.change(screen.getByLabelText('Filter conversations by project'), {
      target: { value: 'p1' }
    })
    expect(screen.getByText('Attached chat')).toBeInTheDocument()
    expect(screen.queryByText('Projectless chat')).not.toBeInTheDocument()
    fireEvent.click(screen.getByText('Attached chat'))
    expect(screen.getByTestId('location-probe')).toHaveTextContent(`/c/${attachedId}`)

    fireEvent.change(screen.getByLabelText('Filter conversations by project'), {
      target: { value: 'all' }
    })
    expect(screen.getByText('Projectless chat')).toBeInTheDocument()
    fireEvent.click(screen.getByText('Projectless chat'))
    expect(screen.getByTestId('location-probe')).toHaveTextContent(`/c/${projectlessId}`)
  })

  it('shows recovery badges and lifecycle menus from ConversationId keyed state', () => {
    renderSidebar()

    expect(screen.getByLabelText('1 recovery item')).toBeInTheDocument()
    expect(screen.getByLabelText('Lifecycle Projectless chat')).toBeInTheDocument()
  })

  it('exposes the global New Chat action', () => {
    const onNewChat = vi.fn()
    renderSidebar(onNewChat)

    fireEvent.click(screen.getByLabelText('New Chat'))
    expect(onNewChat).toHaveBeenCalledTimes(1)
  })

  it('marks the active Conversation with the shared list row, not a 28px chip ring', () => {
    useConversationStore.getState().setActiveConversationId(projectlessId)
    renderSidebar()

    const sidebar = screen.getByText('Conversations').closest('aside')
    expect(sidebar).toHaveClass('w-full')

    const row = screen.getByText('Projectless chat').closest('[data-conversation-id]')
    const active = row?.querySelector('[data-list-row]')
    expect(active).toHaveAttribute('data-active')
    expect(active).toHaveClass('bg-sidebar-accent')
    expect(active).not.toHaveClass('min-h-7', 'ring-1')

    const idle = screen
      .getByText('Attached chat')
      .closest('[data-conversation-id]')
      ?.querySelector('[data-list-row]')
    expect(idle).not.toHaveAttribute('data-active')
    expect(idle).not.toHaveClass('bg-sidebar-accent')
    expect(idle).toHaveTextContent('rev 3')
    expect(row).toHaveTextContent('Idle')
  })

  it('renders the same global navigation controls from the Chinese locale', async () => {
    await i18n.changeLanguage('zh-CN')
    renderSidebar()

    expect(screen.getByText('会话')).toBeInTheDocument()
    expect(screen.getByLabelText('搜索会话')).toBeInTheDocument()
    expect(screen.getByLabelText('按项目筛选会话')).toBeInTheDocument()
    expect(screen.getByLabelText('新建聊天')).toBeInTheDocument()
    expect(screen.getAllByText('空闲')).toHaveLength(2)
  })
})
