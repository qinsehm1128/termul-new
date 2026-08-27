import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const { conflictMock, conversationApiMock } = vi.hoisted(() => ({
  conflictMock: vi.fn(),
  conversationApiMock: {
    resolveRecovery: vi.fn()
  }
}))

vi.mock('@/hooks/use-session-workspace-sync', () => ({
  resolveSessionWorkspaceConflict: conflictMock
}))

vi.mock('@/lib/conversation-api', () => ({
  conversationApi: conversationApiMock
}))

vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn() }))

import { useSessionWorkspaceSyncStore } from '@/stores/session-workspace-sync-store'
import { WorkspaceConflictBanner } from './WorkspaceConflictBanner'

const one = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
const two = '5f7a1c01-4d1b-4c8a-af01-0123456789ab'
const recoveryItem = {
  recoveryId: 'a'.repeat(64),
  kind: 'ambiguous_workspace_manifest' as const,
  severity: 'warning' as const,
  sourcePaths: ['legacy_workspace_manifests/0/shared.json'],
  conversationIds: [one, two],
  sourceSha256: ['e'.repeat(64)],
  candidateFacts: [],
  provenance: [
    {
      sourceKind: 'legacy_workspace_manifests',
      relativePath: 'legacy_workspace_manifests/0/shared.json',
      sha256: 'e'.repeat(64),
      preservedReadOnly: true as const
    }
  ],
  status: 'unresolved' as const,
  suggestedActions: [
    'inspect',
    'associateConversation',
    'startEmptyWorkspace',
    'dismissPreservedSource'
  ] as const,
  revision: 7,
  associationDecisions: []
}

beforeEach(() => {
  vi.clearAllMocks()
  conversationApiMock.resolveRecovery.mockImplementation(async (request) => ({
    success: true,
    data: {
      recoveryId: request.recoveryId,
      action: request.action,
      authorization: request.action === 'inspect' ? 'read' : 'mutation',
      status: 'unresolved',
      recoveryRevision: 7,
      workspaceRevision: null,
      workspaceChanged: false,
      sourcePaths: recoveryItem.sourcePaths,
      sourceSha256: recoveryItem.sourceSha256,
      candidateFacts: recoveryItem.candidateFacts,
      provenance: recoveryItem.provenance
    }
  }))
  useSessionWorkspaceSyncStore.setState({
    activeConversationId: one,
    basedRevisionByConversation: {},
    conflictsByConversation: {},
    recoveryByConversation: {},
    loadOutcomeByConversation: {},
    restoreInProgressByConversation: {}
  })
})

afterEach(cleanup)

describe('WorkspaceConflictBanner', () => {
  it('scopes a stale conflict to the active Conversation, not the project store', () => {
    useSessionWorkspaceSyncStore.getState().setConflict(one, {
      conversationId: one,
      currentRevision: 5,
      currentUpdatedAtUtc: '2026-08-15T10:00:00.000Z',
      currentUpdateIdentity: 'other-client'
    })
    const { rerender } = render(<WorkspaceConflictBanner />)
    expect(screen.getByRole('alert')).toHaveAttribute('data-conversation-id', one)
    expect(screen.getByText(/revision 5/)).toBeInTheDocument()

    act(() => useSessionWorkspaceSyncStore.getState().setActiveConversationId(two))
    rerender(<WorkspaceConflictBanner />)
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('calls Conversation-scoped conflict actions', () => {
    useSessionWorkspaceSyncStore.getState().setConflict(one, {
      conversationId: one,
      currentRevision: 5,
      currentUpdatedAtUtc: '2026-08-15T10:00:00.000Z'
    })
    render(<WorkspaceConflictBanner conversationId={one} />)
    fireEvent.click(screen.getByRole('button', { name: 'Reload from host' }))
    fireEvent.click(screen.getByRole('button', { name: 'Overwrite with local' }))
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }))
    expect(conflictMock).toHaveBeenNthCalledWith(1, one, 'reload')
    expect(conflictMock).toHaveBeenNthCalledWith(2, one, 'overwrite')
    expect(conflictMock).toHaveBeenNthCalledWith(3, one, 'dismiss')
  })

  it('renders immutable source/checksum context and the exact shared recovery actions', () => {
    useSessionWorkspaceSyncStore.getState().setRecoveryItems(one, [recoveryItem])
    render(<WorkspaceConflictBanner conversationId={one} />)
    expect(
      screen.getByRole('region', {
        name: new RegExp(`ambiguous_workspace_manifest ${'a'.repeat(64)}`)
      })
    ).toBeVisible()
    expect(
      screen.getAllByText(/legacy_workspace_manifests\/0\/shared.json/).length
    ).toBeGreaterThan(0)
    expect(screen.getAllByText(new RegExp(`sha256:${'e'.repeat(64)}`)).length).toBeGreaterThan(0)
    for (const action of recoveryItem.suggestedActions) {
      expect(document.querySelector(`[data-recovery-action="${action}"]`)).toBeVisible()
    }
  })

  it('invokes every exact recovery action with RecoveryItem revision and action payloads', async () => {
    useSessionWorkspaceSyncStore.getState().setRecoveryItems(one, [recoveryItem])
    render(<WorkspaceConflictBanner conversationId={one} />)
    for (const action of recoveryItem.suggestedActions) {
      const button = document.querySelector<HTMLButtonElement>(`[data-recovery-action="${action}"]`)
      expect(button).toBeVisible()
      fireEvent.click(button!)
      await waitFor(() =>
        expect(conversationApiMock.resolveRecovery).toHaveBeenCalledWith(
          expect.objectContaining({
            recoveryId: recoveryItem.recoveryId,
            expectedRevision: 7,
            action
          })
        )
      )
    }
    expect(conversationApiMock.resolveRecovery).toHaveBeenCalledTimes(4)
  })

  it('uses accessible buttons and responsive wrapping', () => {
    useSessionWorkspaceSyncStore.getState().setRecoveryItems(one, [recoveryItem])
    render(<WorkspaceConflictBanner conversationId={one} />)
    expect(screen.getByRole('alert')).toHaveAttribute('aria-live', 'polite')
    for (const button of screen.getAllByRole('button')) {
      expect(button).toHaveAttribute('type', 'button')
      expect(button.parentElement?.className).toContain('grid-cols-2')
    }
  })
})
