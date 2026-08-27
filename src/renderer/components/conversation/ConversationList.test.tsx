import type { ConversationRecordV2 } from '@shared/types/conversation.types'
import { fireEvent, render, screen, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useAcpStore } from '@/stores/acp-store'
import { useConversationStore } from '@/stores/conversation-store'
import { useProjectStore } from '@/stores/project-store'
import { ConversationList } from './ConversationList'

vi.mock('@/components/chat/ChatHistoryEntryRow', () => ({
  ConversationLifecycleActions: ({ title }: { title: string }) => (
    <button type="button" aria-label={`Lifecycle ${title}`} />
  )
}))

const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'

function summary(overrides: Partial<ConversationRecordV2> = {}): ConversationRecordV2 {
  return {
    schemaVersion: 2,
    conversationId,
    createdAtUtc: '2026-08-15T09:45:15.123Z',
    creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
    workspaceCwd: '/workspaces/termul',
    executionTarget: { kind: 'workspace' },
    projectAttachment: {
      schemaVersion: 1,
      projectId: 'p1',
      attachedAtUtc: '2026-08-15T09:45:15.123Z',
      projectPathSnapshot: '/projects/demo',
      worktreePath: null,
      worktreeBranch: null
    },
    lifecycleState: 'ready',
    lastSeq: 4,
    createdBy: 'termul',
    title: 'Fix the layout',
    ...overrides
  }
}

function renderList(): void {
  render(
    <MemoryRouter>
      <ConversationList />
    </MemoryRouter>
  )
}

beforeEach(() => {
  useAcpStore.setState({
    sessions: {},
    sessionIndex: [],
    pendingPermissions: {},
    pendingQuestions: {}
  })
  useConversationStore.getState().reset()
  useConversationStore.getState().replaceSummaries([summary()])
  useProjectStore.setState({
    projects: [{ id: 'p1', name: 'Demo project', color: 'blue' }],
    activeProjectId: ''
  })
})

describe('ConversationList row language', () => {
  it('keeps a folder preview only when it differs from the title, as path-secondary mono', () => {
    renderList()

    const preview = screen.getByText('termul')
    expect(preview).toHaveClass('font-mono')
    expect(preview).not.toHaveTextContent('last-turn')
    expect(screen.getByText('Fix the layout')).toBeInTheDocument()
  })

  it('uses time · agent · revision in meta and keeps project in expanded details', () => {
    useAcpStore.setState({
      sessions: {
        s1: { conversationId, agentId: 'claude-code', activeTurn: false, status: 'active' }
      },
      sessionIndex: [{ id: 's1', conversationId, agentId: 'claude-code' }]
    })
    renderList()

    const row = screen.getByText('Fix the layout').closest('[data-conversation-id]')
    expect(row).toBeTruthy()
    expect(row).toHaveTextContent('Claude Code')
    expect(row).toHaveTextContent('rev 4')
    expect(within(row as HTMLElement).queryByText('Demo project')).not.toBeInTheDocument()

    fireEvent.click(within(row as HTMLElement).getByLabelText('Show conversation details'))
    expect(within(row as HTMLElement).getByText('Demo project')).toBeInTheDocument()
  })

  it('shows Need you when a pending permission belongs to the conversation', () => {
    useAcpStore.setState({
      sessions: {
        s1: { conversationId, agentId: 'claude-code', activeTurn: true, status: 'active' }
      },
      pendingPermissions: { req: { sessionId: 's1' } }
    })
    renderList()

    const chip = screen.getByText('Need you')
    expect(chip).toHaveAttribute('data-list-row-status', 'need')
  })

  it('shows Working when a live session has an active turn', () => {
    useAcpStore.setState({
      sessions: {
        s1: { conversationId, agentId: 'claude-code', activeTurn: true, status: 'active' }
      }
    })
    renderList()

    expect(screen.getByText('Working')).toHaveAttribute('data-list-row-status', 'working')
  })
})
