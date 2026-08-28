import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import type { PendingPermission } from '@/stores/acp-store'
import { PermissionDialog } from './PermissionDialog'

vi.mock('sonner', () => ({
  toast: { error: vi.fn() }
}))

vi.mock('@/stores/acp-store', () => ({
  useAcpStore: (selector: (state: { respondPermission: unknown }) => unknown) =>
    selector({ respondPermission: vi.fn() })
}))

/** A real-world worst case: an absolute path with no spaces to break on. */
const LONG_ACTION =
  'Write /Users/qs/Documents/Termul/sessions/2026/08/28/38ceadcf-158a-4283-98be-7094bcc1d076/VoiceCard/Sources/VoiceCard/Panel/SettingsWindowController.swift'

function permission(overrides: Partial<PendingPermission> = {}): PendingPermission {
  return {
    requestId: 'req-1',
    sessionId: 's-1',
    toolCall: { title: LONG_ACTION, toolCallId: 'call-1' },
    options: [
      { optionId: 'allow-once', name: 'Allow once', kind: 'allow_once' },
      { optionId: 'reject-once', name: 'Reject', kind: 'reject_once' }
    ],
    ...overrides
  } as PendingPermission
}

describe('PermissionDialog long-content layout', () => {
  it('renders the agent-supplied action in its own breakable block, not inside the sentence', () => {
    render(<PermissionDialog permission={permission()} />)

    const action = screen.getByText(LONG_ACTION)
    // `break-all` is what stops a slash-free path segment from overflowing the
    // dialog horizontally; wrapping alone is not enough.
    expect(action).toHaveClass('break-all')
    // Bounded height + scroll keeps an arbitrarily long path from stretching
    // the dialog past the viewport.
    expect(action).toHaveClass('max-h-24')
    expect(action).toHaveClass('overflow-y-auto')
    // The full value stays recoverable on hover even when visually clipped.
    expect(action).toHaveAttribute('title', LONG_ACTION)

    // The description sentence must no longer interpolate the action, or the
    // path would be back inside an unbounded paragraph.
    const description = screen.getByText('The agent wants to run:')
    expect(description).not.toHaveTextContent(LONG_ACTION)
  })

  it('lets a long option label wrap instead of clipping to the button height', () => {
    const longName = 'Allow always for every file under this workspace and remember the choice'
    render(
      <PermissionDialog
        permission={permission({
          options: [{ optionId: 'allow-always', name: longName, kind: 'allow_always' }]
        })}
      />
    )

    const button = screen.getByRole('button', { name: longName })
    // Buttons default to `whitespace-nowrap` + a fixed height, which would clip
    // an agent-supplied label; both have to be released for it to wrap.
    expect(button).toHaveClass('whitespace-normal')
    expect(button).toHaveClass('h-auto')
  })
})
