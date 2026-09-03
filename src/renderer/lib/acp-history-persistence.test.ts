import { beforeEach, describe, expect, it, vi } from 'vitest'

const { mockTransport, mockHistoryApi } = vi.hoisted(() => ({
  mockTransport: {
    historyMode: vi.fn(() => 'tauri_store' as const),
    listPersistedSessions: vi.fn(),
    openPersistedSession: vi.fn(),
    getSessionPayload: vi.fn(),
    getSessionPayloadPage: vi.fn()
  },
  mockHistoryApi: {
    list: vi.fn(),
    get: vi.fn(),
    getPage: vi.fn(),
    listLegacy: vi.fn(),
    getLegacy: vi.fn(),
    save: vi.fn(),
    delete: vi.fn(),
    flush: vi.fn(),
    markLegacyImportComplete: vi.fn()
  }
}))

vi.mock('@/lib/acp-transport', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/acp-transport')>()
  return { ...actual, getAcpTransport: () => mockTransport }
})
vi.mock('@/lib/acp-history-api', () => ({ acpHistoryApi: mockHistoryApi }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn() }))
vi.mock('@/lib/api', () => ({
  persistenceApi: {
    read: vi.fn(),
    write: vi.fn(),
    writeDebounced: vi.fn(),
    delete: vi.fn()
  }
}))

import { i18n } from '@/i18n'
import type { ToolCall } from '@/lib/acp-api'
import { persistenceApi } from '@/lib/api'
import type { ChatMessage } from '@/stores/acp-store'
import {
  _clearPayloadCacheForTesting,
  _failedPrefixIdsForTesting,
  _resetHistoryPagingForTesting,
  _resetPendingIndexWriteTrackerForTesting,
  _resumeMetadataForTesting,
  _seedFailedPrefixForTesting,
  deriveTitle,
  disposeFailedPrefixPayloads,
  FAILED_PREFIX_TTL_MS,
  flushSessionHistory,
  fromPersistedSessionSummary,
  getCachedSessionPayload,
  groupSessionsByRecency,
  historyPagingMetrics,
  INACTIVE_PAYLOAD_CACHE_BUDGET,
  loadSessionIndex,
  loadSessionPayload,
  MAX_FAILED_PREFIX_ASSEMBLIES,
  MAX_FAILED_PREFIX_PAYLOAD_BYTES,
  MAX_HISTORY_IN_FLIGHT_BYTES,
  markSessionPayloadPinned,
  maxPayloadSeq,
  normalizeCwdForScope,
  PERSISTED_TOOL_CALL_BYTE_BUDGET,
  PERSISTED_TOOL_CALLS_LIMIT,
  queueSessionPayloadDelete,
  queueSessionPayloadSave,
  RENDERER_HISTORY_PAGE_SIZE,
  restoredToolCalls,
  runHistoryWipeMigration,
  SESSION_INDEX_KEY,
  type SessionIndexEntry,
  type SessionPayload,
  sanitizeToolCallsForPersistence,
  saveSessionPayload,
  scopeSessionIndex,
  sessionPayloadKey,
  setCachedSessionPayload,
  toPersistedSessionSummaries,
  trackPendingIndexWrite,
  unpinSessionPayload,
  waitForPendingSessionIndexWrite
} from './acp-history-persistence'

function msg(role: ChatMessage['role'], text: string): ChatMessage {
  return { id: `m-${text}`, role, blocks: [{ type: 'text', text }], streaming: false, timestamp: 0 }
}

function entry(id: string, overrides: Partial<SessionIndexEntry> = {}): SessionIndexEntry {
  return {
    id,
    agentId: 'agent-1',
    agentConfigId: 'cfg-1',
    title: `Chat ${id}`,
    cwd: '/project',
    projectId: 'project-1',
    createdAt: 1,
    lastActivityAt: 2,
    messageCount: 0,
    status: 'closed',
    ...overrides
  }
}

function payload(id: string, messages: ChatMessage[] = []): SessionPayload {
  return { metadata: entry(id, { messageCount: messages.length }), messages }
}

function historyRecord(sessionId: string, seq: number, type: string, recordPayload: unknown) {
  return {
    schemaVersion: 1 as const,
    sessionId,
    seq,
    type,
    recordedAt: seq,
    payload: recordPayload
  }
}

function historyPage(
  _sessionId: string,
  records: ReturnType<typeof historyRecord>[],
  targetLastSeq: number,
  complete = records.at(-1)?.seq === targetLastSeq
) {
  return {
    schemaVersion: 1 as const,
    records,
    nextCursor: records.at(-1)?.seq ?? targetLastSeq,
    complete,
    targetLastSeq
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  _clearPayloadCacheForTesting()
  _resetPendingIndexWriteTrackerForTesting()
  _resetHistoryPagingForTesting()
  mockTransport.historyMode.mockReturnValue('tauri_store')
  mockTransport.listPersistedSessions.mockResolvedValue([])
  mockTransport.getSessionPayloadPage.mockResolvedValue({
    schemaVersion: 1,
    records: [],
    nextCursor: 0,
    complete: true,
    targetLastSeq: 0
  })
  mockHistoryApi.list.mockResolvedValue({ sessions: [], legacyImportComplete: false })
  mockHistoryApi.get.mockResolvedValue(null)
  mockHistoryApi.getPage.mockResolvedValue({
    schemaVersion: 1,
    records: [],
    nextCursor: 0,
    complete: true,
    targetLastSeq: 0
  })
  mockHistoryApi.listLegacy.mockResolvedValue({ sessions: [], legacyImportComplete: false })
  mockHistoryApi.getLegacy.mockResolvedValue(null)
  mockHistoryApi.save.mockResolvedValue(undefined)
  mockHistoryApi.delete.mockResolvedValue(undefined)
  mockHistoryApi.flush.mockResolvedValue(undefined)
  mockHistoryApi.markLegacyImportComplete.mockResolvedValue(undefined)
  ;(persistenceApi.read as ReturnType<typeof vi.fn>).mockResolvedValue({
    success: false,
    code: 'KEY_NOT_FOUND',
    error: 'not found'
  })
  ;(persistenceApi.write as ReturnType<typeof vi.fn>).mockResolvedValue({ success: true })
  ;(persistenceApi.delete as ReturnType<typeof vi.fn>).mockResolvedValue({ success: true })
})

describe('pure history helpers', () => {
  it('derives, truncates, and falls back for titles', () => {
    expect(deriveTitle([msg('agent', 'hi'), msg('user', 'Refactor auth')], 'fallback')).toBe(
      'Refactor auth'
    )
    expect(deriveTitle([msg('user', 'x'.repeat(60))], 'fallback')).toBe(`${'x'.repeat(48)}…`)
    expect(deriveTitle([msg('user', '😀'.repeat(60))], 'fallback')).toBe(`${'😀'.repeat(48)}…`)
    expect(deriveTitle([msg('user', 'First line\nSecond line')], 'fallback')).toBe('First line')
    expect(deriveTitle([msg('agent', 'hello')], 'fallback')).toBe('fallback')
  })

  it('localizes a missing persisted session title at conversion time', async () => {
    const previousLanguage = i18n.language
    const summary = {
      storageKey: 'session:test',
      sessionId: 'test',
      stableAgentNamespace: null,
      cwd: '/project',
      title: null,
      createdAt: 1,
      lastActivityAt: 2,
      status: 'closed' as const,
      messageCount: 0,
      toolCount: 0,
      lastSeq: 0,
      resumeEligible: false
    }
    try {
      await i18n.changeLanguage('en')
      expect(fromPersistedSessionSummary(summary).title).toBe('Untitled Chat')
      await i18n.changeLanguage('zh-CN')
      expect(fromPersistedSessionSummary(summary).title).toBe('未命名对话')
    } finally {
      await i18n.changeLanguage(previousLanguage)
    }
  })

  it('groups by recency and scopes by project/cwd with fallback', () => {
    const now = new Date('2026-05-30T12:00:00').getTime()
    const groups = groupSessionsByRecency(
      [
        entry('today', { lastActivityAt: new Date('2026-05-30T11:00:00').getTime() }),
        entry('yesterday', { lastActivityAt: new Date('2026-05-29T11:00:00').getTime() }),
        entry('old', { lastActivityAt: new Date('2026-05-01T11:00:00').getTime() })
      ],
      now
    )
    expect(groups.map(({ group }) => group)).toEqual(['Today', 'Yesterday', 'Earlier'])

    const entries = [
      entry('exact', { cwd: '/a' }),
      entry('other-cwd', { cwd: '/b' }),
      entry('other-project', { projectId: 'project-2', cwd: '/a' })
    ]
    expect(scopeSessionIndex(entries, 'project-1', '/a').map(({ id }) => id)).toEqual(['exact'])
    expect(scopeSessionIndex(entries, 'project-1', '/missing').map(({ id }) => id)).toEqual([
      'exact',
      'other-cwd'
    ])
    expect(scopeSessionIndex(entries, '', '/a')).toEqual([])
  })

  it('scopes with worktree-inclusive reachability from the root view', () => {
    // Root view (activeWorktreeId=null): activeCwd is the project root. The
    // sidebar must list root-cwd sessions AND the project's worktree-cwd
    // sessions so a worktree chat stays reachable without a restart.
    const root = entry('root', { cwd: '/project' })
    const wtChat = entry('wt-chat', { cwd: '/project/.se-manager/worktrees/wt' })
    const otherProjectWt = entry('other-wt', {
      projectId: 'project-2',
      cwd: '/project/.se-manager/worktrees/wt'
    })
    const entries = [root, wtChat, otherProjectWt]

    expect(
      scopeSessionIndex(entries, 'project-1', '/project', [
        '/project/.se-manager/worktrees/wt'
      ]).map(({ id }) => id)
    ).toEqual(['root', 'wt-chat'])
  })

  it('keeps the active worktree session visible while it is active', () => {
    // activeCwd is the worktree path: exact-cwd match returns the worktree
    // chat; the root-cwd chat is not in the worktreePath set so it is hidden,
    // matching the prior scoped-to-active-cwd behavior.
    const root = entry('root', { cwd: '/project' })
    const wtChat = entry('wt-chat', { cwd: '/project/.se-manager/worktrees/wt' })
    const entries = [root, wtChat]

    expect(
      scopeSessionIndex(entries, 'project-1', '/project/.se-manager/worktrees/wt', [
        '/project/.se-manager/worktrees/wt'
      ]).map(({ id }) => id)
    ).toEqual(['wt-chat'])
  })

  it('does not surface another project worktree cwd and still falls back when empty', () => {
    // A worktree path of a DIFFERENT project must not leak into the active
    // project's scoping; and the worktree-inclusive set never duplicates an
    // entry already matched by exact-cwd (filter, not concat).
    const root = entry('root', { cwd: '/project' })
    const wtChat = entry('wt-chat', { cwd: '/project/.se-manager/worktrees/wt' })
    const foreignWt = entry('foreign', {
      projectId: 'project-2',
      cwd: '/other/.se-manager/worktrees/x'
    })
    const entries = [root, wtChat, foreignWt]

    // Foreign worktree path is not in the active project's worktreePaths.
    expect(
      scopeSessionIndex(entries, 'project-1', '/project', [
        '/project/.se-manager/worktrees/wt'
      ]).map(({ id }) => id)
    ).toEqual(['root', 'wt-chat'])

    // projectId-only fallback when the scoped set (exact + worktree) is empty.
    expect(
      scopeSessionIndex(entries, 'project-2', '/nowhere', [
        '/project/.se-manager/worktrees/wt'
      ]).map(({ id }) => id)
    ).toEqual(['foreign'])
  })

  it('normalizes Windows verbatim-prefix + separator forms before scoping', () => {
    // Real runtime shapes on Windows: the host persists session cwds via Rust
    // `Path::canonicalize` (verbatim `\\?\` prefix, backslashes), while the
    // project store's worktree paths come from `worktreeApi.list` (forward
    // slashes, no prefix). A raw `===`/`Set.has` would never equate them and
    // the worktree chat would be hidden from the root view. Root sessions also
    // appear in BOTH forms (471 unprefixed + 94 prefixed in real data).
    const rootUnprefixed = entry('root-unprefixed', { cwd: 'E:\\project' })
    const rootPrefixed = entry('root-prefixed', { cwd: '\\\\?\\E:\\project' })
    const wtChat = entry('wt-chat', { cwd: '\\\\?\\E:\\project\\.se-manager\\worktrees\\wt' })
    const entries = [rootUnprefixed, rootPrefixed, wtChat]

    // Root view: activeCwd is the project root (store form, backslashes);
    // worktreePaths is the store form (forward slashes, no prefix). Both root
    // forms AND the worktree chat must be listed.
    expect(
      scopeSessionIndex(entries, 'project-1', 'E:\\project', [
        'E:/project/.se-manager/worktrees/wt'
      ])
        .map(({ id }) => id)
        .sort()
    ).toEqual(['root-prefixed', 'root-unprefixed', 'wt-chat'])

    // Active-worktree view: activeCwd is the store worktree path (forward
    // slashes); the prefixed-backslash session cwd must still match exactly.
    expect(
      scopeSessionIndex(entries, 'project-1', 'E:/project/.se-manager/worktrees/wt', [
        'E:/project/.se-manager/worktrees/wt'
      ]).map(({ id }) => id)
    ).toEqual(['wt-chat'])
  })

  it('normalizes extended UNC verbatim prefix to match standard UNC paths', () => {
    // Windows extended UNC verbatim prefix `\\?\UNC\server\share\…` and the
    // standard UNC `\\server\share\…` must produce the same scope value so a
    // UNC-rooted worktree chat is reachable regardless of which form the host
    // canonicalized the cwd into.
    const extended = '\\\\?\\UNC\\server\\share\\wt'
    const standard = '\\\\server\\share\\wt'
    expect(normalizeCwdForScope(extended)).toBe(normalizeCwdForScope(standard))

    // Scoping matches an extended-UNC session cwd against a standard-UNC
    // active cwd / worktree path from the root view.
    const wtChat = entry('unc-wt', { cwd: extended })
    expect(
      scopeSessionIndex([wtChat], 'project-1', standard, [standard]).map(({ id }) => id)
    ).toEqual(['unc-wt'])
  })

  it('preserves the existing browser summary wire shape', () => {
    expect(toPersistedSessionSummaries([entry('s-1', { status: 'initializing' })])[0]).toEqual(
      expect.objectContaining({
        sessionId: 's-1',
        stableAgentNamespace: 'config:cfg-1',
        status: 'active',
        resumeEligible: true
      })
    )
  })

  it('surfaces the real persisted lastSeq and degrades to 0 when absent', () => {
    // R3 / parent-spec R2 index-list completeness: the summary must carry the
    // real max message seq when the index entry has it.
    const withSeq = toPersistedSessionSummaries([entry('seq-7', { lastSeq: 7 })])[0]
    expect(withSeq.lastSeq).toBe(7)

    // Absent (old save or Rust index that does not surface it) → 0 (pre-R3).
    const withoutSeq = toPersistedSessionSummaries([entry('seq-0')])[0]
    expect(withoutSeq.lastSeq).toBe(0)
  })
})

describe('durable tool-call sanitization', () => {
  function toolCall(overrides: Partial<ToolCall> = {}): ToolCall {
    return {
      toolCallId: 'tc-1',
      title: 'Read file',
      kind: 'read',
      status: 'completed',
      timestamp: 100,
      seq: 5,
      rawInput: { path: '/a.ts' },
      rawOutput: 'huge output',
      ...overrides
    }
  }

  it('returns undefined for absent or empty lists so the field is omitted', () => {
    expect(sanitizeToolCallsForPersistence(undefined)).toBeUndefined()
    expect(sanitizeToolCallsForPersistence([])).toBeUndefined()
  })

  it('strips rawOutput but keeps summary fields and timeline stamps', () => {
    const [clean] = sanitizeToolCallsForPersistence([toolCall()])!
    expect(clean.rawOutput).toBeUndefined()
    expect(clean).toMatchObject({
      toolCallId: 'tc-1',
      title: 'Read file',
      kind: 'read',
      status: 'completed',
      timestamp: 100,
      seq: 5,
      rawInput: { path: '/a.ts' }
    })
  })

  it('keeps structured content and rawInput within the byte budget', () => {
    const [clean] = sanitizeToolCallsForPersistence([
      toolCall({
        content: [{ type: 'text', text: 'short' }],
        rawInput: { command: 'ls' }
      })
    ])!
    expect(clean.content).toEqual([{ type: 'text', text: 'short' }])
    expect(clean.rawInput).toEqual({ command: 'ls' })
  })

  it('degrades over-budget calls to the structural subset', () => {
    const huge = 'x'.repeat(PERSISTED_TOOL_CALL_BYTE_BUDGET + 1)
    const [clean] = sanitizeToolCallsForPersistence([
      toolCall({ rawInput: { path: '/big.ts', blob: huge } })
    ])!
    expect(clean.rawInput).toBeUndefined()
    expect(clean.content).toBeUndefined()
    expect(clean).toMatchObject({ toolCallId: 'tc-1', title: 'Read file', kind: 'read', seq: 5 })
  })

  it('falls back to the structural subset for non-serializable fields', () => {
    const circular: Record<string, unknown> = {}
    circular.self = circular
    const [clean] = sanitizeToolCallsForPersistence([toolCall({ rawInput: circular })])!
    expect(clean.rawInput).toBeUndefined()
    expect(clean).toMatchObject({ toolCallId: 'tc-1', status: 'completed', seq: 5 })
  })

  it('persists mid-flight statuses as failed so restored cards do not spin forever', () => {
    const pending = sanitizeToolCallsForPersistence([toolCall({ status: 'pending' })])!
    const inProgress = sanitizeToolCallsForPersistence([toolCall({ status: 'in_progress' })])!
    expect(pending[0].status).toBe('failed')
    expect(inProgress[0].status).toBe('failed')
    const completed = sanitizeToolCallsForPersistence([toolCall({ status: 'completed' })])!
    expect(completed[0].status).toBe('completed')
  })

  it('bounds agent-controlled titles so the degraded subset stays bounded', () => {
    const hugeTitle = 't'.repeat(5000)
    const [clean] = sanitizeToolCallsForPersistence([
      toolCall({
        title: hugeTitle,
        rawInput: { blob: 'x'.repeat(PERSISTED_TOOL_CALL_BYTE_BUDGET) }
      })
    ])!
    expect(clean.title!.length).toBeLessThanOrEqual(201)
  })

  it('drops unknown agent fields at the persistence boundary', () => {
    const [clean] = sanitizeToolCallsForPersistence([
      toolCall({ vendorBlob: 'should not survive' })
    ])!
    expect(clean.vendorBlob).toBeUndefined()
    expect(clean.rawOutput).toBeUndefined()
  })

  it('keeps only the most recent calls per session (recency bound)', () => {
    const calls = Array.from({ length: PERSISTED_TOOL_CALLS_LIMIT + 10 }, (_, index) => ({
      toolCallId: `tc-${index}`,
      seq: index + 1
    }))
    const clean = sanitizeToolCallsForPersistence(calls)!
    expect(clean).toHaveLength(PERSISTED_TOOL_CALLS_LIMIT)
    expect(clean[0].toolCallId).toBe('tc-10')
    expect(clean[clean.length - 1].toolCallId).toBe(`tc-${PERSISTED_TOOL_CALLS_LIMIT + 9}`)
  })

  it('tolerates non-array input without throwing', () => {
    expect(sanitizeToolCallsForPersistence('corrupt' as unknown as ToolCall[])).toBeUndefined()
  })

  it('drops calls whose structural subset still exceeds the budget (oversized id)', () => {
    const oversizedId = 'tc-'.concat('x'.repeat(PERSISTED_TOOL_CALL_BYTE_BUDGET + 1))
    const clean = sanitizeToolCallsForPersistence([
      toolCall({ toolCallId: oversizedId }),
      toolCall({ toolCallId: 'tc-ok' })
    ])!
    expect(clean).toHaveLength(1)
    expect(clean[0].toolCallId).toBe('tc-ok')
  })

  it('returns undefined when every call is dropped for exceeding the budget', () => {
    const oversizedId = 'tc-'.concat('x'.repeat(PERSISTED_TOOL_CALL_BYTE_BUDGET + 1))
    expect(sanitizeToolCallsForPersistence([toolCall({ toolCallId: oversizedId })])).toBeUndefined()
  })
})

describe('payload restore helpers', () => {
  it('maxPayloadSeq folds message and tool-call seqs', () => {
    expect(
      maxPayloadSeq({
        messages: [{ id: 'm', role: 'user', blocks: [], streaming: false, timestamp: 0, seq: 3 }],
        toolCalls: [{ toolCallId: 'tc', seq: 7 }]
      })
    ).toBe(7)
    expect(
      maxPayloadSeq({
        messages: [{ id: 'm', role: 'user', blocks: [], streaming: false, timestamp: 0, seq: 9 }],
        toolCalls: [{ toolCallId: 'tc', seq: 2 }]
      })
    ).toBe(9)
  })

  it('restore helpers degrade corrupt toolCalls shapes instead of throwing', () => {
    const corrupt = { toolCalls: 'not-an-array' as unknown as ToolCall[] }
    expect(maxPayloadSeq({ messages: [], ...corrupt })).toBe(0)
    expect(restoredToolCalls(corrupt)).toEqual([])
    expect(restoredToolCalls({})).toEqual([])
  })

  it('restore helpers skip null and malformed entries inside the array', () => {
    const valid = { toolCallId: 'tc-valid', seq: 4 }
    const junk = [
      null,
      42,
      'tc-string',
      { seq: 3 },
      { toolCallId: '' },
      valid
    ] as unknown as ToolCall[]
    expect(maxPayloadSeq({ messages: [], toolCalls: junk })).toBe(4)
    expect(restoredToolCalls({ toolCalls: junk })).toEqual([valid])
  })

  it('maxPayloadSeq ignores non-finite seqs instead of poisoning the rebase', () => {
    const corrupt = {
      toolCalls: [{ toolCallId: 'tc-nan', seq: Number.NaN }] as unknown as ToolCall[]
    }
    expect(maxPayloadSeq({ messages: [], ...corrupt })).toBe(0)
  })
})

describe('progressive bounded history assembly', () => {
  it('assembles the real 50,000-record / 200-page path linearly with bounded snapshots', async () => {
    const sessionId = 'progressive-50k'
    const targetLastSeq = 50_000
    mockHistoryApi.getPage.mockImplementation(
      async (_id, afterSeq: number, limit: number, target?: number) => {
        expect(limit).toBe(RENDERER_HISTORY_PAGE_SIZE)
        expect(target).toBe(afterSeq === 0 ? undefined : targetLastSeq)
        const count = Math.min(limit, targetLastSeq - afterSeq)
        const records = Array.from({ length: count }, (_, index) => {
          const seq = afterSeq + index + 1
          return historyRecord(sessionId, seq, 'user_prompt', {
            turnId: `turn-${seq}`,
            content: [{ type: 'text', text: `message-${seq}` }]
          })
        })
        return historyPage(sessionId, records, targetLastSeq)
      }
    )
    const snapshots: Array<{ loaded: number; messages: number; complete: boolean }> = []
    const progressCounts: number[] = []

    const result = await loadSessionPayload(sessionId, {
      metadata: entry(sessionId, { lastSeq: targetLastSeq, messageCount: targetLastSeq }),
      onPage: async (current, state) => {
        snapshots.push({
          loaded: state.loadedRecordCount,
          messages: current.messages.length,
          complete: state.complete
        })
        if (state.pageNumber === 1) {
          // Page one is installed and awaited before the traversal can request page two.
          expect(mockHistoryApi.getPage).toHaveBeenCalledTimes(1)
        }
      },
      onProgress: (state) => progressCounts.push(state.loadedRecordCount)
    })

    expect(snapshots).toEqual([
      { loaded: 250, messages: 250, complete: false },
      { loaded: 50_000, messages: 50_000, complete: true }
    ])
    expect(progressCounts).toHaveLength(200)
    expect(progressCounts[0]).toBe(250)
    expect(progressCounts.at(-1)).toBe(50_000)
    expect(result?.messages).toHaveLength(50_000)
    for (let index = 0; index < 50_000; index += 1) {
      expect(result?.messages[index].seq).toBe(index + 1)
    }
    expect(mockHistoryApi.getPage).toHaveBeenCalledTimes(200)
    const metrics = historyPagingMetrics()
    expect(metrics).toMatchObject({
      traversalStarts: 1,
      pageRequests: 200,
      pageApplications: 200,
      recordApplications: 50_000,
      transcriptEntriesCopied: 50_250,
      toolIndexLookups: 0,
      snapshotsCreated: 2,
      currentBytes: 0
    })
    expect(metrics.transcriptEntriesCopied).toBeLessThanOrEqual(100_000)
    expect(metrics.peakBytes).toBeGreaterThan(0)
    expect(metrics.peakBytes).toBeLessThanOrEqual(MAX_HISTORY_IN_FLIGHT_BYTES)
  })

  it('deduplicates the whole traversal and replays shared snapshot/progress to concurrent callers', async () => {
    const sessionId = 'whole-load-flight'
    let releasePageTwo!: () => void
    const pageTwoGate = new Promise<void>((resolve) => {
      releasePageTwo = resolve
    })
    mockHistoryApi.getPage.mockImplementation(
      async (_id, afterSeq: number, limit: number, targetLastSeq?: number) => {
        expect(targetLastSeq).toBe(afterSeq === 0 ? undefined : 500)
        if (afterSeq === 250) await pageTwoGate
        const records = Array.from({ length: Math.min(limit, 500 - afterSeq) }, (_, index) => {
          const seq = afterSeq + index + 1
          return historyRecord(sessionId, seq, 'user_prompt', {
            content: [{ type: 'text', text: `message-${seq}` }]
          })
        })
        return historyPage(sessionId, records, 500)
      }
    )
    const firstSnapshots: number[] = []
    const secondSnapshots: number[] = []
    const first = loadSessionPayload(sessionId, {
      metadata: entry(sessionId),
      onPage: (payload) => firstSnapshots.push(payload.messages.length)
    })
    await vi.waitFor(() => expect(firstSnapshots).toEqual([250]))

    const second = loadSessionPayload(sessionId, {
      metadata: entry(sessionId),
      onPage: (payload) => secondSnapshots.push(payload.messages.length)
    })
    await vi.waitFor(() => expect(secondSnapshots).toEqual([250]))
    expect(mockHistoryApi.getPage.mock.calls.map(([, afterSeq]) => afterSeq)).toEqual([0, 250])

    releasePageTwo()
    const [firstResult, secondResult] = await Promise.all([first, second])
    expect(firstResult).toBe(secondResult)
    expect(firstSnapshots).toEqual([250, 500])
    expect(secondSnapshots).toEqual([250, 500])
    expect(mockHistoryApi.getPage.mock.calls.map(([, afterSeq]) => afterSeq)).toEqual([0, 250])
    expect(historyPagingMetrics()).toMatchObject({ traversalStarts: 1, pageRequests: 2 })
  })

  it('keeps the first frontier pinned across concurrent appends on Desktop pages', async () => {
    const sessionId = 'active-append'
    let currentLastSeq = 500
    mockHistoryApi.getPage.mockImplementation(
      async (_id, afterSeq: number, limit: number, targetLastSeq?: number) => {
        const pinnedTarget = targetLastSeq ?? currentLastSeq
        expect(pinnedTarget).toBe(500)
        const records = Array.from(
          { length: Math.min(limit, pinnedTarget - afterSeq) },
          (_, index) => {
            const seq = afterSeq + index + 1
            return historyRecord(sessionId, seq, 'user_prompt', {
              content: [{ type: 'text', text: `message-${seq}` }]
            })
          }
        )
        if (afterSeq === 0) currentLastSeq = 750
        return historyPage(sessionId, records, pinnedTarget)
      }
    )

    const result = await loadSessionPayload(sessionId, { metadata: entry(sessionId) })

    expect(result?.metadata.lastSeq).toBe(500)
    expect(result?.messages).toHaveLength(500)
    expect(mockHistoryApi.getPage.mock.calls).toEqual([
      [sessionId, 0, RENDERER_HISTORY_PAGE_SIZE, undefined],
      [sessionId, 250, RENDERER_HISTORY_PAGE_SIZE, 500]
    ])
  })

  it('uses one O(1) tool id lookup per tool record without linear scans', async () => {
    const sessionId = 'tool-index'
    const targetLastSeq = 10_000
    mockHistoryApi.getPage.mockImplementation(async (_id, afterSeq: number, limit: number) => {
      const records = Array.from(
        { length: Math.min(limit, targetLastSeq - afterSeq) },
        (_, index) => {
          const seq = afterSeq + index + 1
          return historyRecord(sessionId, seq, 'tool_call', {
            toolCall: { toolCallId: `tool-${seq}`, status: 'completed' }
          })
        }
      )
      return historyPage(sessionId, records, targetLastSeq)
    })

    const result = await loadSessionPayload(sessionId, { metadata: entry(sessionId) })

    expect(result?.toolCalls).toHaveLength(targetLastSeq)
    expect(historyPagingMetrics()).toMatchObject({
      recordApplications: targetLastSeq,
      toolIndexLookups: targetLastSeq,
      pageRequests: 40
    })
  })

  it('preserves message/tool/usage/plan semantics across page boundaries', async () => {
    const sessionId = 'mixed'
    const pages = [
      historyPage(
        sessionId,
        [
          historyRecord(sessionId, 1, 'user_prompt', {
            turnId: 'turn-1',
            content: [{ type: 'text', text: 'hello' }]
          }),
          historyRecord(sessionId, 2, 'message_chunk', {
            role: 'agent',
            content: { type: 'text', text: 'a' }
          }),
          historyRecord(sessionId, 3, 'tool_call', {
            toolCall: { toolCallId: 'tool-1', status: 'in_progress' }
          })
        ],
        10,
        false
      ),
      historyPage(
        sessionId,
        [
          historyRecord(sessionId, 4, 'message_chunk', {
            role: 'agent',
            content: { type: 'text', text: 'b' }
          }),
          historyRecord(sessionId, 5, 'tool_call_update', {
            update: { toolCallId: 'tool-1', status: 'completed' }
          }),
          historyRecord(sessionId, 6, 'usage_update', {
            used: 10,
            size: 100,
            cost: { amount: 1.5, currency: 'USD' }
          }),
          historyRecord(sessionId, 7, 'plan_update', {
            plan: { entries: [{ content: 'ship', status: 'in_progress' }] }
          }),
          historyRecord(sessionId, 8, 'usage_update', {
            used: 0,
            size: 100
          }),
          historyRecord(sessionId, 9, 'plan_update', {
            plan: { entries: [] }
          })
        ],
        10,
        false
      )
    ]
    pages[1] = { ...pages[1], nextCursor: 10, complete: true }
    mockHistoryApi.getPage.mockImplementation(async (_id, afterSeq) =>
      afterSeq === 0 ? pages[0] : pages[1]
    )

    const result = await loadSessionPayload(sessionId, { metadata: entry(sessionId) })

    expect(result?.messages.map((message) => message.seq)).toEqual([1, 2, 4])
    expect(result?.toolCalls).toEqual([
      expect.objectContaining({ toolCallId: 'tool-1', status: 'completed', seq: 3, timestamp: 3 })
    ])
    expect(result?.sessionUsage).toEqual({
      used: 0,
      size: 100,
      baselineUsed: 10,
      updatedAt: 8,
      source: 'reported'
    })
    expect(result).toHaveProperty('plan', [])
    expect(result?.metadata.lastSeq).toBe(10)
  })

  it('retains installed pages after a transient failure and resumes from the failed cursor', async () => {
    const sessionId = 'retry'
    let pageTwoAttempts = 0
    mockHistoryApi.getPage.mockImplementation(async (_id, afterSeq) => {
      if (afterSeq === 0) {
        return historyPage(
          sessionId,
          [
            historyRecord(sessionId, 1, 'user_prompt', {
              content: [{ type: 'text', text: 'retained' }]
            })
          ],
          2,
          false
        )
      }
      pageTwoAttempts += 1
      if (pageTwoAttempts === 1) throw new Error('temporary transport failure')
      return historyPage(
        sessionId,
        [
          historyRecord(sessionId, 2, 'user_prompt', {
            content: [{ type: 'text', text: 'completed' }]
          })
        ],
        2,
        true
      )
    })
    const firstSnapshots: number[] = []
    await expect(
      loadSessionPayload(sessionId, {
        metadata: entry(sessionId),
        onPage: (current) => firstSnapshots.push(current.messages.length)
      })
    ).rejects.toThrow('temporary transport failure')
    expect(firstSnapshots).toEqual([1])

    const retrySnapshots: number[] = []
    const result = await loadSessionPayload(sessionId, {
      metadata: entry(sessionId),
      onPage: (current) => retrySnapshots.push(current.messages.length)
    })
    expect(retrySnapshots).toEqual([1, 2])
    expect(result?.messages.map((message) => message.blocks[0]?.text)).toEqual([
      'retained',
      'completed'
    ])
    expect(
      mockHistoryApi.getPage.mock.calls.map(([, afterSeq, , targetLastSeq]) => [
        afterSeq,
        targetLastSeq
      ])
    ).toEqual([
      [0, undefined],
      [1, 2],
      [1, 2]
    ])
    expect(historyPagingMetrics()).toMatchObject({
      traversalStarts: 2,
      pageRequests: 3,
      recordApplications: 2,
      snapshotsCreated: 2
    })
  })

  it('retains the pinned prefix after a stable later-page failure and retries its exact cursor', async () => {
    const sessionId = 'stable-retry'
    let pageTwoAttempts = 0
    mockHistoryApi.getPage.mockImplementation(async (_id, afterSeq) => {
      if (afterSeq === 0) {
        return historyPage(
          sessionId,
          [
            historyRecord(sessionId, 1, 'user_prompt', {
              content: [{ type: 'text', text: 'retained' }]
            })
          ],
          2,
          false
        )
      }
      pageTwoAttempts += 1
      if (pageTwoAttempts === 1) {
        throw Object.assign(new Error('pinned history frontier is temporarily unavailable'), {
          code: 'stale'
        })
      }
      return historyPage(
        sessionId,
        [
          historyRecord(sessionId, 2, 'user_prompt', {
            content: [{ type: 'text', text: 'completed' }]
          })
        ],
        2,
        true
      )
    })

    await expect(
      loadSessionPayload(sessionId, { metadata: entry(sessionId) })
    ).rejects.toMatchObject({ code: 'stale' })
    const result = await loadSessionPayload(sessionId, { metadata: entry(sessionId) })

    expect(result?.messages.map((message) => message.blocks[0]?.text)).toEqual([
      'retained',
      'completed'
    ])
    expect(
      mockHistoryApi.getPage.mock.calls.map(([, afterSeq, , targetLastSeq]) => [
        afterSeq,
        targetLastSeq
      ])
    ).toEqual([
      [0, undefined],
      [1, 2],
      [1, 2]
    ])
    expect(historyPagingMetrics()).toMatchObject({
      traversalStarts: 2,
      pageRequests: 3,
      recordApplications: 2,
      snapshotsCreated: 2
    })
  })

  it('rejects an oversized page without exceeding the 4 MiB retained-byte metric', async () => {
    const sessionId = 'oversized'
    mockHistoryApi.getPage.mockResolvedValueOnce(
      historyPage(
        sessionId,
        [
          historyRecord(sessionId, 1, 'message_chunk', {
            role: 'agent',
            content: { type: 'text', text: 'x'.repeat(MAX_HISTORY_IN_FLIGHT_BYTES) }
          })
        ],
        1,
        true
      )
    )

    await expect(
      loadSessionPayload(sessionId, { metadata: entry(sessionId) })
    ).rejects.toMatchObject({ code: 'CONVERSATION_HISTORY_IN_FLIGHT_LIMIT' })
    expect(historyPagingMetrics().currentBytes).toBe(0)
    expect(historyPagingMetrics().peakBytes).toBeLessThanOrEqual(MAX_HISTORY_IN_FLIGHT_BYTES)
  })

  it('preserves structured paging-required failures and rejects cross-session pages', async () => {
    const pagingRequired = Object.assign(new Error('use pages'), {
      code: 'CONVERSATION_HISTORY_PAGING_REQUIRED'
    })
    mockHistoryApi.getPage.mockRejectedValueOnce(pagingRequired)
    await expect(loadSessionPayload('required', { metadata: entry('required') })).rejects.toBe(
      pagingRequired
    )

    mockHistoryApi.getPage.mockResolvedValueOnce(
      historyPage(
        'conversation-a',
        [historyRecord('conversation-a', 1, 'user_prompt', { content: [] })],
        1,
        true
      )
    )
    await expect(
      loadSessionPayload('conversation-b', { metadata: entry('conversation-b') })
    ).rejects.toMatchObject({ code: 'VALIDATION_ERROR' })
  })
})

describe('provider routing', () => {
  it('loads desktop index from the Rust facade, including fresh empty state', async () => {
    mockHistoryApi.list.mockResolvedValueOnce({ sessions: [], legacyImportComplete: false })
    await expect(loadSessionIndex()).resolves.toEqual([])
    expect(mockHistoryApi.list).toHaveBeenCalledTimes(1)
    expect(persistenceApi.read).not.toHaveBeenCalled()

    mockHistoryApi.list.mockResolvedValueOnce({
      sessions: [entry('stored')],
      legacyImportComplete: true
    })
    await expect(loadSessionIndex()).resolves.toEqual([entry('stored')])
  })

  it('keeps standalone server and live-only behavior unchanged', async () => {
    mockTransport.historyMode.mockReturnValue('server')
    mockTransport.listPersistedSessions.mockResolvedValue([
      {
        storageKey: 'opaque',
        sessionId: 'server-1',
        stableAgentNamespace: 'config:cfg-server',
        runtimeAgentId: 'runtime-old',
        projectId: 'project-1',
        cwd: '/srv/project',
        title: 'Server chat',
        createdAt: 1,
        lastActivityAt: 2,
        status: 'closed',
        messageCount: 3,
        toolCount: 1,
        lastSeq: 7,
        discovered: true,
        resumeEligible: true
      }
    ])
    await expect(loadSessionIndex()).resolves.toEqual([
      expect.objectContaining({
        id: 'server-1',
        agentConfigId: 'cfg-server',
        discovered: true
      })
    ])
    expect(mockHistoryApi.list).not.toHaveBeenCalled()

    mockTransport.historyMode.mockReturnValue('live_only')
    await expect(loadSessionIndex()).resolves.toEqual([])
  })

  it('retires desktop payload writes (host-authored) but still routes flush', async () => {
    const stored = payload('desktop', [msg('user', 'hi')])
    await saveSessionPayload('desktop', stored)
    // CAP-2: the host event/session layer owns durable writes; the renderer
    // save path must not reach the store (the payload stays a local cache).
    expect(mockHistoryApi.save).not.toHaveBeenCalled()
    expect(getCachedSessionPayload('desktop')).toEqual(stored)

    await flushSessionHistory()
    expect(mockHistoryApi.flush).toHaveBeenCalledTimes(1)
  })

  it('always refetches bounded pages in server mode and never uses the full route', async () => {
    mockTransport.historyMode.mockReturnValue('server')
    mockTransport.listPersistedSessions.mockResolvedValue([
      {
        storageKey: 'opaque',
        sessionId: 'server',
        stableAgentNamespace: 'config:cfg-server',
        runtimeAgentId: 'runtime-old',
        cwd: '/srv/project',
        title: 'Server chat',
        createdAt: 1,
        lastActivityAt: 2,
        status: 'closed',
        messageCount: 1,
        toolCount: 0,
        lastSeq: 1,
        resumeEligible: true
      }
    ])
    mockTransport.getSessionPayloadPage
      .mockResolvedValueOnce(
        historyPage(
          'server',
          [historyRecord('server', 1, 'user_prompt', { content: [{ type: 'text', text: 'one' }] })],
          1,
          true
        )
      )
      .mockResolvedValueOnce(
        historyPage(
          'server',
          [historyRecord('server', 1, 'user_prompt', { content: [{ type: 'text', text: 'two' }] })],
          1,
          true
        )
      )
    expect((await loadSessionPayload('server'))?.messages[0].blocks[0]?.text).toBe('one')
    expect((await loadSessionPayload('server'))?.messages[0].blocks[0]?.text).toBe('two')
    expect(mockTransport.getSessionPayloadPage).toHaveBeenCalledTimes(2)
    expect(mockTransport.getSessionPayload).not.toHaveBeenCalled()
  })
})

describe('bounded full-payload cache', () => {
  it('evicts least-recent inactive entries and reloads them from Rust', async () => {
    for (let index = 0; index <= INACTIVE_PAYLOAD_CACHE_BUDGET; index += 1) {
      setCachedSessionPayload(`s-${index}`, payload(`s-${index}`, [msg('user', `${index}`)]))
    }
    expect(getCachedSessionPayload('s-0')).toBeUndefined()

    mockHistoryApi.list.mockResolvedValueOnce({
      sessions: [entry('s-0', { lastSeq: 1, messageCount: 1 })],
      legacyImportComplete: true
    })
    mockHistoryApi.getPage.mockResolvedValueOnce(
      historyPage(
        's-0',
        [
          historyRecord('s-0', 1, 'user_prompt', {
            content: [{ type: 'text', text: 'reloaded' }]
          })
        ],
        1,
        true
      )
    )
    const reloaded = await loadSessionPayload('s-0')
    expect(reloaded?.messages[0].blocks[0]?.text).toBe('reloaded')
    expect(mockHistoryApi.getPage).toHaveBeenCalledWith(
      's-0',
      0,
      RENDERER_HISTORY_PAGE_SIZE,
      undefined
    )
    expect(mockHistoryApi.get).not.toHaveBeenCalled()
  })

  it('pins trimmed live sessions in addition to the inactive budget', () => {
    markSessionPayloadPinned('pinned')
    setCachedSessionPayload('pinned', payload('pinned'))
    for (let index = 0; index <= INACTIVE_PAYLOAD_CACHE_BUDGET; index += 1) {
      setCachedSessionPayload(`inactive-${index}`, payload(`inactive-${index}`))
    }
    expect(getCachedSessionPayload('pinned')).toBeDefined()
    expect(getCachedSessionPayload('inactive-0')).toBeUndefined()
    unpinSessionPayload('pinned')
  })

  it('saveSessionPayload never reads or writes the store (host-authored history)', async () => {
    const retained = msg('agent', 'retained')
    const latest = msg('agent', 'latest')

    await saveSessionPayload('merge', payload('merge', [retained, latest]))

    expect(mockHistoryApi.save).not.toHaveBeenCalled()
    expect(mockHistoryApi.get).not.toHaveBeenCalled()
    expect(getCachedSessionPayload('merge')?.messages).toEqual([retained, latest])
  })
})

describe('legacy import', () => {
  const old1 = payload('old-1', [msg('user', 'one')])
  const old2 = payload('old-2', [msg('user', 'two'), msg('agent', 'reply')])
  const index = [old1.metadata, old2.metadata]

  function mockLegacyReads(): void {
    ;(persistenceApi.read as ReturnType<typeof vi.fn>)
      .mockResolvedValueOnce({ success: true, data: index })
      .mockResolvedValueOnce({ success: true, data: old1 })
      .mockResolvedValueOnce({ success: true, data: old2 })
  }

  it('round-trips every payload before deleting only ACP history keys', async () => {
    mockLegacyReads()
    mockHistoryApi.listLegacy
      .mockResolvedValueOnce({ sessions: [], legacyImportComplete: false })
      .mockResolvedValueOnce({ sessions: index, legacyImportComplete: false })
    mockHistoryApi.getLegacy.mockImplementation(async (id: string) =>
      id === 'old-1' ? old1 : old2
    )

    await runHistoryWipeMigration()

    expect(mockHistoryApi.save).toHaveBeenNthCalledWith(1, 'old-1', old1)
    expect(mockHistoryApi.save).toHaveBeenNthCalledWith(2, 'old-2', old2)
    expect(mockHistoryApi.getLegacy).toHaveBeenCalledWith('old-1')
    expect(mockHistoryApi.getLegacy).toHaveBeenCalledWith('old-2')
    expect(persistenceApi.delete).toHaveBeenCalledWith(sessionPayloadKey('old-1'))
    expect(persistenceApi.delete).toHaveBeenCalledWith(sessionPayloadKey('old-2'))
    expect(persistenceApi.delete).toHaveBeenCalledWith(SESSION_INDEX_KEY)
    expect(persistenceApi.delete).toHaveBeenCalledTimes(3)
    expect(mockHistoryApi.markLegacyImportComplete).toHaveBeenCalledTimes(1)
  })

  it('is idempotent when Rust marks legacy import complete', async () => {
    mockHistoryApi.listLegacy.mockResolvedValueOnce({
      sessions: index,
      legacyImportComplete: true
    })
    await runHistoryWipeMigration()
    expect(persistenceApi.read).not.toHaveBeenCalled()
    expect(mockHistoryApi.save).not.toHaveBeenCalled()
  })

  it('fails closed on a successful non-array legacy index', async () => {
    ;(persistenceApi.read as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      success: true,
      data: { unexpected: true }
    })
    await expect(runHistoryWipeMigration()).rejects.toThrow('Legacy session index is not an array')
    expect(mockHistoryApi.save).not.toHaveBeenCalled()
  })

  it('fails closed when legacy index and payload ids differ', async () => {
    ;(persistenceApi.read as ReturnType<typeof vi.fn>)
      .mockResolvedValueOnce({ success: true, data: index })
      .mockResolvedValueOnce({ success: true, data: payload('different') })
    await expect(runHistoryWipeMigration()).rejects.toThrow('Legacy payload id mismatch')
    expect(mockHistoryApi.save).not.toHaveBeenCalled()
  })

  it('does not overwrite a newer differing durable payload on retry', async () => {
    mockLegacyReads()
    mockHistoryApi.listLegacy.mockResolvedValueOnce({
      sessions: [old1.metadata],
      legacyImportComplete: false
    })
    mockHistoryApi.getLegacy.mockResolvedValueOnce(payload('old-1', [msg('user', 'newer')]))
    await expect(runHistoryWipeMigration()).rejects.toThrow('Durable history differs')
    expect(mockHistoryApi.save).not.toHaveBeenCalled()
    expect(persistenceApi.delete).not.toHaveBeenCalled()
  })

  it('fails closed when any legacy payload cannot be read', async () => {
    ;(persistenceApi.read as ReturnType<typeof vi.fn>)
      .mockResolvedValueOnce({ success: true, data: index })
      .mockResolvedValueOnce({ success: true, data: old1 })
      .mockResolvedValueOnce({ success: false, code: 'READ_ERROR', error: 'payload unavailable' })

    await expect(runHistoryWipeMigration()).rejects.toThrow('payload unavailable')
    expect(mockHistoryApi.save).not.toHaveBeenCalled()
    expect(persistenceApi.delete).not.toHaveBeenCalled()
  })

  it('fails closed when Rust verification fails and retains all legacy keys', async () => {
    mockLegacyReads()
    mockHistoryApi.listLegacy
      .mockResolvedValueOnce({ sessions: [], legacyImportComplete: false })
      .mockResolvedValueOnce({ sessions: [old1.metadata], legacyImportComplete: false })

    await expect(runHistoryWipeMigration()).rejects.toThrow('Legacy payload verification failed')
    expect(persistenceApi.delete).not.toHaveBeenCalled()
    expect(mockHistoryApi.markLegacyImportComplete).not.toHaveBeenCalled()
  })

  it('restores the complete legacy source if cleanup fails part-way', async () => {
    mockLegacyReads()
    mockHistoryApi.listLegacy
      .mockResolvedValueOnce({ sessions: [], legacyImportComplete: false })
      .mockResolvedValueOnce({ sessions: index, legacyImportComplete: false })
    mockHistoryApi.getLegacy.mockImplementation(async (id: string) =>
      id === 'old-1' ? old1 : old2
    )
    ;(persistenceApi.delete as ReturnType<typeof vi.fn>)
      .mockResolvedValueOnce({ success: true })
      .mockResolvedValueOnce({ success: false, error: 'delete failed', code: 'DELETE_ERROR' })

    await expect(runHistoryWipeMigration()).rejects.toThrow('delete failed')
    expect(persistenceApi.write).toHaveBeenCalledWith(sessionPayloadKey('old-1'), old1)
    expect(persistenceApi.write).toHaveBeenCalledWith(sessionPayloadKey('old-2'), old2)
    expect(persistenceApi.write).toHaveBeenCalledWith(SESSION_INDEX_KEY, index)
    expect(mockHistoryApi.markLegacyImportComplete).not.toHaveBeenCalled()
  })

  it('reports rollback write failures', async () => {
    mockLegacyReads()
    mockHistoryApi.listLegacy
      .mockResolvedValueOnce({ sessions: [], legacyImportComplete: false })
      .mockResolvedValueOnce({ sessions: index, legacyImportComplete: false })
    mockHistoryApi.getLegacy.mockImplementation(async (id: string) =>
      id === 'old-1' ? old1 : old2
    )
    ;(persistenceApi.delete as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      success: false,
      error: 'delete failed',
      code: 'DELETE_ERROR'
    })
    ;(persistenceApi.write as ReturnType<typeof vi.fn>).mockResolvedValue({
      success: false,
      error: 'rollback disk full',
      code: 'WRITE_ERROR'
    })
    await expect(runHistoryWipeMigration()).rejects.toThrow('rollback failed: rollback disk full')
  })
})

describe('serialized save/delete/close barriers', () => {
  const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0))

  it('coalesces repeated queued saves to the latest pending payload (host-authored, no store)', async () => {
    const first = queueSessionPayloadSave('stream', payload('stream', [msg('user', 'one')]))
    await flush()
    const second = queueSessionPayloadSave('stream', payload('stream', [msg('user', 'two')]))
    const third = queueSessionPayloadSave('stream', payload('stream', [msg('user', 'three')]))
    await Promise.all([first, second, third, waitForPendingSessionIndexWrite()])
    // CAP-2: writes are host-owned — the queue still coalesces + resolves, but
    // nothing reaches the renderer-owned store; the last payload stays cached.
    expect(mockHistoryApi.save).not.toHaveBeenCalled()
    expect(getCachedSessionPayload('stream')?.messages).toEqual([msg('user', 'three')])
  })

  it('delete supersedes a stale pending save for the same session', async () => {
    const other = queueSessionPayloadSave('other', payload('other'))
    await flush()
    const stale = queueSessionPayloadSave('deleted', payload('deleted', [msg('user', 'stale')]))
    const deletion = queueSessionPayloadDelete('deleted')
    await Promise.all([other, stale, deletion, waitForPendingSessionIndexWrite()])
    expect(mockHistoryApi.save).not.toHaveBeenCalledWith('deleted', expect.anything())
    expect(mockHistoryApi.delete).toHaveBeenCalledWith('deleted')
  })

  it('rejects a queued delete failure and keeps the tombstone until a successful retry', async () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {})
    mockHistoryApi.delete.mockRejectedValueOnce(new Error('delete failed'))

    await expect(queueSessionPayloadDelete('recreated')).rejects.toThrow('delete failed')
    await queueSessionPayloadSave('recreated', payload('recreated', [msg('user', 'blocked')]))
    // Tombstone still set: the queued save is dropped without caching.
    expect(getCachedSessionPayload('recreated')).toBeUndefined()

    await expect(queueSessionPayloadDelete('recreated')).resolves.toBeUndefined()
    await queueSessionPayloadSave('recreated', payload('recreated', [msg('user', 'saved')]))
    await waitForPendingSessionIndexWrite()
    // Tombstone cleared: the save applies (local cache only — host owns writes).
    expect(getCachedSessionPayload('recreated')?.messages).toEqual([msg('user', 'saved')])
    consoleError.mockRestore()
  })

  it('flush waits for a gated tracked write before invoking Rust flush', async () => {
    let releaseSave!: () => void
    const saveGate = new Promise<void>((resolve) => {
      releaseSave = resolve
    })
    void trackPendingIndexWrite(() => saveGate)
    let flushed = false
    void flushSessionHistory().then(() => {
      flushed = true
    })
    await flush()
    expect(mockHistoryApi.flush).not.toHaveBeenCalled()
    expect(flushed).toBe(false)
    releaseSave()
    await waitForPendingSessionIndexWrite()
    await flush()
    expect(mockHistoryApi.flush).toHaveBeenCalledTimes(1)
    expect(flushed).toBe(true)
  })

  it('dedupes concurrent flushSessionHistory calls to a single backend flush', async () => {
    // beforeunload + pagehide + closeAppWithPersistenceFlush can all fire on
    // close; without memoization they would race 3× concurrent backend
    // acp_history_flush calls. All three callers must await the SAME in-flight
    // promise so exactly one backend flush is invoked.
    let releaseFlush!: () => void
    const flushGate = new Promise<void>((resolve) => {
      releaseFlush = resolve
    })
    mockHistoryApi.flush.mockReturnValue(flushGate)

    const first = flushSessionHistory()
    const second = flushSessionHistory()
    const third = flushSessionHistory()
    await flush()
    expect(mockHistoryApi.flush).toHaveBeenCalledTimes(1)

    releaseFlush()
    await Promise.all([first, second, third])
    expect(mockHistoryApi.flush).toHaveBeenCalledTimes(1)

    // After the in-flight promise settles, a fresh flushSessionHistory can
    // invoke the backend again (the memo is cleared, not pinned forever).
    mockHistoryApi.flush.mockResolvedValue(undefined)
    await flushSessionHistory()
    expect(mockHistoryApi.flush).toHaveBeenCalledTimes(2)
  })

  it('serializes operations so a queued stale save lands before delete', async () => {
    const order: string[] = []
    let releaseSave!: () => void
    const saveGate = new Promise<void>((resolve) => {
      releaseSave = resolve
    })
    void trackPendingIndexWrite(async () => {
      order.push('save-start')
      await saveGate
      order.push('save-end')
    })
    void trackPendingIndexWrite(async () => {
      order.push('delete')
    })

    await flush()
    expect(order).toEqual(['save-start'])
    releaseSave()
    await waitForPendingSessionIndexWrite()
    expect(order).toEqual(['save-start', 'save-end', 'delete'])
  })

  it('awaits operations queued while the close barrier is draining', async () => {
    let releaseFirst!: () => void
    let releaseSecond!: () => void
    const first = new Promise<void>((resolve) => {
      releaseFirst = resolve
    })
    const second = new Promise<void>((resolve) => {
      releaseSecond = resolve
    })
    void trackPendingIndexWrite(() => first)
    let settled = false
    void waitForPendingSessionIndexWrite().then(() => {
      settled = true
    })
    await flush()
    void trackPendingIndexWrite(() => second)
    releaseFirst()
    await flush()
    expect(settled).toBe(false)
    releaseSecond()
    await waitForPendingSessionIndexWrite()
    expect(settled).toBe(true)
  })

  it('logs write failures without breaking later operations', async () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {})
    void trackPendingIndexWrite(() => Promise.reject(new Error('failed')))
    await expect(waitForPendingSessionIndexWrite()).resolves.toBeUndefined()
    expect(consoleError).toHaveBeenCalledWith(
      '[acp] failed to persist session history',
      expect.any(Error)
    )
    consoleError.mockRestore()
  })
})

describe('failed prefix budgets', () => {
  beforeEach(() => {
    _resetHistoryPagingForTesting()
  })

  it('failed prefixes evict by count byte and ttl', () => {
    const now = Date.now()
    _seedFailedPrefixForTesting('s1', 100, now)
    _seedFailedPrefixForTesting('s2', 100, now + 1)
    _seedFailedPrefixForTesting('s3', 100, now + 2)
    _seedFailedPrefixForTesting('s4', 100, now + 3)
    _seedFailedPrefixForTesting('s5', 100, now + 4)
    expect(_failedPrefixIdsForTesting()).toHaveLength(MAX_FAILED_PREFIX_ASSEMBLIES)
    expect(_failedPrefixIdsForTesting()).not.toContain('s1')
    expect(_failedPrefixIdsForTesting()).toEqual(['s2', 's3', 's4', 's5'])

    _resetHistoryPagingForTesting()
    _seedFailedPrefixForTesting('big-1', MAX_FAILED_PREFIX_PAYLOAD_BYTES - 10, now)
    _seedFailedPrefixForTesting('big-2', 20, now + 1)
    expect(_failedPrefixIdsForTesting()).toEqual(['big-2'])

    _resetHistoryPagingForTesting()
    _seedFailedPrefixForTesting('old', 10, now - FAILED_PREFIX_TTL_MS - 1)
    _seedFailedPrefixForTesting('fresh', 10, now)
    expect(_failedPrefixIdsForTesting()).toEqual(['fresh'])
  })

  it('unpin and dispose clear failed prefix payload keep resume metadata', () => {
    const now = Date.now()
    _seedFailedPrefixForTesting('kept', 32, now, 'CONVERSATION_PAGE_TOO_LARGE')
    expect(_failedPrefixIdsForTesting()).toEqual(['kept'])
    expect(_resumeMetadataForTesting('kept')).toEqual({
      sessionId: 'kept',
      cursor: 0,
      targetLastSeq: 0,
      errorCode: 'CONVERSATION_PAGE_TOO_LARGE'
    })
    unpinSessionPayload('kept')
    expect(_failedPrefixIdsForTesting()).toEqual([])
    expect(_resumeMetadataForTesting('kept')?.errorCode).toBe('CONVERSATION_PAGE_TOO_LARGE')

    _seedFailedPrefixForTesting('other', 16, now, 'TRANSPORT_ERROR')
    disposeFailedPrefixPayloads()
    expect(_failedPrefixIdsForTesting()).toEqual([])
    expect(_resumeMetadataForTesting('other')?.errorCode).toBe('TRANSPORT_ERROR')
  })
})
