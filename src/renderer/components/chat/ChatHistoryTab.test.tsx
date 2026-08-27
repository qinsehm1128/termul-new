import type { ConversationRecordV2 } from '@shared/types/conversation.types'
import { fireEvent, render, screen, within } from '@testing-library/react'
import { MemoryRouter, useLocation } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useAcpStore } from '@/stores/acp-store'
import { useConversationStore } from '@/stores/conversation-store'
import { useProjectStore } from '@/stores/project-store'
import { ChatHistoryTab } from './ChatHistoryTab'

vi.mock('@/components/chat/ChatHistoryEntryRow', () => ({
  ConversationLifecycleActions: ({ title }: { title: string }) => (
    <button type="button" aria-label={`Lifecycle ${title}`} />
  )
}))

function summary(index: number, projectId: string | null = null): ConversationRecordV2 {
  const prefix = (index + 1).toString(16).padStart(2, '0')
  const conversationId = `${prefix}8f7a1c-1b4d-7c8a-9f01-0123456789ab`
  return {
    schemaVersion: 2,
    conversationId,
    createdAtUtc: `2026-08-15T09:${String(index).padStart(2, '0')}:00.000Z`,
    creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
    workspaceCwd: `/workspaces/Chat ${index}`,
    executionTarget: { kind: 'workspace' },
    projectAttachment: projectId
      ? {
          schemaVersion: 1,
          projectId,
          attachedAtUtc: '2026-08-15T09:00:00.000Z',
          projectPathSnapshot: `/projects/${projectId}`,
          worktreePath: null,
          worktreeBranch: null
        }
      : null,
    lifecycleState: 'ready',
    lastSeq: index,
    createdBy: 'termul'
  }
}

function LocationProbe(): React.JSX.Element {
  return <output data-testid="location">{useLocation().pathname}</output>
}

beforeEach(() => {
  useAcpStore.setState({
    sessions: {},
    sessionIndex: [],
    pendingPermissions: {},
    pendingQuestions: {}
  })
  useConversationStore.getState().reset()
  useProjectStore.setState({ projects: [], activeProjectId: '' })
})

function renderTab(onSessionOpened?: () => void): void {
  render(
    <MemoryRouter>
      <ChatHistoryTab onSessionOpened={onSessionOpened} />
      <LocationProbe />
    </MemoryRouter>
  )
}

describe('ChatHistoryTab global Conversation navigation', () => {
  it('shows project-less and attached Conversations even when no project is active', () => {
    useProjectStore.setState({ projects: [{ id: 'p1', name: 'Demo', color: 'blue' }] })
    useConversationStore.getState().replaceSummaries([summary(0), summary(1, 'p1')])

    renderTab()

    expect(screen.getByText('Chat 0')).toBeInTheDocument()
    expect(screen.getByText('Chat 1')).toBeInTheDocument()

    const attached = screen.getByText('Chat 1').closest('[data-conversation-id]')
    expect(attached).toBeTruthy()
    fireEvent.click(within(attached as HTMLElement).getByLabelText('Show conversation details'))
    expect(within(attached as HTMLElement).getByText('Demo')).toBeInTheDocument()
  })

  it('shows a stable empty state with zero Conversations', () => {
    renderTab()
    expect(screen.getByText('No conversations match this view.')).toBeInTheDocument()
  })

  it('searches the canonical global list and clears on Escape', () => {
    useConversationStore.getState().replaceSummaries([summary(0), summary(1)])
    renderTab()
    const search = screen.getByLabelText('Search conversations')

    fireEvent.change(search, { target: { value: 'Chat 1' } })
    expect(screen.getByText('Chat 1')).toBeInTheDocument()
    expect(screen.queryByText('Chat 0')).not.toBeInTheDocument()

    fireEvent.keyDown(search, { key: 'Escape' })
    expect(screen.getByText('Chat 0')).toBeInTheDocument()
  })

  it('opens rows by canonical ConversationId and closes the mobile drawer callback', () => {
    const conversation = summary(0)
    useConversationStore.getState().replaceSummaries([conversation])
    const onSessionOpened = vi.fn()
    renderTab(onSessionOpened)

    fireEvent.click(screen.getByText('Chat 0'))

    expect(screen.getByTestId('location')).toHaveTextContent(`/c/${conversation.conversationId}`)
    expect(onSessionOpened).toHaveBeenCalledTimes(1)
  })

  it('exposes lifecycle controls keyed by ConversationId', () => {
    useConversationStore.getState().replaceSummaries([summary(0)])
    renderTab()

    expect(screen.getByLabelText('Lifecycle Chat 0')).toBeInTheDocument()
  })

  it('caps rows and lazily loads the next global page', () => {
    useConversationStore
      .getState()
      .replaceSummaries(Array.from({ length: 55 }, (_, index) => summary(index)))
    renderTab()

    expect(screen.getAllByRole('button', { name: /Lifecycle Chat/ })).toHaveLength(50)
    fireEvent.click(screen.getByText('Load 5 more'))
    expect(screen.getAllByRole('button', { name: /Lifecycle Chat/ })).toHaveLength(55)
  })
})
