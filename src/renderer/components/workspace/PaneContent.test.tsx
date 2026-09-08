import { render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ErrorBoundary } from '@/components/ErrorBoundary'
import type { LeafNode } from '@/types/workspace.types'

// CAP-6 Row 3: EditorPanel is React.lazy in PaneContent.tsx — when an editor
// pane opens, <Suspense fallback={<PaneSkeleton/>}> shows a skeleton until the
// lazy chunk resolves, then the editor renders. This test verifies the
// lazy+Suspense mechanism resolves and renders EditorPanel with the correct
// filePath prop. EditorPanel is stubbed so real CodeMirror/BlockNote don't
// load in jsdom — the point is to verify the lazy boundary, not editor internals.
//
// CAP-6 Patch 4: The error-path test simulates a chunk-load failure by making
// the EditorPanel mock throw, then asserts the ErrorBoundary surfaces the error
// and calls logFrontendError (per the spec's "If the chunk fails to load, the
// Suspense boundary surfaces the error via log-api.ts"). The existing
// ErrorBoundary already calls logFrontendError — no per-pane wrapper needed.

const { editorMock } = vi.hoisted(() => ({
  editorMock: vi.fn()
}))

vi.mock('@/components/editor/EditorPanel', () => ({
  EditorPanel: editorMock
}))

const { logFrontendError } = vi.hoisted(() => ({
  logFrontendError: vi.fn()
}))

vi.mock('@/lib/log-api', () => ({ logFrontendError }))

vi.mock('@/stores/project-store', () => ({
  useProjectStore: vi.fn((selector: (s: { activeProjectId: string }) => unknown) =>
    selector({ activeProjectId: 'proj-1' })
  )
}))

vi.mock('@/stores/terminal-store', () => ({
  useTerminalStore: vi.fn((selector: (s: { terminals: never[] }) => unknown) =>
    selector({ terminals: [] })
  ),
  useTerminalActions: vi.fn(() => ({ setTerminalPtyId: vi.fn() }))
}))

vi.mock('@/stores/workspace-store', () => ({
  useWorkspaceStore: vi.fn((selector: (s: Record<string, unknown>) => unknown) =>
    selector({
      root: { type: 'leaf', id: 'pane-1', tabs: [], activeTabId: null },
      activePaneId: 'pane-1',
      fullscreenPaneId: null,
      agentLauncherPaneId: null,
      setActivePane: vi.fn()
    })
  ),
  getAllLeafPanes: () => []
}))

vi.mock('@/hooks/use-mobile-web-shell', () => ({
  useMobileWebShell: () => false
}))

vi.mock('@/hooks/use-pane-dnd', () => ({
  usePaneDnd: () => ({ isDragging: false, previewTarget: null })
}))

vi.mock('@/components/workspace/WorkspaceTabBar', () => ({
  WorkspaceTabBar: () => <div data-testid="tabbar-stub" />
}))
vi.mock('@/components/workspace/DropZoneOverlay', () => ({
  DropZoneOverlay: () => null
}))
vi.mock('@/components/agents/AgentLauncher', () => ({
  AgentLauncher: () => <div data-testid="launcher-stub" />
}))
vi.mock('@/components/agents/AgentIcon', () => ({
  AgentIcon: () => <span data-testid="agent-icon-stub" />
}))

import { useConversationStore } from '@/stores/conversation-store'
import { PaneContent } from './PaneContent'

const editorPane: LeafNode = {
  type: 'leaf',
  id: 'pane-1',
  activeTabId: 'tab-editor-1',
  tabs: [{ type: 'editor', id: 'tab-editor-1', filePath: '/project/foo.ts' }]
}

describe('PaneContent — editor pane lazy/Suspense boundary (CAP-6 Row 3)', () => {
  beforeEach(() => {
    editorMock.mockImplementation(({ filePath }: { filePath: string }) => (
      <div data-testid="editor-stub" data-filepath={filePath}>
        editor
      </div>
    ))
  })

  afterEach(() => {
    editorMock.mockReset()
    logFrontendError.mockReset()
  })

  it('renders EditorPanel through React.lazy + <Suspense>', async () => {
    render(
      <MemoryRouter>
        <PaneContent pane={editorPane} />
      </MemoryRouter>
    )

    const editor = await screen.findByTestId('editor-stub')
    expect(editor).toBeInTheDocument()
    expect(editor.getAttribute('data-filepath')).toBe('/project/foo.ts')
  })
})

describe('PaneContent — chunk-load failure error path (CAP-6 Patch 4)', () => {
  beforeEach(() => {
    editorMock.mockImplementation(() => {
      throw new Error('Failed to load dynamic target chunk')
    })
  })

  afterEach(() => {
    editorMock.mockReset()
    logFrontendError.mockReset()
  })

  it('surfaces the error via ErrorBoundary + logFrontendError when the editor chunk fails', async () => {
    render(
      <MemoryRouter>
        <ErrorBoundary context="editorPane">
          <PaneContent pane={editorPane} />
        </ErrorBoundary>
      </MemoryRouter>
    )

    // The ErrorBoundary catches the thrown error, calls logFrontendError,
    // and renders the ErrorFallback UI with the error message.
    await waitFor(() => {
      expect(logFrontendError).toHaveBeenCalledWith(
        expect.objectContaining({ source: 'ErrorBoundary:editorPane' })
      )
    })

    expect(screen.getByText('Something went wrong in Editor Pane')).toBeInTheDocument()
    expect(screen.getByText('Failed to load dynamic target chunk')).toBeInTheDocument()
  })
})

describe('PaneContent — an agent-chat tab whose conversation has no agent', () => {
  const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
  const chatPane: LeafNode = {
    type: 'leaf',
    id: 'pane-2',
    activeTabId: 'tab-chat-1',
    tabs: [{ type: 'agent-chat', id: 'tab-chat-1', conversationId }]
  }

  function seedConversation(backend?: 'agent' | 'terminal'): void {
    useConversationStore.setState({
      summariesById: {
        [conversationId]: {
          schemaVersion: 2,
          conversationId,
          createdAtUtc: '2026-08-15T09:45:15.123Z',
          creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
          workspaceCwd: '/conversations/terminal',
          executionTarget: { kind: 'workspace' },
          projectAttachment: null,
          lifecycleState: 'ready',
          ...(backend ? { backend } : {}),
          lastSeq: 0,
          createdBy: 'se-manager',
          title: null,
          titleSource: null
        }
      },
      conversationIds: [conversationId],
      openingById: {}
    } as never)
  }

  afterEach(() => {
    useConversationStore.setState({ summariesById: {}, conversationIds: {} } as never)
  })

  it('does not offer the launcher for a terminal-backed conversation', async () => {
    // The launcher is a restart surface for an agent. Offering it here invites
    // the user to start something this conversation never asked for.
    seedConversation('terminal')
    render(
      <MemoryRouter>
        <PaneContent pane={chatPane} />
      </MemoryRouter>
    )

    expect(await screen.findByTestId('conversation-terminal-empty')).toBeInTheDocument()
    expect(screen.queryByTestId('launcher-stub')).not.toBeInTheDocument()
  })

  it('still offers the launcher when the conversation declares no backend', async () => {
    // Every record predating the discriminator: an agent conversation whose
    // agent is simply not currently bound.
    seedConversation()
    render(
      <MemoryRouter>
        <PaneContent pane={chatPane} />
      </MemoryRouter>
    )

    expect(await screen.findByTestId('launcher-stub')).toBeInTheDocument()
    expect(screen.queryByTestId('conversation-terminal-empty')).not.toBeInTheDocument()
  })
})
