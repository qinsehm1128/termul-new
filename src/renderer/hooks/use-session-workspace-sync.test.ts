import { act, cleanup, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const {
  getMock,
  writeMock,
  recoveryMock,
  logMock,
  terminalResumeMock,
  terminalSpawnMock,
  terminalTerminateMock
} = vi.hoisted(() => ({
  getMock: vi.fn(),
  writeMock: vi.fn(),
  recoveryMock: vi.fn(),
  logMock: vi.fn(),
  terminalResumeMock: vi.fn(),
  terminalSpawnMock: vi.fn(),
  terminalTerminateMock: vi.fn()
}))

vi.mock('@/lib/session-workspace-api', () => ({
  sessionWorkspaceApi: {
    getWorkspace: getMock,
    writeWorkspace: writeMock,
    resolveRecovery: recoveryMock
  }
}))
vi.mock('@/lib/log-api', () => ({ logFrontendError: logMock }))
vi.mock('@/lib/terminal-api', () => ({
  terminalApi: {
    resume: terminalResumeMock,
    spawn: terminalSpawnMock,
    terminate: terminalTerminateMock,
    closeView: vi.fn()
  }
}))
vi.mock('@/hooks/useTerminalAutoSave', () => ({ isTerminalRestoreInProgress: () => false }))

import type { SessionWorkspaceV1 } from '@shared/types/session-workspace.types'
import { useAcpStore } from '@/stores/acp-store'
import { useConversationStore } from '@/stores/conversation-store'
import { useEditorStore } from '@/stores/editor-store'
import { useSessionWorkspaceSyncStore } from '@/stores/session-workspace-sync-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { useWorkspaceStore } from '@/stores/workspace-store'
import {
  buildSessionWorkspace,
  getActiveConversationId,
  loadSessionWorkspace,
  performSessionWorkspaceWrite,
  resolveSessionWorkspaceConflict,
  resolveSessionWorkspaceRecovery,
  useSessionWorkspaceSync
} from './use-session-workspace-sync'

const one = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
const two = '5f7a1c01-4d1b-4c8a-af01-0123456789ab'

function deferred<T>(): {
  promise: Promise<T>
  resolve: (value: T) => void
} {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

function workspace(conversationId: string, revision: number, leafId: string): SessionWorkspaceV1 {
  return {
    schemaVersion: 1,
    conversationId,
    revision,
    updatedAtUtc: '2026-08-15T10:00:00.000Z',
    topology: {
      type: 'leaf',
      id: leafId,
      terminalIds: [],
      editorIds: [],
      activeTabId: null
    },
    activePaneId: leafId,
    resources: [],
    projectionState: { status: 'native' }
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  useSessionWorkspaceSyncStore.setState({
    activeConversationId: null,
    basedRevisionByConversation: {},
    conflictsByConversation: {},
    recoveryByConversation: {},
    loadOutcomeByConversation: {},
    restoreInProgressByConversation: {}
  })
  useWorkspaceStore.getState().resetLayout()
  useConversationStore.getState().reset()
  useAcpStore.setState({ sessions: {}, activeSessionId: null })
  useEditorStore.getState().clearAllFiles()
  useTerminalStore.setState({ terminals: [], activeTerminalId: '', ptyIdIndex: new Map() })
  terminalSpawnMock.mockResolvedValue({
    success: false,
    error: 'not expected',
    code: 'SPAWN_FAILED'
  })
  terminalTerminateMock.mockResolvedValue({ success: true, data: undefined })
})

afterEach(() => {
  vi.useRealTimers()
  cleanup()
})

describe('Conversation-scoped SessionWorkspace sync', () => {
  it('restores two Conversations independently and keeps their revisions isolated', async () => {
    getMock
      .mockResolvedValueOnce({
        success: true,
        data: { status: 'loaded', workspace: workspace(one, 3, 'leaf-one') }
      })
      .mockResolvedValueOnce({
        success: true,
        data: { status: 'loaded', workspace: workspace(two, 8, 'leaf-two') }
      })

    await loadSessionWorkspace(one)
    expect(useWorkspaceStore.getState().root.id).toBe('leaf-one')
    await loadSessionWorkspace(two)
    expect(useWorkspaceStore.getState().root.id).toBe('leaf-two')
    const store = useSessionWorkspaceSyncStore.getState()
    expect(store.getBasedRevision(one)).toBe(3)
    expect(store.getBasedRevision(two)).toBe(8)
  })

  it('refuses to replace WorkspaceStore after an activation guard becomes stale', async () => {
    const response = deferred<{
      success: true
      data: { status: 'loaded'; workspace: SessionWorkspaceV1 }
    }>()
    getMock.mockReturnValue(response.promise)
    const originalRoot = useWorkspaceStore.getState().root
    let current = true

    const loading = loadSessionWorkspace(one, () => current)
    current = false
    response.resolve({
      success: true,
      data: { status: 'loaded', workspace: workspace(one, 3, 'stale-leaf') }
    })

    await expect(loading).resolves.toBe(false)
    expect(useWorkspaceStore.getState().root).toBe(originalRoot)
    expect(useSessionWorkspaceSyncStore.getState().loadOutcomeByConversation[one]).toBeUndefined()
    expect(useSessionWorkspaceSyncStore.getState().getBasedRevision(one)).toBeNull()
  })

  it('hydrates live and denied terminal descriptors before rebuilding topology', async () => {
    const coldWorkspace = workspace(one, 4, 'leaf-cold')
    if (coldWorkspace.topology?.type !== 'leaf') {
      throw new Error('expected leaf')
    }
    coldWorkspace.topology.terminalIds = ['record-live', 'record-denied']
    coldWorkspace.topology.activeTabId = 'term-record-live'
    coldWorkspace.resources = [
      {
        kind: 'terminal',
        terminalId: 'pty-live',
        terminalRecordId: 'record-live',
        conversationId: one
      },
      {
        kind: 'terminal',
        terminalId: 'pty-denied',
        terminalRecordId: 'record-denied',
        conversationId: one
      },
      {
        kind: 'terminal',
        terminalId: 'pty-live',
        terminalRecordId: 'record-live',
        conversationId: one
      }
    ]
    getMock.mockResolvedValue({
      success: true,
      data: { status: 'loaded', workspace: coldWorkspace }
    })
    const rootBeforeLoad = useWorkspaceStore.getState().root.id
    terminalResumeMock.mockImplementation(async (request: { terminalId: string }) => {
      const terminal = useTerminalStore.getState().findTerminalByPtyId(request.terminalId)
      expect(terminal).toMatchObject({
        ptyId: request.terminalId,
        healthStatus: 'disconnected',
        conversationId: one
      })
      expect(useWorkspaceStore.getState().root.id).toBe(rootBeforeLoad)
      if (request.terminalId === 'pty-denied') {
        return { success: false, error: 'Unauthorized', code: 'UNAUTHORIZED' }
      }
      return {
        success: true,
        data: {
          terminal: {
            id: 'pty-live',
            shell: 'bash',
            cwd: '/workspace/live',
            pid: 42,
            cols: 100,
            rows: 30,
            latestSeq: 17,
            gap: false
          },
          claim: 'memory-only-resume-grant'
        }
      }
    })

    await expect(loadSessionWorkspace(one)).resolves.toBe(true)

    expect(terminalResumeMock).toHaveBeenCalledTimes(2)
    expect(terminalResumeMock).toHaveBeenCalledWith({
      conversationId: one,
      terminalId: 'pty-live',
      lastSeq: 0
    })
    expect(terminalResumeMock).toHaveBeenCalledWith({
      conversationId: one,
      terminalId: 'pty-denied',
      lastSeq: 0
    })
    expect(terminalSpawnMock).not.toHaveBeenCalled()
    expect(terminalTerminateMock).not.toHaveBeenCalled()

    const terminals = useTerminalStore.getState().terminals
    expect(terminals.find((terminal) => terminal.id === 'record-live')).toMatchObject({
      ptyId: 'pty-live',
      shell: 'bash',
      cwd: '/workspace/live',
      healthStatus: 'running',
      resumeCursor: 17,
      claim: 'memory-only-resume-grant'
    })
    expect(terminals.find((terminal) => terminal.id === 'record-denied')).toMatchObject({
      ptyId: 'pty-denied',
      healthStatus: 'disconnected',
      claim: undefined
    })

    const root = useWorkspaceStore.getState().root
    expect(root).toMatchObject({
      type: 'leaf',
      id: 'leaf-cold',
      activeTabId: 'term-record-live',
      tabs: [
        { type: 'terminal', id: 'term-record-live', terminalId: 'record-live' },
        { type: 'terminal', id: 'term-record-denied', terminalId: 'record-denied' }
      ]
    })
    expect(logMock).toHaveBeenCalledWith(
      expect.objectContaining({
        source: 'session-workspace-sync.terminal-resume',
        message: expect.stringContaining('code=UNAUTHORIZED')
      })
    )
  })

  it('keeps a disconnected placeholder when the resume boundary throws', async () => {
    const coldWorkspace = workspace(one, 5, 'leaf-network')
    if (coldWorkspace.topology?.type !== 'leaf') {
      throw new Error('expected leaf')
    }
    coldWorkspace.topology.terminalIds = ['record-network']
    coldWorkspace.resources = [
      {
        kind: 'terminal',
        terminalId: 'pty-network',
        terminalRecordId: 'record-network',
        conversationId: one
      }
    ]
    getMock.mockResolvedValue({
      success: true,
      data: { status: 'loaded', workspace: coldWorkspace }
    })
    terminalResumeMock.mockRejectedValue(new Error('transport exploded with private details'))

    await expect(loadSessionWorkspace(one)).resolves.toBe(true)

    expect(useTerminalStore.getState().terminals).toEqual([
      expect.objectContaining({
        id: 'record-network',
        ptyId: 'pty-network',
        healthStatus: 'disconnected',
        claim: undefined
      })
    ])
    expect(useWorkspaceStore.getState().root).toMatchObject({
      type: 'leaf',
      tabs: [{ type: 'terminal', id: 'term-record-network', terminalId: 'record-network' }]
    })
    expect(logMock).toHaveBeenCalledWith(
      expect.objectContaining({ message: expect.stringContaining('code=NETWORK_ERROR') })
    )
    expect(JSON.stringify(useTerminalStore.getState().terminals)).not.toContain(
      'transport exploded with private details'
    )
  })

  it('deduplicates concurrent resume attempts and reuses the in-memory replay cursor', async () => {
    useTerminalStore.getState().setTerminals([
      {
        id: 'record-single-flight',
        conversationId: one,
        ptyId: 'pty-single-flight',
        name: 'Restored terminal',
        projectId: '',
        shell: 'bash',
        healthStatus: 'disconnected',
        resumeCursor: 33
      }
    ])
    let resolveResume:
      | ((value: {
          success: true
          data: {
            terminal: {
              id: string
              shell: string
              cwd: string
              pid: number
              cols: number
              rows: number
              latestSeq: number
              gap: boolean
            }
            claim: string
          }
        }) => void)
      | undefined
    terminalResumeMock.mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveResume = resolve
        })
    )

    const first = useTerminalStore.getState().resumeTerminalResource('record-single-flight')
    const second = useTerminalStore.getState().resumeTerminalResource('record-single-flight')

    expect(second).toBe(first)
    expect(terminalResumeMock).toHaveBeenCalledTimes(1)
    expect(terminalResumeMock).toHaveBeenCalledWith({
      conversationId: one,
      terminalId: 'pty-single-flight',
      lastSeq: 33
    })
    resolveResume?.({
      success: true,
      data: {
        terminal: {
          id: 'pty-single-flight',
          shell: 'bash',
          cwd: '/workspace/single-flight',
          pid: 19,
          cols: 80,
          rows: 24,
          latestSeq: 41,
          gap: false
        },
        claim: 'single-flight-memory-grant'
      }
    })

    // Both callers of the single flight see the same host replay coverage —
    // it is what lets each decide whether its transcript is redundant.
    await expect(Promise.all([first, second])).resolves.toEqual([
      { success: true, data: { latestSeq: 41, gap: false } },
      { success: true, data: { latestSeq: 41, gap: false } }
    ])
    expect(useTerminalStore.getState().terminals[0]).toMatchObject({
      healthStatus: 'running',
      resumeCursor: 41,
      claim: 'single-flight-memory-grant'
    })
    expect(terminalSpawnMock).not.toHaveBeenCalled()
    expect(terminalTerminateMock).not.toHaveBeenCalled()
  })

  it('serializes only exact Conversation-bound terminal refs and referenced editors', () => {
    const root = useWorkspaceStore.getState().root
    if (root.type !== 'leaf') throw new Error('expected leaf')
    useWorkspaceStore.setState({
      root: {
        ...root,
        tabs: [
          { type: 'terminal', id: 'term-t-one', terminalId: 't-one' },
          { type: 'terminal', id: 'term-t-two', terminalId: 't-two' }
        ]
      }
    })
    useTerminalStore.setState({
      terminals: [
        {
          id: 't-one',
          ptyId: 'pty-one',
          projectId: 'same-project',
          shell: 'bash',
          name: 'one',
          conversationId: one
        },
        {
          id: 't-two',
          ptyId: 'pty-two',
          projectId: 'same-project',
          shell: 'bash',
          name: 'two',
          conversationId: two
        }
      ] as never
    })
    const value = buildSessionWorkspace(one)
    expect(value.resources).toEqual([
      {
        kind: 'terminal',
        terminalId: 'pty-one',
        terminalRecordId: 't-one',
        conversationId: one
      }
    ])
    expect(value.topology).toMatchObject({ type: 'leaf', terminalIds: ['t-one'] })
    expect(JSON.stringify(value)).not.toMatch(/claim|envVars|credentials/i)
  })

  it('drops a closed unresumable placeholder from the manifest but keeps a healthy hidden terminal', () => {
    // The manifest is rebuilt from the live terminal list, and
    // `reconcileTerminalResources` re-materializes every listed resource. So a
    // closed record whose resume was refused must leave the manifest, or the
    // phantom "Restored terminal" tab comes back on every sync. A hidden but
    // healthy terminal is a live PTY the user can reopen and has to survive.
    useTerminalStore.setState({
      terminals: [
        {
          id: 't-denied',
          ptyId: 'pty-denied',
          projectId: 'same-project',
          shell: 'bash',
          name: 'Restored terminal',
          conversationId: one,
          isHidden: true,
          healthStatus: 'disconnected'
        },
        {
          id: 't-hidden-alive',
          ptyId: 'pty-hidden-alive',
          projectId: 'same-project',
          shell: 'bash',
          name: 'hidden but running',
          conversationId: one,
          isHidden: true,
          healthStatus: 'running'
        }
      ] as never
    })

    const value = buildSessionWorkspace(one)

    expect(value.resources).toEqual([
      {
        kind: 'terminal',
        terminalId: 'pty-hidden-alive',
        terminalRecordId: 't-hidden-alive',
        conversationId: one
      }
    ])
  })

  it('reads only ConversationStore authority and never treats an ACP SessionId as identity', () => {
    useAcpStore.setState({ activeSessionId: one })
    expect(getActiveConversationId()).toBeNull()

    useConversationStore.getState().setActiveConversationId(two)
    expect(getActiveConversationId()).toBe(two)
  })

  it('routes a typed HTTP 409 Conflict outcome to Conversation conflict state', async () => {
    vi.useFakeTimers()
    writeMock.mockResolvedValue({
      success: true,
      data: {
        status: 'conflict',
        currentRevision: 6,
        currentUpdatedAtUtc: '2026-08-15T10:00:00.000Z',
        currentUpdateIdentity: 'other'
      }
    })
    useConversationStore.getState().setActiveConversationId(one)
    useSessionWorkspaceSyncStore.getState().setBasedRevision(one, 4)
    const { unmount } = renderHook(() => useSessionWorkspaceSync(one))
    act(() => useWorkspaceStore.setState({ activePaneId: 'changed' }))
    expect(writeMock).not.toHaveBeenCalled()
    await act(async () => vi.advanceTimersByTime(500))
    expect(writeMock).toHaveBeenCalledTimes(1)
    expect(useSessionWorkspaceSyncStore.getState().getConflict(one)).toMatchObject({
      conversationId: one,
      currentRevision: 6
    })
    expect(useSessionWorkspaceSyncStore.getState().getConflict(two)).toBeNull()
    expect(logMock).not.toHaveBeenCalled()
    unmount()
  })

  it('resolves reload and overwrite without changing another Conversation base', async () => {
    useConversationStore.getState().setActiveConversationId(one)
    const store = useSessionWorkspaceSyncStore.getState()
    store.setBasedRevision(one, 2)
    store.setBasedRevision(two, 9)
    store.setConflict(one, {
      conversationId: one,
      currentRevision: 5,
      currentUpdatedAtUtc: '2026-08-15T10:00:00.000Z'
    })
    writeMock.mockResolvedValue({
      success: true,
      data: { status: 'updated', revision: 6, updatedAtUtc: '2026-08-15T10:00:01.000Z' }
    })
    await resolveSessionWorkspaceConflict(one, 'overwrite')
    expect(writeMock).toHaveBeenCalledWith(one, 5, expect.any(Object))
    expect(store.getBasedRevision(one)).toBe(6)
    expect(store.getBasedRevision(two)).toBe(9)
  })

  it('surfaces recovery-required and sends exact shared action CAS fields', async () => {
    const item = {
      recoveryId: 'a'.repeat(64),
      kind: 'ambiguous_workspace_manifest' as const,
      severity: 'warning' as const,
      sourcePaths: ['legacy/workspace.json'],
      conversationIds: [one, two],
      sourceSha256: ['e'.repeat(64)],
      candidateFacts: [],
      provenance: [],
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
    getMock.mockResolvedValue({
      success: true,
      data: { status: 'recoveryRequired', conversationId: one, recoveryItems: [item] }
    })
    expect(await loadSessionWorkspace(one)).toBe(false)
    expect(useSessionWorkspaceSyncStore.getState().getRecoveryItems(one)).toEqual([item])
    recoveryMock.mockResolvedValue({
      success: true,
      data: {
        recoveryId: item.recoveryId,
        action: 'startEmptyWorkspace',
        authorization: 'mutation',
        status: 'resolvedStartedEmpty',
        recoveryRevision: 8,
        workspaceRevision: 1,
        workspaceChanged: true,
        sourcePaths: item.sourcePaths,
        sourceSha256: item.sourceSha256,
        candidateFacts: [],
        provenance: []
      }
    })
    getMock.mockResolvedValueOnce({
      success: true,
      data: { status: 'loaded', workspace: workspace(one, 1, 'empty') }
    })
    await resolveSessionWorkspaceRecovery(one, item, 'startEmptyWorkspace')
    expect(recoveryMock).toHaveBeenCalledWith(
      expect.objectContaining({
        recoveryId: item.recoveryId,
        expectedRevision: 7,
        action: 'startEmptyWorkspace',
        payload: { conversationId: one, expectedWorkspaceRevision: null }
      })
    )
  })

  it('routes a typed HTTP 422 RecoveryRequired outcome without a network-error branch', async () => {
    useConversationStore.getState().setActiveConversationId(one)
    useSessionWorkspaceSyncStore.getState().setBasedRevision(one, 4)
    writeMock.mockResolvedValue({
      success: true,
      data: { status: 'recoveryRequired', recoveryItems: [] }
    })
    expect(await performSessionWorkspaceWrite(one)).toBe('recoveryRequired')
    expect(useSessionWorkspaceSyncStore.getState().getBasedRevision(one)).toBe(4)
    expect(logMock).not.toHaveBeenCalled()
  })
})
