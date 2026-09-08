import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { ConversationLifecycleActions } from '@/components/chat/ChatHistoryEntryRow'
import { useConversationStore } from '@/stores/conversation-store'

vi.mock('@/stores/acp-store', () => ({
  useAcpStore: (selector: (state: unknown) => unknown) =>
    selector({
      closeChatView: vi.fn(),
      detachAgentBinding: vi.fn(),
      rebindDetachedBinding: vi.fn(),
      suspendAgentBinding: vi.fn(),
      replaceAgentBinding: vi.fn(),
      deleteConversation: vi.fn(),
      agentConfigs: []
    }),
  useAgentTemplateId: () => null
}))

const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'

function seed(backend?: 'agent' | 'terminal'): void {
  useConversationStore.setState({
    summariesById: {
      [conversationId]: {
        schemaVersion: 2,
        conversationId,
        createdAtUtc: '2026-08-15T09:45:15.123Z',
        creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
        workspaceCwd: '/conversations/x',
        executionTarget: { kind: 'workspace' },
        projectAttachment: null,
        lifecycleState: 'ready',
        ...(backend ? { backend } : {}),
        lastSeq: 0,
        createdBy: 'se-manager',
        title: null,
        titleSource: null
      }
    }
  } as never)
}

function openMenu(): void {
  render(<ConversationLifecycleActions conversationId={conversationId} title="chat" />)
  const trigger = screen.getByRole('button')
  // Radix opens on pointerdown, not click.
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false, pointerType: 'mouse' })
  fireEvent.click(trigger)
}

describe('ConversationLifecycleActions — backend-scoped menu', () => {
  afterEach(() => {
    // Radix renders the menu into a body portal; without an explicit unmount
    // the next test's trigger opens against a stale tree.
    cleanup()
    useConversationStore.setState({ summariesById: {} } as never)
  })

  it('hides every binding action for a terminal-backed conversation', () => {
    // Each of these calls the binding lifecycle API. With no binding they fail
    // with CONVERSATION_BINDING_NOT_FOUND — after the user already confirmed a
    // destructive-sounding dialog.
    seed('terminal')
    openMenu()

    for (const name of [
      /detach binding/i,
      /rebind detached agent/i,
      /suspend agent/i,
      /restart agent/i
    ]) {
      expect(screen.queryAllByRole('menuitem', { name })).toHaveLength(0)
    }
  })

  it('keeps the actions that belong to the conversation itself', () => {
    // Close view, rename and delete are properties of the Conversation, not of
    // an agent — removing them would strip a terminal conversation of any way
    // to be renamed or deleted.
    seed('terminal')
    openMenu()

    for (const name of [/close chat view/i, /^rename$/i, /delete conversation/i]) {
      expect(screen.queryAllByRole('menuitem', { name }).length).toBeGreaterThan(0)
    }
  })

  it('shows the binding actions for a conversation with no declared backend', () => {
    // Every record predating the discriminator is agent-backed and keeps them.
    seed()
    openMenu()

    for (const name of [
      /detach binding/i,
      /rebind detached agent/i,
      /suspend agent/i,
      /restart agent/i
    ]) {
      expect(screen.queryAllByRole('menuitem', { name }).length).toBeGreaterThan(0)
    }
  })
})
