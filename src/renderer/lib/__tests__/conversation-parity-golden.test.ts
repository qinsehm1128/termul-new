import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import type {
  ConversationAggregateMutationOutcome,
  ConversationId,
  ProjectAttachment
} from '@shared/types/conversation.types'
import { parseConversationId } from '@shared/types/conversation.types'
import {
  RECOVERY_ACTION_FIXTURES,
  type ResolveRecoveryItemRequest
} from '@shared/types/conversation-recovery.types'
import { invoke } from '@tauri-apps/api/core'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  _resetAcpTransportForTests,
  _resetRemoteAccessCredentialForTests,
  _setAcpTransportForTests,
  type AcpTransport,
  getRemoteAccessCredential,
  remoteAccessHeaders
} from '@/lib/acp-transport'
import {
  conversationApi,
  createConversationFacadeApi,
  webConversationApi
} from '@/lib/conversation-api'
import {
  ConversationLifecycleApiError,
  conversationLifecycleApi,
  createConversationLifecycleApi
} from '@/lib/conversation-lifecycle-api'
import { sessionWorkspaceApi } from '@/lib/session-workspace-api'
import { normalizeConversationError, tauriConversationApi } from '@/lib/tauri-conversation-api'
import { tauriSessionWorkspaceApi } from '@/lib/tauri-session-workspace-api'
import { webSessionWorkspaceApi } from '@/lib/web-session-workspace-api'

const ID = parseConversationId('018f7a1c-1b4d-7c8a-9f01-0123456789ab')
const ACCESS_TOKEN = 'memory-only-access-credential'

function response(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' }
  })
}

function workspace(conversationId: ConversationId = ID) {
  return {
    schemaVersion: 1 as const,
    conversationId,
    revision: 0,
    updatedAtUtc: '',
    resources: [],
    projectionState: { status: 'native' as const }
  }
}

const attachment: ProjectAttachment = {
  schemaVersion: 1,
  projectId: 'project-1',
  attachedAtUtc: '2026-08-15T10:00:00.000Z',
  projectPathSnapshot: '/projects/termul',
  worktreePath: null,
  worktreeBranch: null
}

function aggregateOutcome(
  action: ConversationAggregateMutationOutcome['action'],
  previousRevision: number,
  projectAttachment: ProjectAttachment | null,
  executionTarget: ConversationAggregateMutationOutcome['executionTarget']
): ConversationAggregateMutationOutcome {
  const identity = {
    conversationId: ID,
    createdAtUtc: '2026-08-15T09:45:15.123Z',
    creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
    workspaceCwd: '/visible/conversation'
  }
  return {
    status: 'updated',
    action,
    conversationId: ID,
    previousRevision,
    revision: previousRevision + 1,
    identityBefore: identity,
    identityAfter: identity,
    projectAttachment,
    executionTarget,
    conversation: {
      schemaVersion: 2,
      ...identity,
      projectAttachment,
      executionTarget,
      lifecycleState: 'ready',
      lastSeq: previousRevision + 1,
      createdBy: 'termul'
    }
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  _resetAcpTransportForTests()
  _resetRemoteAccessCredentialForTests()
  window.localStorage.clear()
  window.sessionStorage.clear()
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__
  window.history.replaceState(null, '', '/')
})

afterEach(() => {
  _resetAcpTransportForTests()
  _resetRemoteAccessCredentialForTests()
  vi.unstubAllGlobals()
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__
  window.history.replaceState(null, '', '/')
})

describe('Conversation production transport golden parity', () => {
  it('pins the exact Tauri core singleton request names, payload casing, and stable failures', async () => {
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === 'conversation_resolve_legacy_id') {
        expect(args).toEqual({
          request: { sourceKind: 'legacyStorageKey', value: 'legacy-one' }
        })
        return {
          success: true,
          data: { conversationId: ID, canonicalRoute: `#/c/${ID}` }
        }
      }
      if (command === 'conversation_open') {
        expect(args).toEqual({ conversationId: ID })
        return {
          success: false,
          code: 'CONVERSATION_RECOVERY_REQUIRED',
          error: 'recovery required'
        }
      }
      return { success: true, data: [] }
    })

    await expect(tauriConversationApi.listConversations()).resolves.toEqual({
      success: true,
      data: []
    })
    await expect(
      tauriConversationApi.resolveLegacyConversationId({
        sourceKind: 'legacyStorageKey',
        value: 'legacy-one'
      })
    ).resolves.toEqual({
      success: true,
      data: { conversationId: ID, canonicalRoute: `#/c/${ID}` }
    })
    await expect(tauriConversationApi.openConversation(ID)).resolves.toEqual({
      success: false,
      code: 'CONVERSATION_RECOVERY_REQUIRED',
      error: 'recovery required'
    })
  })

  it.each([
    [400, 'VALIDATION_ERROR'],
    [401, 'UNAUTHORIZED'],
    [403, 'FORBIDDEN'],
    [404, 'CONVERSATION_NOT_FOUND'],
    [409, 'CONVERSATION_CONFLICT'],
    [422, 'CONVERSATION_RECOVERY_REQUIRED'],
    [503, 'CONVERSATION_SERVICE_UNAVAILABLE']
  ])('preserves authenticated HTTP status %s with stable code %s', async (status, code) => {
    window.history.replaceState(null, '', `/#access_token=${ACCESS_TOKEN}`)
    const fetchMock = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      expect(new Headers(init?.headers).get('authorization')).toBe(`Bearer ${ACCESS_TOKEN}`)
      return response({ success: false, code, error: `stable:${code}` }, status)
    })
    vi.stubGlobal('fetch', fetchMock)

    await expect(webConversationApi.listConversations()).resolves.toEqual({
      success: false,
      code,
      error: `stable:${code}`
    })
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })

  it('uses one exact HTTP envelope policy across core, workspace, recovery, and lifecycle domains', async () => {
    _setAcpTransportForTests({
      onEvent: vi.fn(() => vi.fn()),
      dispose: vi.fn()
    } as unknown as AcpTransport)
    const lifecycle = createConversationLifecycleApi('web')
    const lifecycleOutcome = {
      status: 'updated' as const,
      action: 'detachBinding' as const,
      conversationId: ID,
      previousRevision: 1,
      revision: 2,
      workspaceCwd: '/visible/conversation',
      lifecycleState: 'ready' as const,
      currentBinding: {
        schemaVersion: 1 as const,
        bindingId: 'b2832b54-2ca4-4db4-93fd-f93bf6793114',
        agentSessionId: 'opaque/session',
        runtimeAgentId: 'agent-runtime',
        stableAgentNamespace: 'config:test',
        executionCwd: '/visible/conversation',
        boundAtUtc: '2026-08-15T09:45:16.000Z',
        state: 'detached' as const
      }
    }
    const domains = [
      {
        successData: [],
        invoke: () => webConversationApi.listConversations()
      },
      {
        successData: { status: 'missing' as const, conversationId: ID },
        invoke: () => webSessionWorkspaceApi.getWorkspace(ID)
      },
      {
        successData: lifecycleOutcome,
        invoke: () => lifecycle.detachBinding(ID, 1)
      }
    ] as const

    for (const status of [200, 409, 422, 500]) {
      for (const domain of domains) {
        vi.stubGlobal(
          'fetch',
          vi.fn(async () => response({ success: true, data: domain.successData }, status))
        )
        await expect(domain.invoke()).resolves.toEqual(
          domain === domains[2] ? domain.successData : { success: true, data: domain.successData }
        )

        const applicationFailure = {
          success: false as const,
          code: 'FORBIDDEN',
          error: 'stable failure'
        }
        vi.stubGlobal(
          'fetch',
          vi.fn(async () => response(applicationFailure, status))
        )
        if (domain === domains[2]) {
          await expect(domain.invoke()).rejects.toMatchObject({
            name: 'ConversationLifecycleApiError',
            code: 'FORBIDDEN',
            message: 'stable failure'
          })
        } else {
          await expect(domain.invoke()).resolves.toEqual(applicationFailure)
        }
      }
    }

    const malformedDomains = [
      {
        body: { success: true, data: [{}] },
        invoke: () => webConversationApi.listConversations()
      },
      {
        body: { success: true, data: { status: 'missing', conversationId: ID, extra: true } },
        invoke: () => webSessionWorkspaceApi.getWorkspace(ID)
      },
      {
        body: { success: true, data: { ...lifecycleOutcome, extra: true } },
        invoke: () => lifecycle.detachBinding(ID, 1)
      }
    ] as const
    for (const [index, domain] of malformedDomains.entries()) {
      vi.stubGlobal(
        'fetch',
        vi.fn(async () => response(domain.body, 422))
      )
      if (index === 2) {
        await expect(domain.invoke()).rejects.toMatchObject({ code: 'NETWORK_ERROR' })
      } else {
        await expect(domain.invoke()).resolves.toMatchObject({
          success: false,
          code: 'NETWORK_ERROR'
        })
      }
    }
  })

  it('pins aggregate mutation commands and authenticated HTTP routes with identical outcomes', async () => {
    const attached = aggregateOutcome('attachProject', 4, attachment, { kind: 'workspace' })
    const target = {
      kind: 'project_root' as const,
      projectId: attachment.projectId,
      projectRoot: attachment.projectPathSnapshot
    }
    const retargeted = aggregateOutcome('updateExecutionTarget', 5, attachment, target)
    const detached = aggregateOutcome('detachProject', 6, null, { kind: 'workspace' })
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === 'conversation_attach_project') {
        expect(args).toEqual({ conversationId: ID, expectedRevision: 4, attachment })
        return { success: true, data: attached }
      }
      if (command === 'conversation_update_execution_target') {
        expect(args).toEqual({ conversationId: ID, expectedRevision: 5, executionTarget: target })
        return { success: true, data: retargeted }
      }
      if (command === 'conversation_detach_project') {
        expect(args).toEqual({ conversationId: ID, expectedRevision: 6 })
        return { success: true, data: detached }
      }
      throw new Error(`unexpected command ${command}`)
    })

    await expect(tauriConversationApi.attachProject(ID, 4, attachment)).resolves.toEqual({
      success: true,
      data: attached
    })
    await expect(tauriConversationApi.updateExecutionTarget(ID, 5, target)).resolves.toEqual({
      success: true,
      data: retargeted
    })
    await expect(tauriConversationApi.detachProject(ID, 6)).resolves.toEqual({
      success: true,
      data: detached
    })

    window.history.replaceState(null, '', `/#access_token=${ACCESS_TOKEN}`)
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      expect(new Headers(init?.headers).get('authorization')).toBe(`Bearer ${ACCESS_TOKEN}`)
      expect(init?.method).toBe('POST')
      const url = String(input)
      if (url.endsWith(`/conversations/${ID}/attach-project`)) {
        expect(init?.body).toBe(JSON.stringify({ expectedRevision: 4, attachment }))
        return response({ success: true, data: attached })
      }
      if (url.endsWith(`/conversations/${ID}/execution-target`)) {
        expect(init?.body).toBe(JSON.stringify({ expectedRevision: 5, executionTarget: target }))
        return response({ success: true, data: retargeted })
      }
      if (url.endsWith(`/conversations/${ID}/detach-project`)) {
        expect(init?.body).toBe(JSON.stringify({ expectedRevision: 6 }))
        return response({ success: true, data: detached })
      }
      throw new Error(`unexpected request ${url}`)
    })
    vi.stubGlobal('fetch', fetchMock)

    await expect(webConversationApi.attachProject(ID, 4, attachment)).resolves.toEqual({
      success: true,
      data: attached
    })
    await expect(webConversationApi.updateExecutionTarget(ID, 5, target)).resolves.toEqual({
      success: true,
      data: retargeted
    })
    await expect(webConversationApi.detachProject(ID, 6)).resolves.toEqual({
      success: true,
      data: detached
    })
    expect(fetchMock).toHaveBeenCalledTimes(3)
  })

  it('routes the production web compatibility facade through exact specialized singletons', async () => {
    window.history.replaceState(null, '', `/#access_token=${ACCESS_TOKEN}`)
    const lifecycle = vi.fn(async () => ({
      status: 'blocked' as const,
      action: 'deleteConversation' as const,
      conversationId: ID,
      revision: 7,
      code: 'CONVERSATION_LIVE_RESOURCES' as const,
      blockers: [{ kind: 'terminalResources' as const, count: 1, ids: ['terminal-live'] }]
    }))
    _setAcpTransportForTests({
      conversationLifecycle: lifecycle,
      onEvent: vi.fn(() => vi.fn()),
      dispose: vi.fn()
    } as unknown as AcpTransport)

    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      expect(new Headers(init?.headers).get('authorization')).toBe(`Bearer ${ACCESS_TOKEN}`)
      if (url.endsWith('/conversations')) {
        return response({ success: true, data: [] })
      }
      if (url.endsWith(`/conversations/${ID}/workspace`)) {
        expect(init?.method).toBe('POST')
        expect(init?.body).toBe(JSON.stringify({ basedRevision: null, workspace: workspace() }))
        return response({ success: false, code: 'FORBIDDEN', error: 'localhost-only' }, 403)
      }
      throw new Error(`unexpected request ${url}`)
    })
    vi.stubGlobal('fetch', fetchMock)

    expect(conversationApi.listConversations).not.toBe(webConversationApi.listConversations)
    expect(conversationApi.getWorkspace).not.toBe(sessionWorkspaceApi.getWorkspace)
    await expect(conversationApi.listConversations()).resolves.toEqual({ success: true, data: [] })
    await expect(conversationApi.writeWorkspace(ID, null, workspace())).resolves.toEqual({
      success: false,
      code: 'FORBIDDEN',
      error: 'localhost-only'
    })
    await expect(conversationApi.deleteConversation(ID, 7)).resolves.toMatchObject({
      status: 'blocked',
      code: 'CONVERSATION_LIVE_RESOURCES'
    })
    expect(lifecycle).toHaveBeenCalledWith('delete', ID, 7, undefined)
  })

  it('dispatches all five web lifecycle mutations through the real production factory', async () => {
    const lifecycle = vi.fn(
      async (
        action: 'detach' | 'rebind' | 'suspend' | 'replace' | 'delete',
        conversationId: string,
        expectedRevision: number
      ) => ({
        status: 'updated' as const,
        action:
          action === 'detach'
            ? ('detachBinding' as const)
            : action === 'rebind'
              ? ('rebindDetachedBinding' as const)
              : action === 'suspend'
                ? ('suspendBinding' as const)
                : action === 'replace'
                  ? ('replaceBinding' as const)
                  : ('deleteConversation' as const),
        conversationId: parseConversationId(conversationId),
        previousRevision: expectedRevision,
        revision: expectedRevision + 1,
        workspaceCwd: '/visible/conversation',
        lifecycleState: action === 'delete' ? ('deleted' as const) : ('ready' as const),
        currentBinding: null
      })
    )
    _setAcpTransportForTests({
      conversationLifecycle: lifecycle,
      onEvent: vi.fn(() => vi.fn()),
      dispose: vi.fn()
    } as unknown as AcpTransport)

    const api = createConversationLifecycleApi('web')
    const replacement = {
      schemaVersion: 1 as const,
      conversationId: ID,
      executionTarget: { kind: 'workspace' as const }
    }
    await api.detachBinding(ID, 1)
    await api.rebindDetachedBinding(ID, 2)
    await api.suspendBinding(ID, 3)
    await api.replaceBinding(ID, replacement, 4)
    await api.deleteConversation(ID, 5)

    expect(lifecycle.mock.calls).toEqual([
      ['detach', ID, 1, undefined],
      ['rebind', ID, 2, undefined],
      ['suspend', ID, 3, undefined],
      ['replace', ID, 4, replacement],
      ['delete', ID, 5, undefined]
    ])
  })

  it('passes workspace success/conflict and lifecycle resource outcomes unchanged on Tauri', async () => {
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {}
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'session_workspace_get') {
        return { success: true, data: { status: 'missing', conversationId: ID } }
      }
      if (command === 'session_workspace_write') {
        return {
          success: true,
          data: {
            status: 'conflict',
            currentRevision: 7,
            currentUpdatedAtUtc: '2026-08-15T10:00:00.000Z',
            currentUpdateIdentity: 'browser-b'
          }
        }
      }
      if (command === 'conversation_delete') {
        return {
          success: true,
          data: {
            status: 'blocked',
            action: 'deleteConversation',
            conversationId: ID,
            revision: 7,
            code: 'CONVERSATION_LIVE_RESOURCES',
            blockers: [{ kind: 'terminalResources', count: 1, ids: ['terminal-live'] }]
          }
        }
      }
      return { success: false, code: 'CONVERSATION_RECOVERY_REQUIRED', error: 'recovery' }
    })

    await expect(tauriSessionWorkspaceApi.getWorkspace(ID)).resolves.toMatchObject({
      success: true,
      data: { status: 'missing', conversationId: ID }
    })
    await expect(
      tauriSessionWorkspaceApi.writeWorkspace(ID, 6, workspace())
    ).resolves.toMatchObject({
      success: true,
      data: { status: 'conflict', currentRevision: 7 }
    })
    await expect(
      createConversationLifecycleApi('tauri').deleteConversation(ID, 7)
    ).resolves.toMatchObject({
      status: 'blocked',
      code: 'CONVERSATION_LIVE_RESOURCES',
      blockers: [{ kind: 'terminalResources', ids: ['terminal-live'] }]
    })
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__
  })

  it.each([
    '018F7A1C-1B4D-7C8A-9F01-0123456789AB',
    '018f7a1c1b4d7c8a9f010123456789ab',
    '018f7a1c-1b4d-7c8a-9f01-0123456789ab/child',
    ' 018f7a1c-1b4d-7c8a-9f01-0123456789ab'
  ])('rejects malformed ids before every production transport dispatch: %s', async (value) => {
    const malformed = value as ConversationId
    const lifecycle = vi.fn()
    _setAcpTransportForTests({
      conversationLifecycle: lifecycle,
      onEvent: vi.fn(() => vi.fn()),
      dispose: vi.fn()
    } as unknown as AcpTransport)
    const fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)

    await expect(tauriConversationApi.openConversation(malformed)).resolves.toMatchObject({
      success: false,
      code: 'CONVERSATION_INVALID_ID'
    })
    await expect(webConversationApi.openConversation(malformed)).resolves.toMatchObject({
      success: false,
      code: 'CONVERSATION_INVALID_ID'
    })
    await expect(tauriSessionWorkspaceApi.getWorkspace(malformed)).resolves.toMatchObject({
      success: false,
      code: 'CONVERSATION_INVALID_ID'
    })
    await expect(webSessionWorkspaceApi.getWorkspace(malformed)).resolves.toMatchObject({
      success: false,
      code: 'CONVERSATION_INVALID_ID'
    })
    await expect(
      createConversationLifecycleApi('tauri').detachBinding(malformed, 0)
    ).rejects.toMatchObject({ code: 'VALIDATION_ERROR' })
    await expect(
      createConversationLifecycleApi('web').detachBinding(malformed, 0)
    ).rejects.toMatchObject({ code: 'VALIDATION_ERROR' })

    expect(invoke).not.toHaveBeenCalled()
    expect(fetchMock).not.toHaveBeenCalled()
    expect(lifecycle).not.toHaveBeenCalled()
  })

  it('consumes the access fragment once, clears it, and retains only in-memory bearer state', () => {
    const localWrite = vi.spyOn(Storage.prototype, 'setItem')
    window.history.replaceState(null, '', `/conversation?view=active#access_token=${ACCESS_TOKEN}`)

    expect(getRemoteAccessCredential()).toBe(ACCESS_TOKEN)
    expect(window.location.hash).toBe('')
    expect(window.location.search).toBe('?view=active')
    expect(remoteAccessHeaders().get('authorization')).toBe(`Bearer ${ACCESS_TOKEN}`)
    expect(getRemoteAccessCredential()).toBe(ACCESS_TOKEN)
    expect(localWrite).not.toHaveBeenCalled()
    expect(window.localStorage.length).toBe(0)
    expect(window.sessionStorage.length).toBe(0)
  })

  it('preserves release-stable application codes without transport remapping', () => {
    const stableCodes = [
      'CONVERSATION_INVALID_ID',
      'CONVERSATION_NOT_FOUND',
      'CONVERSATION_CONFLICT',
      'CONVERSATION_RECOVERY_REQUIRED',
      'CONVERSATION_LIVE_RESOURCES',
      'CONVERSATION_BINDING_NOT_FOUND',
      'CONVERSATION_BINDING_NOT_ACTIVE',
      'CONVERSATION_BINDING_NOT_DETACHED',
      'CONVERSATION_BINDING_NOT_ADDRESSABLE',
      'CONVERSATION_DURABILITY_FAILED',
      'ACP_COMPENSATION_FAILED',
      'LEGACY_COMPATIBILITY_READ_ONLY',
      'LEGACY_ID_AMBIGUOUS',
      'MIGRATION_IDEMPOTENCY_CONFLICT',
      'VALIDATION_ERROR',
      'FORBIDDEN',
      'UNAUTHORIZED'
    ]
    for (const code of stableCodes) {
      expect(normalizeConversationError({ code, message: `stable:${code}` })).toEqual({
        success: false,
        code,
        error: `stable:${code}`
      })
    }
  })

  it('parses actionable compensation receipts without exposing provider detail', () => {
    const receipt = {
      conversationId: ID,
      primaryCode: 'CONVERSATION_DURABILITY_FAILED',
      providerCloseCode: 'ACP_CLOSE_FAILED',
      failureRecordCode: 'CONVERSATION_DURABILITY_FAILED',
      recoveryId: 'a'.repeat(64)
    }
    const error = new ConversationLifecycleApiError(
      'ACP_COMPENSATION_FAILED',
      JSON.stringify(receipt)
    )
    expect(error.compensation).toEqual(receipt)
    expect(error.message).not.toContain('SUPER_SECRET')
  })

  it('keeps the compatibility facade as zero-logic delegation', async () => {
    const core = {
      getHostStatus: vi.fn(),
      listConversations: vi.fn(async () => ({ success: true as const, data: [] })),
      getConversation: vi.fn(),
      getCurrentBinding: vi.fn(),
      openConversation: vi.fn(),
      resolveLegacyConversationId: vi.fn(),
      attachProject: vi.fn(),
      detachProject: vi.fn(),
      updateExecutionTarget: vi.fn(),
      subscribeHostStatus: vi.fn()
    }
    const workspaces = {
      getWorkspace: vi.fn(),
      writeWorkspace: vi.fn(),
      resolveRecovery: vi.fn()
    }
    const lifecycle = {
      detachBinding: vi.fn(),
      rebindDetachedBinding: vi.fn(),
      suspendBinding: vi.fn(),
      replaceBinding: vi.fn(),
      deleteConversation: vi.fn(),
      subscribe: vi.fn()
    }
    const facade = createConversationFacadeApi(core, workspaces, lifecycle)

    await facade.listConversations()
    await facade.attachProject(ID, 4, attachment)
    await facade.detachProject(ID, 5)
    await facade.updateExecutionTarget(ID, 6, { kind: 'workspace' })
    expect(core.listConversations).toHaveBeenCalledTimes(1)
    expect(core.attachProject).toHaveBeenCalledWith(ID, 4, attachment)
    expect(core.detachProject).toHaveBeenCalledWith(ID, 5)
    expect(core.updateExecutionTarget).toHaveBeenCalledWith(ID, 6, { kind: 'workspace' })
    expect(workspaces.getWorkspace).not.toHaveBeenCalled()
    expect(lifecycle.deleteConversation).not.toHaveBeenCalled()
  })

  it('reuses the authoritative RecoveryAction union without local action aliases', () => {
    const request = RECOVERY_ACTION_FIXTURES[1].request as ResolveRecoveryItemRequest
    expect(request.action).toBe('associateConversation')

    const root = join(__dirname, '..', '..', '..', '..')
    const files = [
      'src/shared/types/conversation-api.types.ts',
      'src/renderer/lib/tauri-conversation-api.ts',
      'src/renderer/lib/web-conversation-api.ts',
      'src/renderer/hooks/use-conversation-host-bootstrap.ts',
      'src/renderer/components/conversation/ConversationHostStatus.tsx'
    ]
    for (const relative of files) {
      const source = readFileSync(join(root, relative), 'utf8')
      expect(source, relative).not.toMatch(
        /'inspect'\s*\|\s*'associateConversation'|"inspect"\s*\|\s*"associateConversation"/
      )
    }
    const contract = readFileSync(join(root, 'src/shared/types/conversation-api.types.ts'), 'utf8')
    expect(contract).toContain("from './conversation-recovery.types'")
    expect(contract).toContain('export type {')
  })

  it('pins the production web lifecycle singleton to the same factory contract', () => {
    expect(Object.keys(conversationLifecycleApi).sort()).toEqual(
      Object.keys(createConversationLifecycleApi('web')).sort()
    )
  })
})
