import type {
  RecoveryAction,
  RecoveryActionResult,
  RecoveryItemV1
} from '@shared/types/conversation-recovery.types'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { i18n } from '@/i18n'

const { resolveRecovery, loadSessionWorkspace } = vi.hoisted(() => ({
  resolveRecovery: vi.fn(),
  loadSessionWorkspace: vi.fn()
}))

vi.mock('@/lib/conversation-api', () => ({
  conversationApi: { resolveRecovery }
}))

vi.mock('@/hooks/use-session-workspace-sync', () => ({
  loadSessionWorkspace
}))

import { ConversationRecoveryPanel } from './ConversationRecoveryPanel'

const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
const checksum = 'e'.repeat(64)
const item: RecoveryItemV1 = {
  recoveryId: 'a'.repeat(64),
  kind: 'ambiguous_workspace_manifest',
  severity: 'blocking',
  sourcePaths: ['legacy_workspace_manifests/0/shared.json'],
  conversationIds: [conversationId],
  sourceSha256: [checksum],
  candidateFacts: [{ candidate: 'preserved' }],
  provenance: [
    {
      sourceKind: 'legacy_workspace_manifests',
      relativePath: 'legacy_workspace_manifests/0/shared.json',
      sha256: checksum,
      preservedReadOnly: true
    }
  ],
  status: 'unresolved',
  suggestedActions: [
    'inspect',
    'associateConversation',
    'startEmptyWorkspace',
    'dismissPreservedSource'
  ],
  revision: 7,
  associationDecisions: []
}

const redactedItem: RecoveryItemV1 = {
  ...item,
  sourcePaths: [],
  sourceSha256: [],
  candidateFacts: [],
  provenance: []
}

function resultFor(request: RecoveryAction): RecoveryActionResult {
  const mutation = request.action !== 'inspect'
  return {
    recoveryId: request.recoveryId,
    action: request.action,
    authorization: mutation ? 'mutation' : 'read',
    status:
      request.action === 'inspect'
        ? 'unresolved'
        : request.action === 'associateConversation'
          ? 'resolvedAssociated'
          : request.action === 'startEmptyWorkspace'
            ? 'resolvedStartedEmpty'
            : 'dismissedPreserved',
    recoveryRevision: request.expectedRevision + (mutation ? 1 : 0),
    workspaceRevision: request.action === 'startEmptyWorkspace' ? 1 : null,
    workspaceChanged: request.action === 'startEmptyWorkspace',
    sourcePaths: item.sourcePaths,
    sourceSha256: item.sourceSha256,
    candidateFacts: item.candidateFacts,
    provenance: item.provenance
  }
}

beforeEach(async () => {
  await i18n.changeLanguage('en')
  resolveRecovery.mockReset()
  loadSessionWorkspace.mockReset()
  loadSessionWorkspace.mockResolvedValue(true)
  resolveRecovery.mockImplementation(async (request: RecoveryAction) => ({
    success: true,
    data: resultFor(request)
  }))
})

describe('ConversationRecoveryPanel', () => {
  it('renders immutable checksums and provenance with exact shared camelCase actions', () => {
    render(<ConversationRecoveryPanel items={[item]} conversationId={conversationId} />)

    expect(
      screen.getAllByText(/legacy_workspace_manifests\/0\/shared.json/).length
    ).toBeGreaterThan(0)
    expect(screen.getAllByText(new RegExp(`sha256:${checksum}`)).length).toBeGreaterThan(0)
    expect(screen.getByText(/preserved read-only/)).toBeInTheDocument()
    expect(screen.getByText('{"candidate":"preserved"}')).toBeInTheDocument()
    for (const action of item.suggestedActions) {
      expect(document.querySelector(`[data-recovery-action="${action}"]`)).not.toBeNull()
    }
  })

  it('localizes visible actions while retaining exact shared action identities', async () => {
    await i18n.changeLanguage('zh-CN')
    render(<ConversationRecoveryPanel items={[item]} conversationId={conversationId} />)

    expect(screen.getByRole('button', { name: '检查保留来源' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '启动空工作区' })).toBeInTheDocument()
    expect(document.querySelector('[data-recovery-action="startEmptyWorkspace"]')).not.toBeNull()
  })

  it('sends expectedRevision, UUID mutation keys, and action-specific payloads', async () => {
    render(<ConversationRecoveryPanel items={[item]} conversationId={conversationId} />)

    for (const action of item.suggestedActions) {
      const button = document.querySelector<HTMLButtonElement>(`[data-recovery-action="${action}"]`)
      if (!button) throw new Error(`missing recovery action ${action}`)
      fireEvent.click(button)
      await waitFor(() =>
        expect(resolveRecovery).toHaveBeenCalledTimes(item.suggestedActions.indexOf(action) + 1)
      )
    }

    const requests = resolveRecovery.mock.calls.map(([request]) => request as RecoveryAction)
    expect(requests[0]).toEqual({
      recoveryId: item.recoveryId,
      expectedRevision: 7,
      action: 'inspect',
      payload: {}
    })
    expect(requests[1]).toMatchObject({
      recoveryId: item.recoveryId,
      expectedRevision: 7,
      action: 'associateConversation',
      payload: { conversationId }
    })
    expect(requests[2]).toMatchObject({
      recoveryId: item.recoveryId,
      expectedRevision: 7,
      action: 'startEmptyWorkspace',
      payload: { conversationId, expectedWorkspaceRevision: null }
    })
    expect(requests[3]).toMatchObject({
      recoveryId: item.recoveryId,
      expectedRevision: 7,
      action: 'dismissPreservedSource',
      payload: { reasonCode: 'deferLegacyProjection' }
    })
    for (const request of requests.slice(1)) {
      expect(request.idempotencyKey).toMatch(
        /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/
      )
    }
    expect(loadSessionWorkspace).toHaveBeenCalledWith(conversationId)
  })

  it('publishes resolved item status and revision to embedded store integrations', async () => {
    const onItemsChange = vi.fn()
    render(
      <ConversationRecoveryPanel
        items={[item]}
        conversationId={conversationId}
        onItemsChange={onItemsChange}
      />
    )

    const associate = document.querySelector<HTMLButtonElement>(
      '[data-recovery-action="associateConversation"]'
    )
    if (!associate) throw new Error('missing associateConversation action')
    fireEvent.click(associate)

    await waitFor(() =>
      expect(onItemsChange).toHaveBeenCalledWith([
        expect.objectContaining({
          recoveryId: item.recoveryId,
          status: 'resolvedAssociated',
          revision: 8,
          sourcePaths: item.sourcePaths,
          sourceSha256: item.sourceSha256,
          provenance: item.provenance
        })
      ])
    )
  })

  it('shows a CAS conflict without hiding preserved source evidence', async () => {
    resolveRecovery.mockResolvedValueOnce({
      success: false,
      code: 'CONVERSATION_CONFLICT',
      error: 'workspace already exists'
    })
    render(<ConversationRecoveryPanel items={[item]} conversationId={conversationId} />)

    const startEmpty = document.querySelector<HTMLButtonElement>(
      '[data-recovery-action="startEmptyWorkspace"]'
    )
    if (!startEmpty) throw new Error('missing startEmptyWorkspace action')
    fireEvent.click(startEmpty)

    expect(await screen.findByRole('alert')).toHaveAttribute(
      'data-error-code',
      'CONVERSATION_CONFLICT'
    )
    expect(screen.getAllByText(new RegExp(`sha256:${checksum}`)).length).toBeGreaterThan(0)
    expect(screen.getByText(/preserved read-only/)).toBeInTheDocument()
  })

  it('reuses the UUID idempotency key when the same mutation request is retried', async () => {
    resolveRecovery.mockResolvedValue({
      success: false,
      code: 'CONVERSATION_CONFLICT',
      error: 'retryable conflict'
    })
    render(<ConversationRecoveryPanel items={[item]} conversationId={conversationId} />)

    const associate = document.querySelector<HTMLButtonElement>(
      '[data-recovery-action="associateConversation"]'
    )
    if (!associate) throw new Error('missing associateConversation action')
    fireEvent.click(associate)
    await waitFor(() => expect(resolveRecovery).toHaveBeenCalledTimes(1))
    fireEvent.click(associate)
    await waitFor(() => expect(resolveRecovery).toHaveBeenCalledTimes(2))

    const first = resolveRecovery.mock.calls[0][0] as RecoveryAction
    const second = resolveRecovery.mock.calls[1][0] as RecoveryAction
    expect(first.action).toBe('associateConversation')
    expect(second.action).toBe('associateConversation')
    if (first.action !== 'associateConversation' || second.action !== 'associateConversation') {
      throw new Error('unexpected recovery action')
    }
    expect(second.idempotencyKey).toBe(first.idempotencyKey)
  })

  it('reveals exact authenticated Inspect evidence from a redacted item without mutation', async () => {
    const originalSnapshot = structuredClone(redactedItem)
    const originalPaths = redactedItem.sourcePaths
    const onItemsChange = vi.fn()
    render(
      <ConversationRecoveryPanel
        items={[redactedItem]}
        conversationId={conversationId}
        onItemsChange={onItemsChange}
      />
    )

    expect(screen.queryByText(/legacy_workspace_manifests\/0\/shared.json/)).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Inspect preserved source' }))

    expect(
      (await screen.findAllByText(/legacy_workspace_manifests\/0\/shared.json/)).length
    ).toBeGreaterThan(0)
    expect(screen.getAllByText(new RegExp(`sha256:${checksum}`)).length).toBeGreaterThan(0)
    expect(screen.getByText('{"candidate":"preserved"}')).toBeInTheDocument()
    expect(screen.getByText(/preserved read-only/)).toBeInTheDocument()
    expect(redactedItem).toEqual(originalSnapshot)
    expect(redactedItem.sourcePaths).toBe(originalPaths)
    expect(onItemsChange).not.toHaveBeenCalled()
  })

  it.each([
    ['recoveryId', { recoveryId: 'b'.repeat(64) }],
    ['revision', { recoveryRevision: redactedItem.revision + 1 }]
  ])('rejects mismatched Inspect %s and renders no authenticated evidence', async (_field, patch) => {
    const originalSnapshot = structuredClone(redactedItem)
    resolveRecovery.mockImplementation(async (request: RecoveryAction) => ({
      success: true,
      data: { ...resultFor(request), ...patch }
    }))
    render(<ConversationRecoveryPanel items={[redactedItem]} conversationId={conversationId} />)

    fireEvent.click(screen.getByRole('button', { name: 'Inspect preserved source' }))

    expect(await screen.findByRole('alert')).toHaveAttribute(
      'data-error-code',
      'CONVERSATION_RECOVERY_FAILED'
    )
    expect(screen.queryByText(/legacy_workspace_manifests\/0\/shared.json/)).not.toBeInTheDocument()
    expect(redactedItem).toEqual(originalSnapshot)
  })

  it('keeps forbidden Inspect redacted and exposes only the stable application code', async () => {
    resolveRecovery.mockResolvedValueOnce({
      success: false,
      code: 'FORBIDDEN',
      error: 'remote principal lacks the required capability'
    })
    render(<ConversationRecoveryPanel items={[redactedItem]} conversationId={conversationId} />)

    fireEvent.click(screen.getByRole('button', { name: 'Inspect preserved source' }))

    expect(await screen.findByRole('alert')).toHaveAttribute('data-error-code', 'FORBIDDEN')
    expect(screen.queryByText(/legacy_workspace_manifests\/0\/shared.json/)).not.toBeInTheDocument()
    expect(document.body.textContent).not.toContain(checksum)
    expect(document.body.textContent).not.toContain(
      'remote principal lacks the required capability'
    )
  })

  it('recovery action result discarded on revision mismatch', async () => {
    let resolveAction!: (value: { success: true; data: RecoveryActionResult }) => void
    resolveRecovery.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveAction = resolve
        })
    )
    const onItemsChange = vi.fn()
    const { rerender } = render(
      <ConversationRecoveryPanel
        items={[item]}
        conversationId={conversationId}
        onItemsChange={onItemsChange}
      />
    )
    const associate = document.querySelector<HTMLButtonElement>(
      '[data-recovery-action="associateConversation"]'
    )
    if (!associate) throw new Error('missing associateConversation action')
    fireEvent.click(associate)
    await waitFor(() => expect(resolveRecovery).toHaveBeenCalledTimes(1))

    const newer = { ...item, revision: item.revision + 1 }
    rerender(
      <ConversationRecoveryPanel
        items={[newer]}
        conversationId={conversationId}
        onItemsChange={onItemsChange}
      />
    )
    resolveAction({
      success: true,
      data: resultFor({
        recoveryId: item.recoveryId,
        expectedRevision: item.revision,
        action: 'associateConversation',
        payload: { conversationId }
      })
    })
    await waitFor(() => expect(resolveRecovery).toHaveBeenCalledTimes(1))
    expect(onItemsChange).not.toHaveBeenCalled()
    expect(screen.getByText(new RegExp(`revision ${newer.revision}`))).toBeInTheDocument()
  })
})
