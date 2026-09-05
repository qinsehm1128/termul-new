import type { ConversationRecordV2 } from '@shared/types/conversation.types'
import { fireEvent, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useConversationStore } from '@/stores/conversation-store'
import { useProjectStore } from '@/stores/project-store'
import { ProjectChatList } from './ProjectChatList'

vi.mock('@/components/chat/ChatHistoryEntryRow', () => ({
  ConversationLifecycleActions: () => null
}))

const one = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
const two = '028f7a1c-1b4d-7c8a-9f01-0123456789ab'
const projectless = '038f7a1c-1b4d-7c8a-9f01-0123456789ab'

function summary(
  conversationId: string,
  projectId: string | null,
  workspaceCwd: string
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
          projectPathSnapshot: `/projects/${projectId}`,
          worktreePath: null,
          worktreeBranch: null
        }
      : null,
    lifecycleState: 'ready',
    lastSeq: 0,
    createdBy: 'se-manager'
  }
}

beforeEach(() => {
  useConversationStore.getState().reset()
  useConversationStore
    .getState()
    .replaceSummaries([
      summary(one, 'p1', '/workspace/one'),
      summary(two, 'p2', '/workspace/two'),
      summary(projectless, null, '/workspace/projectless')
    ])
  useProjectStore.setState({
    projects: [
      { id: 'p1', name: 'One', color: 'blue' },
      { id: 'p2', name: 'Two', color: 'green' }
    ]
  })
})

function renderList(): void {
  render(
    <MemoryRouter>
      <ProjectChatList projectId="p1" />
    </MemoryRouter>
  )
}

describe('ProjectChatList optional projection', () => {
  it('shows only the requested project projection without owning the global list', () => {
    renderList()

    expect(screen.getByText('one')).toBeInTheDocument()
    expect(screen.queryByText('two')).not.toBeInTheDocument()
    expect(screen.queryByText('projectless')).not.toBeInTheDocument()
    expect(useConversationStore.getState().conversationIds).toEqual([one, two, projectless])
  })

  it('uses the ConversationStore search filter', () => {
    renderList()

    fireEvent.change(screen.getByLabelText('Search conversations'), {
      target: { value: 'missing' }
    })
    expect(screen.getByText('No conversations match this view.')).toBeInTheDocument()

    fireEvent.keyDown(screen.getByLabelText('Search conversations'), { key: 'Escape' })
    expect(screen.getByText('one')).toBeInTheDocument()
  })

  it('keeps project-less Conversations globally reachable after project filtering', () => {
    useConversationStore.getState().setProjectFilter('p1')
    renderList()
    expect(screen.getByText('one')).toBeInTheDocument()

    useConversationStore.getState().setProjectFilter(null)
    expect(useConversationStore.getState().conversationIds.includes(projectless)).toBe(true)
  })
})
