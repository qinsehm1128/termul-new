import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { ChatEmptyState } from './ChatEmptyState'
import { CHAT_CONTENT_WIDTH, CHAT_GUTTER_X } from './chat-layout'

vi.mock('@/stores/acp-store', () => ({
  useAgentIdentity: () => ({ name: 'Cursor', templateId: 'cursor' })
}))

vi.mock('framer-motion', async () => {
  const actual = await vi.importActual<typeof import('framer-motion')>('framer-motion')
  return {
    ...actual,
    useReducedMotion: () => true
  }
})

describe('ChatEmptyState', () => {
  it('uses the same CHAT_GUTTER_X / CHAT_CONTENT_WIDTH column as populated threads', () => {
    const { container } = render(<ChatEmptyState agentId="agent-1" />)
    expect(container.innerHTML).toContain(CHAT_GUTTER_X.split(' ')[0]!)
    expect(container.innerHTML).toContain('@[400px]:px-5')
    expect(container.innerHTML).toContain('max-w-3xl')
    expect(CHAT_CONTENT_WIDTH.split(' ')).toEqual(
      expect.arrayContaining(['mx-auto', 'w-full', 'max-w-3xl'])
    )
  })

  it('names the agent and hides starter prompts until onPick is provided', () => {
    render(<ChatEmptyState agentId="agent-1" />)
    expect(screen.getByRole('heading', { name: 'Chat with Cursor' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Explain this project' })).not.toBeInTheDocument()
    expect(screen.getByText(/for commands/)).toBeInTheDocument()
  })

  it('seeds the composer from a starter prompt', () => {
    const onPick = vi.fn()
    render(<ChatEmptyState agentId="agent-1" onPick={onPick} />)
    fireEvent.click(screen.getByRole('button', { name: 'Explain this project' }))
    expect(onPick).toHaveBeenCalledWith(
      'Give me a high-level overview of this codebase and how it is structured.'
    )
  })
})
