import type {
  IpcResult,
  TerminalReplayCoverage,
  TerminalResumeGrant,
  TerminalSpawnedEvent
} from '@shared/types/ipc.types'
import type {
  TerminalResourceDescriptor,
  TerminalResourceHydrationStatus
} from '@shared/types/session-workspace.types'
import {
  readTerminalResourceFailure,
  type TerminalCleanupRecoveryInput
} from '@shared/types/web-terminal-protocol.types'
import { create } from 'zustand'
import { useShallow } from 'zustand/shallow'
import { disposeCachedTerminal } from '@/components/terminal/terminal-cache'
import { i18n } from '@/i18n'
import { formatNumber } from '@/i18n/format'
import { logFrontendError } from '@/lib/log-api'
import { terminalApi } from '@/lib/terminal-api'
import type { GitStatus, Terminal, TerminalHealthStatus } from '@/types/project'
import { useProjectStore } from './project-store'

const GLOBAL_TERMINAL_LIMIT = 30
export const HIDDEN_BUFFER_TRUNCATION_DELAY = 15 * 60 * 1000 // 15 minutes
export const TRUNCATED_BUFFER_SIZE = 5000
export const MAX_TRANSCRIPT_CHARS = 1_500_000
const LINE_BREAK_PATTERN = /\r\n|\r|\n/
const terminalResumeInFlight = new Map<string, Promise<IpcResult<TerminalReplayCoverage | null>>>()
const terminalCleanupRetryInFlight = new Map<string, Promise<boolean>>()

export interface TerminalCleanupRecovery extends TerminalCleanupRecoveryInput {
  retrying: boolean
  retryFailed: boolean
}

// ADR-004.4: descriptive-only agent metadata applied to a Terminal record.
export interface TerminalAgentMetadata {
  agentId: string
  agentName: string
  agentProgram: string
  agentArgs: string[]
}

function trimTranscriptToMaxChars(transcript: string): string {
  if (transcript.length <= MAX_TRANSCRIPT_CHARS) {
    return transcript
  }

  const tail = transcript.slice(-MAX_TRANSCRIPT_CHARS)
  const firstBreak = LINE_BREAK_PATTERN.exec(tail)
  return firstBreak ? tail.slice(firstBreak.index + firstBreak[0].length) : tail
}

function trimTranscriptToRecentLines(transcript: string): string {
  return transcript.split(LINE_BREAK_PATTERN).slice(-TRUNCATED_BUFFER_SIZE).join('\n')
}

/**
 * Depth of the most-recently-used terminal stack. Only the head is needed for
 * jump-to-previous; the tail exists so the quick switcher can order by
 * recency, and so a jump still lands somewhere sensible when the immediately
 * previous terminal has since been closed.
 */
const RECENT_TERMINAL_LIMIT = 20

function promoteRecentTerminal(recent: readonly string[], id: string): string[] {
  if (!id || recent[0] === id) return [...recent]
  return [id, ...recent.filter((candidate) => candidate !== id)].slice(0, RECENT_TERMINAL_LIMIT)
}

export interface TerminalState {
  // State
  terminals: Terminal[]
  activeTerminalId: string
  /** Most-recently-selected terminal ids, most recent first. May contain closed ids. */
  recentTerminalIds: string[]
  // Index for O(1) ptyId lookups
  ptyIdIndex: Map<string, string>
  /** Secret-free cleanup-only records keyed by the retained host terminal id. */
  cleanupRecoveries: Record<string, TerminalCleanupRecovery>

  // Actions
  selectTerminal: (id: string) => void
  /** Head of the MRU stack that is neither the active terminal nor already closed. */
  getPreviousTerminalId: () => string | undefined
  addTerminal: (
    name: string,
    projectId: string,
    shell?: Terminal['shell'],
    cwd?: string,
    pendingScrollback?: string[],
    conversationId?: string
  ) => Terminal
  adoptRemoteProjectTerminal: (event: TerminalSpawnedEvent) => string | null
  closeTerminal: (id: string, projectId: string) => void
  closeTerminalView: (id: string) => Promise<boolean>
  reopenTerminalView: (id: string) => void
  terminateTerminalResource: (id: string) => Promise<boolean>
  recordTerminalCleanupFailure: (result: IpcResult<unknown>) => TerminalCleanupRecoveryInput | null
  retryTerminalCleanup: (terminalId: string) => Promise<boolean>
  renameTerminal: (id: string, name: string) => void
  reorderTerminals: (projectId: string, orderedIds: string[]) => void
  setTerminals: (terminals: Terminal[]) => void
  hydrateTerminalResource: (
    descriptor: TerminalResourceDescriptor,
    grant?: TerminalResumeGrant,
    projectId?: string
  ) => void
  resumeTerminalResource: (id: string) => Promise<IpcResult<TerminalReplayCoverage | null>>
  setTerminalPtyId: (id: string, ptyId: string) => boolean
  setTerminalClaim: (ptyId: string, claim: string | undefined) => void
  findTerminalByPtyId: (ptyId: string) => Terminal | undefined
  setTerminalAgentMetadata: (id: string, meta: TerminalAgentMetadata) => void
  updateTerminalCwd: (id: string, cwd: string) => void
  updateTerminalGitBranch: (id: string, gitBranch: string | null) => void
  updateTerminalGitStatus: (id: string, gitStatus: GitStatus | null) => void
  updateTerminalExitCode: (id: string, exitCode: number | null) => void
  updateTerminalScrollback: (id: string, scrollback: string[] | undefined) => void
  appendTranscript: (ptyId: string, data: string) => void
  peekTranscript: (ptyId: string) => string
  consumeTranscript: (ptyId: string) => string
  appendDetachedOutput: (ptyId: string, data: string) => void
  consumeDetachedOutput: (ptyId: string) => string
  setRendererAttached: (ptyId: string, attached: boolean) => void
  setTerminalHealthStatus: (id: string, status: TerminalHealthStatus) => void
  setTerminalResumeCursor: (id: string, cursor: number) => void
  setTerminalHidden: (id: string, isHidden: boolean) => void
  setTerminalNeedsAttention: (id: string, value: boolean) => void
  setAppHidden: (isHidden: boolean) => void
  /** @deprecated Use updateTerminalActivityBatch instead */
  updateTerminalActivity: (id: string, hasActivity: boolean) => void
  /** @deprecated Use updateTerminalActivityBatch instead */
  updateTerminalLastActivityTimestamp: (id: string, timestamp: number) => void
  restartTerminal: (id: string) => void
  restartTerminalResource: (id: string) => Promise<boolean>
  updateTerminalActivityBatch: (id: string, hasActivity: boolean, timestamp: number) => void
  clearTerminalPtyId: (ptyId: string) => void
  truncateHiddenTerminalBuffers: () => void
  cleanupProjectTerminals: (projectId: string) => void
  getTerminalCount: () => number
  isTerminalLimitReached: () => boolean
}

export const useTerminalStore = create<TerminalState>((set, get) => ({
  terminals: [],
  activeTerminalId: '',
  recentTerminalIds: [],
  ptyIdIndex: new Map(),
  cleanupRecoveries: {},

  selectTerminal: (id: string): void => {
    set((state) => ({
      activeTerminalId: id,
      recentTerminalIds: promoteRecentTerminal(state.recentTerminalIds, id),
      terminals: state.terminals.map((t) => ({ ...t, isActive: t.id === id }))
    }))
  },

  getPreviousTerminalId: (): string | undefined => {
    const { terminals, activeTerminalId, recentTerminalIds } = get()
    // Resolved against the live set instead of pruned on close: a terminal can
    // leave through closeTerminal, cleanupProjectTerminals, setTerminals or a
    // host-driven teardown, and every one of those would need its own prune.
    // Filtering here makes a stale id unrepresentable in the answer.
    const live = new Set(terminals.map((terminal) => terminal.id))
    return recentTerminalIds.find((id) => id !== activeTerminalId && live.has(id))
  },

  addTerminal: (
    name: string,
    projectId: string,
    shell: Terminal['shell'] = 'powershell',
    cwd?: string,
    pendingScrollback?: string[],
    suppliedConversationId?: string
  ): Terminal => {
    // Check global terminal limit
    const { terminals } = get()
    if (terminals.length >= GLOBAL_TERMINAL_LIMIT) {
      throw new Error(
        i18n.t('limits.global', {
          ns: 'terminal',
          count: GLOBAL_TERMINAL_LIMIT,
          formattedCount: formatNumber(GLOBAL_TERMINAL_LIMIT),
          defaultValue: 'Maximum {{formattedCount}} terminals allowed across all projects'
        })
      )
    }

    // Scope-less project terminals carry no Conversation id; only terminals
    // created inside an open Conversation are conversation-scoped.
    const conversationId = suppliedConversationId

    const newTerminal: Terminal = {
      id: Date.now().toString(),
      conversationId,
      name,
      projectId,
      shell,
      cwd,
      output: [],
      pendingScrollback,
      healthStatus: 'running',
      viewState: 'visible',
      isHidden: false
    }
    set((state) => ({
      terminals: [...state.terminals, newTerminal],
      activeTerminalId: newTerminal.id,
      recentTerminalIds: promoteRecentTerminal(state.recentTerminalIds, newTerminal.id)
    }))
    return newTerminal
  },

  adoptRemoteProjectTerminal: (event) => {
    const projectId = event.projectId?.trim()
    if (!projectId || !event.terminalId.trim()) return null
    const existing = get().findTerminalByPtyId(event.terminalId)
    if (existing) return existing.id
    if (get().terminals.length >= GLOBAL_TERMINAL_LIMIT) {
      void logFrontendError({
        level: 'warn',
        source: 'terminal-store.adoptRemote',
        message: 'code=GLOBAL_TERMINAL_LIMIT'
      })
      return null
    }
    const folder = event.cwd.split(/[\\/]/).filter(Boolean).at(-1)
    const adopted: Terminal = {
      id: event.terminalId,
      ptyId: event.terminalId,
      conversationId: event.conversationId ?? undefined,
      name:
        folder && folder.length > 0
          ? folder
          : i18n.t('defaultName', {
              ns: 'terminal',
              number: formatNumber(get().terminals.length + 1),
              defaultValue: 'Terminal {{number}}'
            }),
      projectId,
      shell: event.shell || 'shell',
      cwd: event.cwd,
      output: [],
      healthStatus: 'running',
      viewState: 'visible',
      isHidden: false
    }
    set((state) => {
      const nextIndex = new Map(state.ptyIdIndex)
      nextIndex.set(event.terminalId, adopted.id)
      return {
        terminals: [...state.terminals, adopted],
        ptyIdIndex: nextIndex
      }
    })
    void logFrontendError({
      level: 'warn',
      source: 'terminal-store.adoptRemote',
      message: `Adopted host terminal projectId=${projectId}`
    })
    return adopted.id
  },

  closeTerminal: (id: string, projectId: string): void => {
    const { terminals, activeTerminalId, ptyIdIndex } = get()
    const closedTerminal = terminals.find((t) => t.id === id)
    const remaining = terminals.filter((t) => t.id !== id)
    const projectTerminals = remaining.filter((t) => t.projectId === projectId)

    const newIndex = new Map(ptyIdIndex)
    if (closedTerminal?.ptyId) {
      disposeCachedTerminal(closedTerminal.ptyId)
      newIndex.delete(closedTerminal.ptyId)
    }

    set({
      terminals: remaining,
      ptyIdIndex: newIndex,
      activeTerminalId:
        activeTerminalId === id && projectTerminals.length > 0
          ? projectTerminals[0].id
          : activeTerminalId === id
            ? ''
            : activeTerminalId
    })
  },

  closeTerminalView: async (id: string): Promise<boolean> => {
    const terminal = get().terminals.find((candidate) => candidate.id === id)
    if (!terminal) return false
    if (terminal.ptyId) {
      const result = await terminalApi.closeView(terminal.ptyId)
      if (!result.success) {
        // Close-view is a local UI hide. The PTY is supposed to keep running,
        // so a detach/forwarder failure must not block removing the tab.
        void logFrontendError({
          level: 'warn',
          source: 'terminal-store.close-view',
          message: `code=${result.code ?? 'CLOSE_VIEW_FAILED'} terminalId=${terminal.ptyId}`
        })
      }
    }
    set((state) => ({
      terminals: state.terminals.map((candidate) =>
        candidate.id === id
          ? { ...candidate, viewState: 'hidden', isHidden: true, hiddenSince: Date.now() }
          : candidate
      ),
      activeTerminalId: state.activeTerminalId === id ? '' : state.activeTerminalId
    }))
    return true
  },

  reopenTerminalView: (id: string): void => {
    set((state) => ({
      terminals: state.terminals.map((candidate) =>
        candidate.id === id
          ? {
              ...candidate,
              viewState: 'visible',
              isHidden: false,
              hiddenSince: undefined
            }
          : candidate
      ),
      activeTerminalId: id,
      recentTerminalIds: promoteRecentTerminal(state.recentTerminalIds, id)
    }))
  },

  terminateTerminalResource: async (id: string): Promise<boolean> => {
    const terminal = get().terminals.find((candidate) => candidate.id === id)
    if (!terminal) return false
    if (terminal.ptyId) {
      const result = await terminalApi.terminate(terminal.ptyId)
      if (!result.success) {
        get().recordTerminalCleanupFailure(result)
        return false
      }
      set((state) => {
        if (!(terminal.ptyId! in state.cleanupRecoveries)) return state
        const cleanupRecoveries = { ...state.cleanupRecoveries }
        delete cleanupRecoveries[terminal.ptyId!]
        return { cleanupRecoveries }
      })
    }
    get().closeTerminal(id, terminal.projectId ?? '')
    return true
  },

  recordTerminalCleanupFailure: (
    result: IpcResult<unknown>
  ): TerminalCleanupRecoveryInput | null => {
    const failure = readTerminalResourceFailure(result)
    if (!failure) return null
    set((state) => ({
      cleanupRecoveries: {
        ...state.cleanupRecoveries,
        [failure.terminalId]: {
          ...failure,
          retrying: false,
          retryFailed: false
        }
      }
    }))
    return failure
  },

  retryTerminalCleanup: (terminalId: string): Promise<boolean> => {
    const existing = terminalCleanupRetryInFlight.get(terminalId)
    if (existing) return existing
    if (!get().cleanupRecoveries[terminalId]) return Promise.resolve(false)

    set((state) => ({
      cleanupRecoveries: {
        ...state.cleanupRecoveries,
        [terminalId]: {
          ...state.cleanupRecoveries[terminalId],
          retrying: true,
          retryFailed: false
        }
      }
    }))

    const task = (async (): Promise<boolean> => {
      let result: IpcResult<void>
      try {
        result = await terminalApi.terminate(terminalId)
      } catch {
        result = { success: false, error: 'Terminal cleanup retry failed', code: 'NETWORK_ERROR' }
      }

      if (result.success) {
        set((state) => {
          const cleanupRecoveries = { ...state.cleanupRecoveries }
          delete cleanupRecoveries[terminalId]
          const ptyIdIndex = new Map(state.ptyIdIndex)
          ptyIdIndex.delete(terminalId)
          const terminals = state.terminals.filter((terminal) => terminal.ptyId !== terminalId)
          const removedIds = new Set(
            state.terminals
              .filter((terminal) => terminal.ptyId === terminalId)
              .map((terminal) => terminal.id)
          )
          return {
            cleanupRecoveries,
            ptyIdIndex,
            terminals,
            activeTerminalId: removedIds.has(state.activeTerminalId) ? '' : state.activeTerminalId
          }
        })
        return true
      }

      const decoded = readTerminalResourceFailure(result)
      set((state) => {
        const retained = state.cleanupRecoveries[terminalId]
        if (!retained) return state
        return {
          cleanupRecoveries: {
            ...state.cleanupRecoveries,
            [terminalId]: {
              ...(decoded?.terminalId === terminalId ? decoded : retained),
              retrying: false,
              retryFailed: true
            }
          }
        }
      })
      return false
    })()

    terminalCleanupRetryInFlight.set(terminalId, task)
    const clearInFlight = (): void => {
      if (terminalCleanupRetryInFlight.get(terminalId) === task) {
        terminalCleanupRetryInFlight.delete(terminalId)
      }
    }
    void task.then(clearInFlight, clearInFlight)
    return task
  },

  renameTerminal: (id: string, name: string): void => {
    set((state) => ({
      terminals: state.terminals.map((t) => (t.id === id ? { ...t, name } : t))
    }))
  },

  reorderTerminals: (projectId: string, orderedIds: string[]): void => {
    set((state) => {
      const projectTerminals = state.terminals.filter((t) => t.projectId === projectId)
      const otherTerminals = state.terminals.filter((t) => t.projectId !== projectId)

      const reordered = orderedIds
        .map((id) => projectTerminals.find((t) => t.id === id))
        .filter((t): t is Terminal => t !== undefined)

      return { terminals: [...otherTerminals, ...reordered] }
    })
  },

  setTerminals: (terminals: Terminal[]): void => {
    const newIndex = new Map<string, string>()
    for (const t of terminals) {
      if (t.ptyId) newIndex.set(t.ptyId, t.id)
    }
    set({ terminals, ptyIdIndex: newIndex })
  },

  /**
   * Materialize a passive SessionWorkspace terminal descriptor in renderer
   * memory. This action never spawns, terminates, or otherwise claims PTY
   * ownership; it only reconciles the renderer record and its ptyId index.
   */
  hydrateTerminalResource: (
    descriptor: TerminalResourceDescriptor,
    grant?: TerminalResumeGrant,
    projectId?: string
  ): void => {
    const recordId = descriptor.terminalRecordId ?? descriptor.terminalId
    const fallbackProjectId = projectId ?? useProjectStore.getState().activeProjectId

    set((state) => {
      const byRecord = state.terminals.find((terminal) => terminal.id === recordId)
      const indexedOwner = state.ptyIdIndex.get(descriptor.terminalId)
      const byPty = indexedOwner
        ? state.terminals.find((terminal) => terminal.id === indexedOwner)
        : state.terminals.find((terminal) => terminal.ptyId === descriptor.terminalId)
      const base = byRecord ?? byPty
      const firstIndex = state.terminals.findIndex(
        (terminal) => terminal.id === recordId || terminal.ptyId === descriptor.terminalId
      )
      const healthStatus: TerminalResourceHydrationStatus = grant ? 'running' : 'disconnected'
      const hydrated: Terminal = {
        ...base,
        id: recordId,
        conversationId: descriptor.conversationId,
        ptyId: descriptor.terminalId,
        name:
          base?.name ??
          i18n.t('resume.restoredName', {
            ns: 'terminal',
            defaultValue: 'Restored terminal'
          }),
        projectId: base?.projectId ?? fallbackProjectId,
        shell: grant?.terminal.shell ?? base?.shell ?? 'shell',
        cwd: grant?.terminal.cwd ?? base?.cwd,
        healthStatus,
        resumeCursor: grant?.terminal.latestSeq ?? base?.resumeCursor,
        claim: grant?.claim,
        viewState: base?.viewState ?? 'visible',
        isHidden: base?.isHidden ?? false,
        rendererAttachmentCount: base?.rendererAttachmentCount ?? 0
      }

      const terminals = state.terminals.filter(
        (terminal) => terminal.id !== recordId && terminal.ptyId !== descriptor.terminalId
      )
      terminals.splice(firstIndex >= 0 ? firstIndex : terminals.length, 0, hydrated)

      const nextIndex = new Map<string, string>()
      for (const terminal of terminals) {
        if (terminal.ptyId) nextIndex.set(terminal.ptyId, terminal.id)
      }

      return {
        terminals,
        ptyIdIndex: nextIndex,
        activeTerminalId:
          state.activeTerminalId === byPty?.id || state.activeTerminalId === byRecord?.id
            ? recordId
            : state.activeTerminalId
      }
    })
  },

  /**
   * Ensure a hydrated terminal has a fresh host-authorized resume grant. A
   * running record with an in-memory claim is already reconciled and returns
   * immediately; all other records use the narrow resume path and never spawn.
   */
  resumeTerminalResource: (id: string): Promise<IpcResult<TerminalReplayCoverage | null>> => {
    const existing = terminalResumeInFlight.get(id)
    if (existing) return existing

    const task = (async (): Promise<IpcResult<TerminalReplayCoverage | null>> => {
      const terminal = get().terminals.find((candidate) => candidate.id === id)
      if (!terminal?.ptyId) {
        return { success: false, error: 'Terminal unavailable', code: 'TERMINAL_NOT_FOUND' }
      }
      // Scope-less project terminals already hold the spawn-issued claim.
      // Requiring a Conversation id here closes them on mount and blanks the pane.
      if (terminal.healthStatus === 'running' && terminal.claim) {
        // Already reconciled: no host replay was requested, so the renderer
        // transcript remains the only source of continuity.
        return { success: true, data: null }
      }
      if (!terminal.conversationId) {
        return { success: false, error: 'Terminal unavailable', code: 'TERMINAL_NOT_FOUND' }
      }

      const descriptor: TerminalResourceDescriptor = {
        kind: 'terminal',
        terminalId: terminal.ptyId,
        terminalRecordId: terminal.id,
        conversationId: terminal.conversationId
      }
      let result: IpcResult<TerminalResumeGrant>
      try {
        result = await terminalApi.resume({
          conversationId: terminal.conversationId,
          terminalId: terminal.ptyId,
          lastSeq: terminal.resumeCursor ?? 0
        })
      } catch {
        result = { success: false, error: 'Terminal resume failed', code: 'NETWORK_ERROR' }
      }

      const current = get().terminals.find((candidate) => candidate.id === id)
      if (
        !current ||
        current.ptyId !== terminal.ptyId ||
        current.conversationId !== terminal.conversationId
      ) {
        return { success: false, error: 'Terminal resume failed', code: 'NETWORK_ERROR' }
      }

      if (result.success && result.data.terminal.id === terminal.ptyId && result.data.claim) {
        get().hydrateTerminalResource(descriptor, result.data, terminal.projectId)
        return {
          success: true,
          data: {
            latestSeq: result.data.terminal.latestSeq,
            gap: result.data.terminal.gap
          }
        }
      }

      get().hydrateTerminalResource(descriptor, undefined, terminal.projectId)
      if (result.success) {
        return { success: false, error: 'Terminal resume failed', code: 'NETWORK_ERROR' }
      }
      return result
    })()

    terminalResumeInFlight.set(id, task)
    const clearInFlight = (): void => {
      if (terminalResumeInFlight.get(id) === task) terminalResumeInFlight.delete(id)
    }
    void task.then(clearInFlight, clearInFlight)
    return task
  },

  setTerminalPtyId: (id: string, ptyId: string): boolean => {
    let didSet = false
    set((state) => {
      const target = state.terminals.find((t) => t.id === id)
      if (!target) {
        return state
      }

      if (target.ptyId && target.ptyId !== ptyId) {
        return state
      }

      const existingOwner = state.ptyIdIndex.get(ptyId)
      if (existingOwner && existingOwner !== id) {
        return state
      }

      const newIndex = new Map(state.ptyIdIndex)
      if (target.ptyId && target.ptyId !== ptyId) {
        newIndex.delete(target.ptyId)
      }
      newIndex.set(ptyId, id)
      didSet = true

      return {
        terminals: state.terminals.map((t) => (t.id === id ? { ...t, ptyId } : t)),
        ptyIdIndex: newIndex
      }
    })
    return didSet
  },

  /**
   * CAP-3: set (or clear, with `undefined`) the in-memory lease credential on
   * the terminal whose `ptyId` matches. Linear scan over `terminals` — this
   * does NOT consult the ptyIdIndex; the claim is written before/without any
   * index guarantee, and a scan keeps the behavior honest for records whose
   * ptyId predates the current index state. The claim is never persisted.
   */
  setTerminalClaim: (ptyId: string, claim: string | undefined): void => {
    set((state) => {
      const hasTarget = state.terminals.some((t) => t.ptyId === ptyId)
      if (!hasTarget) {
        return state
      }

      return {
        terminals: state.terminals.map((t) => (t.ptyId === ptyId ? { ...t, claim } : t))
      }
    })
  },

  findTerminalByPtyId: (ptyId: string): Terminal | undefined => {
    const state = get()
    const terminalId = state.ptyIdIndex.get(ptyId)
    if (terminalId) {
      return state.terminals.find((t) => t.id === terminalId)
    }
    // Fallback to linear scan (for terminals set before index existed)
    return state.terminals.find((t) => t.ptyId === ptyId)
  },

  setTerminalAgentMetadata: (id: string, meta: TerminalAgentMetadata): void => {
    set((state) => ({
      terminals: state.terminals.map((t) =>
        t.id === id
          ? {
              ...t,
              kind: 'agent',
              agentId: meta.agentId,
              agentName: meta.agentName,
              agentProgram: meta.agentProgram,
              agentArgs: meta.agentArgs
            }
          : t
      )
    }))
  },

  updateTerminalCwd: (id: string, cwd: string): void => {
    set((state) => ({
      terminals: state.terminals.map((t) => (t.id === id ? { ...t, cwd } : t))
    }))
  },

  updateTerminalGitBranch: (id: string, gitBranch: string | null): void => {
    set((state) => ({
      terminals: state.terminals.map((t) => (t.id === id ? { ...t, gitBranch } : t))
    }))
  },

  updateTerminalGitStatus: (id: string, gitStatus: GitStatus | null): void => {
    set((state) => ({
      terminals: state.terminals.map((t) => (t.id === id ? { ...t, gitStatus } : t))
    }))
  },

  updateTerminalExitCode: (id: string, exitCode: number | null): void => {
    set((state) => ({
      terminals: state.terminals.map((t) => (t.id === id ? { ...t, lastExitCode: exitCode } : t))
    }))
  },

  updateTerminalScrollback: (id: string, scrollback: string[] | undefined): void => {
    set((state) => {
      const target = state.terminals.find((t) => t.id === id)
      if (!target || target.pendingScrollback === scrollback) {
        return state
      }

      return {
        terminals: state.terminals.map((t) =>
          t.id === id ? { ...t, pendingScrollback: scrollback } : t
        )
      }
    })
  },

  appendTranscript: (ptyId: string, data: string): void => {
    if (!data) return

    set((state) => {
      const hasTarget = state.terminals.some((t) => t.ptyId === ptyId)
      if (!hasTarget) {
        return state
      }

      return {
        terminals: state.terminals.map((t) => {
          if (t.ptyId !== ptyId) {
            return t
          }

          const combined = (t.transcript || '') + data
          const trimmed = trimTranscriptToMaxChars(combined)

          return {
            ...t,
            transcript: trimmed,
            // A trim drops the OLDEST bytes, which may include a DEC mode
            // transition the PTY has since left. The cached-remount replay
            // writes the transcript raw onto a live instance — deliberately,
            // since its modes are already current — so it has neither the
            // `transcriptLooksPartial` heuristic nor `buildRehydrateSequences`
            // to fall back on. That leaves trim-induced mode loss the one
            // silently wrong replay, so mark it here and let the replay
            // telemetry carry it.
            transcriptTrimmed: t.transcriptTrimmed || trimmed.length !== combined.length
          }
        })
      }
    })
  },

  peekTranscript: (ptyId: string): string => {
    const target = get().terminals.find((t) => t.ptyId === ptyId)
    return target?.transcript ?? ''
  },

  consumeTranscript: (ptyId: string): string => {
    let consumed = ''

    set((state) => {
      const target = state.terminals.find((t) => t.ptyId === ptyId && t.transcript)
      if (!target) {
        return state
      }

      consumed = target.transcript ?? ''

      return {
        terminals: state.terminals.map((t) => {
          if (t.ptyId !== ptyId || !t.transcript) {
            return t
          }

          return {
            ...t,
            transcript: undefined,
            transcriptTrimmed: undefined
          }
        })
      }
    })

    return consumed
  },

  appendDetachedOutput: (ptyId: string, data: string): void => {
    if (!data) return

    set((state) => {
      const hasTarget = state.terminals.some((t) => t.ptyId === ptyId)
      if (!hasTarget) {
        return state
      }

      return {
        terminals: state.terminals.map((t) => {
          if (t.ptyId !== ptyId) {
            return t
          }

          return {
            ...t,
            detachedOutput: trimTranscriptToMaxChars((t.detachedOutput || '') + data)
          }
        })
      }
    })
  },

  consumeDetachedOutput: (ptyId: string): string => {
    let consumed = ''

    set((state) => {
      const target = state.terminals.find((t) => t.ptyId === ptyId && t.detachedOutput)
      if (!target) {
        return state
      }

      consumed = target.detachedOutput || ''

      return {
        terminals: state.terminals.map((t) => {
          if (t.ptyId !== ptyId || !t.detachedOutput) {
            return t
          }

          return {
            ...t,
            detachedOutput: ''
          }
        })
      }
    })

    return consumed
  },

  setRendererAttached: (ptyId: string, attached: boolean): void => {
    // F3/R-11: observability only. A miss here means the detached collector
    // (use-terminal-detached-output.ts) keeps writing this pty's bytes into the
    // transcript for its whole lifetime, so the failure cost is unbounded
    // transcript growth. Escalating this warn to a thrown error or a returned
    // failure is a deliberate follow-up decision, to be taken after the
    // real-world frequency of this warning has been observed - do not upgrade it
    // in the same change that first makes it visible.
    // The check is hoisted out of the `set` updater on purpose: a zustand
    // updater must stay pure, or the warning double-fires under StrictMode.
    if (!get().terminals.some((t) => t.ptyId === ptyId)) {
      void logFrontendError({
        level: 'warn',
        source: 'terminal-store.setRendererAttached',
        message: `code=TERMINAL_NOT_FOUND ptyId=${ptyId} attached=${attached}`
      })
      return
    }

    set((state) => {
      const target = state.terminals.find((t) => t.ptyId === ptyId)
      if (!target) {
        return state
      }

      const currentCount = target.rendererAttachmentCount ?? 0
      const nextCount = attached ? currentCount + 1 : Math.max(0, currentCount - 1)

      if (nextCount === currentCount) {
        return state
      }

      return {
        terminals: state.terminals.map((t) => {
          if (t.ptyId !== ptyId) {
            return t
          }

          return {
            ...t,
            rendererAttachmentCount: nextCount
          }
        })
      }
    })
  },

  setTerminalHealthStatus: (id: string, status: TerminalHealthStatus): void => {
    set((state) => ({
      terminals: state.terminals.map((t) => (t.id === id ? { ...t, healthStatus: status } : t))
    }))
  },

  /**
   * Record the host-authoritative replay cursor returned by an attach.
   *
   * `hydrateTerminalResource` already closes this loop for the `resume` path;
   * the `watch` path had no equivalent, so its cursor stayed pinned at whatever
   * the last resume grant set and every later watch replayed the same backlog.
   *
   * Monotonic on purpose: a resume and a watch can settle in either order, and
   * taking the smaller value would rewind the cursor and widen the next replay.
   */
  setTerminalResumeCursor: (id: string, cursor: number): void => {
    if (!Number.isFinite(cursor) || cursor < 0) return
    set((state) => ({
      terminals: state.terminals.map((t) =>
        t.id === id ? { ...t, resumeCursor: Math.max(t.resumeCursor ?? 0, cursor) } : t
      )
    }))
  },

  setTerminalHidden: (id: string, isHidden: boolean): void => {
    set((state) => ({
      terminals: state.terminals.map((t) => {
        if (t.id !== id) {
          return t
        }

        if (t.isHidden === isHidden) {
          return t
        }

        return {
          ...t,
          viewState: isHidden ? 'hidden' : 'visible',
          isHidden,
          hiddenSince: isHidden ? Date.now() : undefined
        }
      })
    }))
  },

  setTerminalNeedsAttention: (id: string, value: boolean): void => {
    set((state) => {
      const target = state.terminals.find((t) => t.id === id)
      // No-op when the flag is already at the requested value to avoid needless re-renders.
      if (!target || (target.needsAttention ?? false) === value) {
        return state
      }
      return {
        terminals: state.terminals.map((t) => (t.id === id ? { ...t, needsAttention: value } : t))
      }
    })
  },

  setAppHidden: (isHidden: boolean): void => {
    set((state) => {
      // Avoid allocating a new array if every terminal already has the correct state
      if (state.terminals.every((t) => t.isAppHidden === isHidden)) {
        return state
      }

      return {
        terminals: state.terminals.map((t) => {
          if (t.isAppHidden === isHidden) {
            return t
          }

          return {
            ...t,
            isAppHidden: isHidden,
            appHiddenSince: isHidden ? Date.now() : undefined
          }
        })
      }
    })
  },

  /** @deprecated Use updateTerminalActivityBatch instead */
  updateTerminalActivity: (id: string, hasActivity: boolean): void => {
    set((state) => ({
      terminals: state.terminals.map((t) => (t.id === id ? { ...t, hasActivity } : t))
    }))
  },

  /** @deprecated Use updateTerminalActivityBatch instead */
  updateTerminalLastActivityTimestamp: (id: string, timestamp: number): void => {
    set((state) => ({
      terminals: state.terminals.map((t) =>
        t.id === id ? { ...t, lastActivityTimestamp: timestamp } : t
      )
    }))
  },

  restartTerminal: (id: string): void => {
    // Compatibility action for legacy callers that only reset renderer state.
    // User-facing restart paths use restartTerminalResource below so the live
    // PTY is explicitly terminated and re-spawned in the same Conversation.
    set((state) => ({
      terminals: state.terminals.map((terminal) =>
        terminal.id === id
          ? {
              ...terminal,
              healthStatus: 'running',
              transcript: undefined,
              transcriptTrimmed: undefined,
              pendingScrollback: undefined,
              pendingModes: undefined
            }
          : terminal
      ),
      activeTerminalId: id,
      recentTerminalIds: promoteRecentTerminal(state.recentTerminalIds, id)
    }))
  },

  restartTerminalResource: async (id: string): Promise<boolean> => {
    const terminal = get().terminals.find((candidate) => candidate.id === id)
    if (!terminal?.ptyId) return false

    const terminated = await terminalApi.terminate(terminal.ptyId)
    if (!terminated.success) {
      get().recordTerminalCleanupFailure(terminated)
      return false
    }

    const previousPtyId = terminal.ptyId
    set((state) => {
      const nextIndex = new Map(state.ptyIdIndex)
      nextIndex.delete(previousPtyId)
      return {
        terminals: state.terminals.map((candidate) =>
          candidate.id === id
            ? {
                ...candidate,
                ptyId: undefined,
                claim: undefined,
                resumeCursor: undefined,
                healthStatus: 'crashed'
              }
            : candidate
        ),
        ptyIdIndex: nextIndex
      }
    })

    const spawned = await terminalApi.spawn({
      ...(terminal.conversationId ? { conversationId: terminal.conversationId } : {}),
      projectId: terminal.projectId,
      shell: terminal.agentProgram ? undefined : terminal.shell,
      cwd: terminal.cwd,
      kind: terminal.kind ?? 'shell',
      program: terminal.agentProgram,
      args: terminal.agentArgs
    })
    if (!spawned.success) return false

    set((state) => {
      if (!state.terminals.some((candidate) => candidate.id === id)) return state
      const nextIndex = new Map(state.ptyIdIndex)
      nextIndex.set(spawned.data.id, id)
      return {
        terminals: state.terminals.map((candidate) =>
          candidate.id === id
            ? {
                ...candidate,
                ptyId: spawned.data.id,
                claim: spawned.data.claim,
                resumeCursor: 0,
                healthStatus: 'running',
                viewState: 'visible',
                isHidden: false,
                hiddenSince: undefined,
                transcript: undefined,
                transcriptTrimmed: undefined,
                pendingScrollback: undefined,
                pendingModes: undefined
              }
            : candidate
        ),
        ptyIdIndex: nextIndex,
        activeTerminalId: id,
        recentTerminalIds: promoteRecentTerminal(state.recentTerminalIds, id)
      }
    })
    return true
  },

  updateTerminalActivityBatch: (id: string, hasActivity: boolean, timestamp: number): void => {
    set((state) => ({
      terminals: state.terminals.map((t) =>
        t.id === id ? { ...t, hasActivity, lastActivityTimestamp: timestamp } : t
      )
    }))
  },

  clearTerminalPtyId: (ptyId: string): void => {
    set((state) => {
      const newIndex = new Map(state.ptyIdIndex)
      newIndex.delete(ptyId)
      return {
        terminals: state.terminals.map((t) =>
          // CAP-3: the claim is bound to the PTY — dropping the ptyId drops
          // the lease with it.
          t.ptyId === ptyId
            ? { ...t, ptyId: undefined, claim: undefined, resumeCursor: undefined }
            : t
        ),
        ptyIdIndex: newIndex
      }
    })
  },

  truncateHiddenTerminalBuffers: (): void => {
    const now = Date.now()
    set((state) => ({
      terminals: state.terminals.map((t) => {
        const hiddenSince = t.appHiddenSince ?? t.hiddenSince
        const isEligibleForTruncation =
          (t.isAppHidden || t.isHidden) &&
          hiddenSince !== undefined &&
          now - hiddenSince > HIDDEN_BUFFER_TRUNCATION_DELAY

        if (!isEligibleForTruncation) {
          return t
        }

        const nextScrollback =
          t.pendingScrollback && t.pendingScrollback.length > TRUNCATED_BUFFER_SIZE
            ? t.pendingScrollback.slice(-TRUNCATED_BUFFER_SIZE)
            : t.pendingScrollback

        const nextTranscript = t.transcript
          ? (() => {
              const trimmedMax = trimTranscriptToMaxChars(t.transcript!)
              return trimmedMax === t.transcript && t.transcript!.length <= TRUNCATED_BUFFER_SIZE
                ? t.transcript
                : trimTranscriptToRecentLines(trimmedMax)
            })()
          : t.transcript

        const nextDetachedOutput = t.detachedOutput
          ? (() => {
              const trimmedMax = trimTranscriptToMaxChars(t.detachedOutput!)
              return trimmedMax === t.detachedOutput &&
                t.detachedOutput!.length <= TRUNCATED_BUFFER_SIZE
                ? t.detachedOutput
                : trimTranscriptToRecentLines(trimmedMax)
            })()
          : t.detachedOutput

        if (
          nextScrollback === t.pendingScrollback &&
          nextTranscript === t.transcript &&
          nextDetachedOutput === t.detachedOutput
        ) {
          return t
        }

        return {
          ...t,
          pendingScrollback: nextScrollback,
          transcript: nextTranscript,
          // Hidden-buffer truncation drops the oldest bytes for the same
          // reason `appendTranscript`'s cap does, and costs the replay the
          // same mode transitions.
          transcriptTrimmed: t.transcriptTrimmed || nextTranscript !== t.transcript,
          detachedOutput: nextDetachedOutput
        }
      })
    }))
  },

  cleanupProjectTerminals: (projectId: string): void => {
    // Project removal/navigation is not PTY termination. Preserve Conversation
    // terminal records and claims; only hide their renderer views.
    set((state) => ({
      terminals: state.terminals.map((terminal) =>
        terminal.projectId === projectId
          ? {
              ...terminal,
              viewState: 'hidden',
              isHidden: true,
              hiddenSince: terminal.hiddenSince ?? Date.now()
            }
          : terminal
      ),
      activeTerminalId: state.terminals.some(
        (terminal) => terminal.id === state.activeTerminalId && terminal.projectId === projectId
      )
        ? ''
        : state.activeTerminalId
    }))
  },

  getTerminalCount: (): number => {
    return get().terminals.length
  },

  isTerminalLimitReached: (): boolean => {
    return get().terminals.length >= GLOBAL_TERMINAL_LIMIT
  }
}))

// Helper to cleanup project terminals from outside the store
export function cleanupProjectTerminals(projectId: string): void {
  useTerminalStore.getState().cleanupProjectTerminals(projectId)
}

// Selectors for performance (selective subscriptions)
// These selectors use the project store's activeProjectId for filtering

export function useTerminals(): Terminal[] {
  const activeProjectId = useProjectStore((state) => state.activeProjectId)
  return useTerminalStore(
    useShallow((state) => state.terminals.filter((t) => t.projectId === activeProjectId))
  )
}

export function useConversationTerminals(conversationId: string | null): Terminal[] {
  return useTerminalStore(
    useShallow((state) =>
      conversationId
        ? state.terminals.filter((terminal) => terminal.conversationId === conversationId)
        : []
    )
  )
}

export function useAllTerminals(): Terminal[] {
  return useTerminalStore(useShallow((state) => state.terminals))
}

export function useActiveTerminal(): Terminal | undefined {
  const activeProjectId = useProjectStore((state) => state.activeProjectId)
  return useTerminalStore((state) => {
    const projectTerminals = state.terminals.filter((t) => t.projectId === activeProjectId)
    // Find by activeTerminalId, or fall back to first terminal in project
    const activeById = projectTerminals.find((t) => t.id === state.activeTerminalId)
    return activeById || projectTerminals[0]
  })
}

export function useActiveTerminalId(): string {
  return useTerminalStore((state) => state.activeTerminalId)
}

export function useTerminalActions(): Pick<
  TerminalState,
  | 'selectTerminal'
  | 'addTerminal'
  | 'closeTerminal'
  | 'closeTerminalView'
  | 'reopenTerminalView'
  | 'terminateTerminalResource'
  | 'recordTerminalCleanupFailure'
  | 'retryTerminalCleanup'
  | 'restartTerminalResource'
  | 'renameTerminal'
  | 'reorderTerminals'
  | 'updateTerminalCwd'
  | 'updateTerminalScrollback'
  | 'appendTranscript'
  | 'peekTranscript'
  | 'consumeTranscript'
  | 'appendDetachedOutput'
  | 'consumeDetachedOutput'
  | 'setRendererAttached'
  | 'setTerminalPtyId'
  | 'clearTerminalPtyId'
> {
  return useTerminalStore(
    useShallow((state) => ({
      selectTerminal: state.selectTerminal,
      addTerminal: state.addTerminal,
      closeTerminal: state.closeTerminal,
      closeTerminalView: state.closeTerminalView,
      reopenTerminalView: state.reopenTerminalView,
      terminateTerminalResource: state.terminateTerminalResource,
      recordTerminalCleanupFailure: state.recordTerminalCleanupFailure,
      retryTerminalCleanup: state.retryTerminalCleanup,
      restartTerminalResource: state.restartTerminalResource,
      renameTerminal: state.renameTerminal,
      reorderTerminals: state.reorderTerminals,
      updateTerminalCwd: state.updateTerminalCwd,
      updateTerminalScrollback: state.updateTerminalScrollback,
      appendTranscript: state.appendTranscript,
      peekTranscript: state.peekTranscript,
      consumeTranscript: state.consumeTranscript,
      appendDetachedOutput: state.appendDetachedOutput,
      consumeDetachedOutput: state.consumeDetachedOutput,
      setRendererAttached: state.setRendererAttached,
      setTerminalPtyId: state.setTerminalPtyId,
      clearTerminalPtyId: state.clearTerminalPtyId
    }))
  )
}

/**
 * Optimized selector that returns a Set of project IDs with active terminal activity.
 * Uses useShallow to prevent re-renders unless the set of active projects actually changes.
 */
export function useProjectsWithActivity(): string[] {
  return useTerminalStore(
    useShallow((state) => {
      const activeProjectIds = new Set<string>()
      for (const t of state.terminals) {
        // Indikator menyala jika:
        // 1. Ada aktivitas output (hasActivity)
        // 2. Sedang proses awal loading/spawn (status running tapi PTY belum siap)
        if (t.projectId && (t.hasActivity || (t.healthStatus === 'running' && !t.ptyId))) {
          activeProjectIds.add(t.projectId)
        }
      }
      return Array.from(activeProjectIds).sort()
    })
  )
}

/**
 * Returns a Set of project IDs that have at least one crashed or disconnected terminal.
 */
export function useProjectsWithErrors(): Set<string> {
  return useTerminalStore(
    useShallow((state) => {
      const errorProjectIds = new Set<string>()
      for (const t of state.terminals) {
        if (t.projectId && (t.healthStatus === 'crashed' || t.healthStatus === 'disconnected')) {
          errorProjectIds.add(t.projectId)
        }
      }
      return errorProjectIds
    })
  )
}
