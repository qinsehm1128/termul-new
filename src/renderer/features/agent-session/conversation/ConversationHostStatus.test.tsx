import { render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'
import { useConversationHostBootstrapStore } from '@/hooks/use-conversation-host-bootstrap'
import { ConversationHostStatus } from './ConversationHostStatus'

const baseStatus = {
  hostKind: 'desktop' as const,
  migrationPhase: 'finalized' as const,
  readerPrecedence: 'conversationV2Only' as const,
  recoveryItemCount: 0,
  recoveryItems: []
}

beforeEach(() => {
  useConversationHostBootstrapStore.getState().reset()
})

describe('ConversationHostStatus', () => {
  for (const state of ['ready', 'migrating', 'hybrid', 'recovery', 'error'] as const) {
    it(`renders the accessible ${state} state with its stable code`, () => {
      const code = `CONVERSATION_HOST_${state.toUpperCase()}`
      useConversationHostBootstrapStore.getState().setReady(
        {
          ...baseStatus,
          state,
          code,
          recoveryItemCount: state === 'recovery' ? 2 : 0
        },
        []
      )
      render(<ConversationHostStatus />)
      const region = screen.getByRole('status')
      expect(region).toHaveAttribute('data-state', state)
      expect(region).toHaveTextContent(code)
      expect(region.className).toContain('max-w-[calc(100vw-1rem)]')
    })
  }
})
