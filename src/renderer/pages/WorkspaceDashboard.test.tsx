import type { ConversationRecordV2 } from '@shared/types/conversation.types'
import type { RecoveryItemV1 } from '@shared/types/conversation-recovery.types'
import { cleanup, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// Mock window.api before any imports that use it
Object.defineProperty(window, 'api', {
  value: {
    persistence: {
      read: vi.fn(() => Promise.resolve({ success: true, data: undefined })),
      write: vi.fn(() => Promise.resolve({ success: true }))
    }
  } as unknown as Window['api'],
  writable: true
})

vi.mock('@/lib/conversation-api', () => ({
  conversationApi: { resolveRecovery: vi.fn() }
}))

// Mock project store
vi.mock('@/stores/project-store', () => ({
  useProjectsLoaded: () => true,
  useProjects: () => [],
  useActiveProject: () => undefined,
  useActiveProjectId: () => '',
  useProjectStore: () => ({ activeProjectId: '' }),
  useProjectActions: () => ({
    selectProject: vi.fn(),
    addProject: vi.fn(),
    updateProject: vi.fn(),
    deleteProject: vi.fn(),
    archiveProject: vi.fn(),
    restoreProject: vi.fn(),
    reorderProjects: vi.fn()
  })
}))

// Mock terminal store
vi.mock('@/stores/terminal-store', () => ({
  useAllTerminals: () => [],
  useTerminals: () => [],
  useActiveTerminal: () => undefined,
  useActiveTerminalId: () => '',
  useTerminalStore: () => ({ terminals: [], activeTerminalId: '' }),
  useTerminalActions: () => ({
    selectTerminal: vi.fn(),
    addTerminal: vi.fn(),
    closeTerminal: vi.fn(),
    renameTerminal: vi.fn(),
    reorderTerminals: vi.fn(),
    setTerminalPtyId: vi.fn()
  })
}))

// Mock app settings store
vi.mock('@/stores/app-settings-store', () => ({
  useTerminalFontSize: () => 14,
  useDefaultShell: () => '',
  useMaxTerminalsPerProject: () => 10,
  useUpdateAppSetting: () => vi.fn(),
  useAppSettingsStore: () => ({
    getState: () => ({ settings: {}, updateSetting: vi.fn() })
  })
}))

import { ConversationRecoveryPanel } from '@/components/conversation/ConversationRecoveryPanel'
import { useConversationStore } from '@/stores/conversation-store'
import WorkspaceDashboard from './WorkspaceDashboard'

const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
const conversation: ConversationRecordV2 = {
  schemaVersion: 2,
  conversationId,
  createdAtUtc: '2026-08-15T09:45:15.123Z',
  creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
  workspaceCwd: `/visible/sessions/2026/08/15/${conversationId}`,
  executionTarget: { kind: 'workspace' },
  projectAttachment: null,
  lifecycleState: 'ready',
  lastSeq: 0,
  createdBy: 'termul'
}
const redactedRecoveryItem: RecoveryItemV1 = {
  recoveryId: 'a'.repeat(64),
  kind: 'ambiguous_workspace_manifest',
  severity: 'warning',
  sourcePaths: [],
  conversationIds: [conversationId],
  sourceSha256: [],
  candidateFacts: [],
  provenance: [],
  status: 'unresolved',
  suggestedActions: ['inspect'],
  revision: 7,
  associationDecisions: []
}

function renderDashboard(withRootRecoveryOwner = false): ReturnType<typeof render> {
  return render(
    <MemoryRouter>
      {withRootRecoveryOwner ? <ConversationRecoveryPanel /> : null}
      <WorkspaceDashboard />
    </MemoryRouter>
  )
}

describe('WorkspaceDashboard', () => {
  beforeEach(() => {
    useConversationStore.getState().reset()
  })

  afterEach(() => {
    cleanup()
  })

  it('renders the project-less Conversation creation entry point', () => {
    renderDashboard()

    expect(screen.getByRole('heading', { name: 'Your Conversation workspace' })).toBeVisible()
    expect(screen.getByText(/Start a chat without creating a project/)).toBeVisible()
    expect(screen.getByText(/No conversations yet/)).toBeVisible()
  })

  it('inherits one root recovery owner without mounting a dashboard duplicate', () => {
    useConversationStore.getState().replaceSummaries([conversation])
    useConversationStore.getState().setRecoveryItems([redactedRecoveryItem])

    const dashboardOnly = renderDashboard()
    expect(screen.getByTestId('conversation-list')).toBeVisible()
    expect(screen.getByText('Untitled conversation')).toBeVisible()
    expect(document.querySelectorAll('[data-conversation-recovery-panel]')).toHaveLength(0)
    dashboardOnly.unmount()
    cleanup()

    renderDashboard(true)
    expect(screen.getByTestId('conversation-list')).toBeVisible()
    expect(screen.getByRole('button', { name: 'Inspect preserved source' })).toBeEnabled()
    expect(document.querySelectorAll('[data-conversation-recovery-panel]')).toHaveLength(1)
    for (const action of redactedRecoveryItem.suggestedActions) {
      expect(document.querySelectorAll(`[data-recovery-action="${action}"]`)).toHaveLength(1)
    }
  })
})
