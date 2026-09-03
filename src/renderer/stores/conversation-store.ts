import type {
  ConversationAggregateMutationOutcome,
  ConversationId,
  ConversationRecordV2,
  ExecutionTarget,
  ProjectAttachment
} from '@shared/types/conversation.types'
import { isConversationId } from '@shared/types/conversation.types'
import type {
  ConversationHostStatus,
  ConversationOpenOutcome
} from '@shared/types/conversation-api.types'
import type { ConversationLifecycleOutcome } from '@shared/types/conversation-lifecycle.types'
import type { RecoveryItemV1 } from '@shared/types/conversation-recovery.types'
import { create } from 'zustand'
import { useShallow } from 'zustand/shallow'
import { notifyAgentSkillsChanged } from '@/lib/agent-skills-events'
import { conversationApi } from '@/lib/conversation-api'
import {
  fetchHostBoundSession,
  isLiveAcpSession,
  resolveConversationSessionId
} from '@/lib/conversation-binding'
import { mergeConversationTitle } from '@/lib/conversation-title'
import { logFrontendError } from '@/lib/log-api'
import { useSessionWorkspaceSyncStore } from '@/stores/session-workspace-sync-store'
import { useWorkspaceStore } from '@/stores/workspace-store'

export type ConversationProjectFilter = string | 'projectless' | null

export interface ConversationStoreError {
  code: string
  message: string
}

interface ConversationState {
  summariesById: Record<ConversationId, ConversationRecordV2>
  conversationIds: ConversationId[]
  detailsById: Record<ConversationId, ConversationOpenOutcome | undefined>
  recoveryItems: RecoveryItemV1[]
  activeConversationId: ConversationId | null
  lifecycleRevisionById: Record<ConversationId, number | undefined>
  deletedRevisionById: Record<ConversationId, number | undefined>
  lifecycleOutcomeKeyById: Record<ConversationId, string | undefined>
  activationEpoch: number
  activationRouteValue: string | null
  searchQuery: string
  projectFilter: ConversationProjectFilter
  loadingList: boolean
  openingById: Record<ConversationId, boolean | undefined>
  aggregateBusyById: Record<ConversationId, boolean | undefined>
  errorsById: Record<ConversationId, ConversationStoreError | undefined>
  listError: ConversationStoreError | null
  replaceSummaries: (summaries: ConversationRecordV2[]) => void
  renameConversation: (
    conversationId: ConversationId,
    title: string
  ) => Promise<ConversationRecordV2>
  setRecoveryItems: (items: RecoveryItemV1[]) => void
  setSearchQuery: (query: string) => void
  setProjectFilter: (projectFilter: ConversationProjectFilter) => void
  setActiveConversationId: (conversationId: ConversationId | null) => void
  beginConversationActivation: (conversationId: string) => number
  activateConversation: (conversationId: string, activationEpoch: number) => Promise<boolean>
  cancelConversationActivation: (activationEpoch: number) => void
  loadConversations: () => Promise<boolean>
  openConversation: (conversationId: ConversationId) => Promise<ConversationOpenOutcome | null>
  applyLifecycleOutcome: (outcome: ConversationLifecycleOutcome) => boolean
  applyAggregateOutcome: (outcome: ConversationAggregateMutationOutcome) => boolean
  attachProject: (
    conversationId: ConversationId,
    attachment: ProjectAttachment
  ) => Promise<ConversationAggregateMutationOutcome | null>
  detachProject: (
    conversationId: ConversationId
  ) => Promise<ConversationAggregateMutationOutcome | null>
  updateExecutionTarget: (
    conversationId: ConversationId,
    executionTarget: ExecutionTarget
  ) => Promise<ConversationAggregateMutationOutcome | null>
  clearConversationError: (conversationId: ConversationId) => void
  reset: () => void
}

const initialState = {
  summariesById: {},
  conversationIds: [],
  detailsById: {},
  recoveryItems: [],
  activeConversationId: null,
  lifecycleRevisionById: {},
  deletedRevisionById: {},
  lifecycleOutcomeKeyById: {},
  activationEpoch: 0,
  activationRouteValue: null,
  searchQuery: '',
  projectFilter: null,
  loadingList: false,
  openingById: {},
  aggregateBusyById: {},
  errorsById: {},
  listError: null
} satisfies Pick<
  ConversationState,
  | 'summariesById'
  | 'conversationIds'
  | 'detailsById'
  | 'recoveryItems'
  | 'activeConversationId'
  | 'lifecycleRevisionById'
  | 'deletedRevisionById'
  | 'lifecycleOutcomeKeyById'
  | 'activationEpoch'
  | 'activationRouteValue'
  | 'searchQuery'
  | 'projectFilter'
  | 'loadingList'
  | 'openingById'
  | 'aggregateBusyById'
  | 'errorsById'
  | 'listError'
>

function indexSummaries(summaries: ConversationRecordV2[]): {
  summariesById: Record<ConversationId, ConversationRecordV2>
  conversationIds: ConversationId[]
} {
  const summariesById: Record<ConversationId, ConversationRecordV2> = {}
  const conversationIds: ConversationId[] = []
  for (const summary of summaries) {
    if (!isConversationId(summary.conversationId)) continue
    summariesById[summary.conversationId] = summary
    conversationIds.push(summary.conversationId)
  }
  conversationIds.sort((left, right) => {
    const leftCreated = summariesById[left]?.createdAtUtc ?? ''
    const rightCreated = summariesById[right]?.createdAtUtc ?? ''
    return rightCreated.localeCompare(leftCreated)
  })
  return { summariesById, conversationIds }
}

function stableError(code: string, message?: string): ConversationStoreError {
  return { code, message: message || code }
}

function sameValue(left: unknown, right: unknown): boolean {
  return JSON.stringify(left) === JSON.stringify(right)
}

function identityMatchesRecord(
  identity: ConversationAggregateMutationOutcome['identityBefore'],
  record: ConversationRecordV2
): boolean {
  return (
    identity.conversationId === record.conversationId &&
    identity.createdAtUtc === record.createdAtUtc &&
    sameValue(identity.creationPartition, record.creationPartition) &&
    identity.workspaceCwd === record.workspaceCwd
  )
}

function validateAggregateOutcome(
  outcome: ConversationAggregateMutationOutcome,
  current: ConversationRecordV2 | undefined
): ConversationStoreError | null {
  if (
    !identityMatchesRecord(outcome.identityBefore, outcome.conversation) ||
    !identityMatchesRecord(outcome.identityAfter, outcome.conversation) ||
    !sameValue(outcome.identityBefore, outcome.identityAfter) ||
    outcome.conversationId !== outcome.conversation.conversationId
  ) {
    return stableError(
      'CONVERSATION_IDENTITY_CHANGED',
      'The host returned an aggregate mutation that changed immutable Conversation identity.'
    )
  }
  if (
    outcome.status !== 'updated' ||
    outcome.revision !== outcome.conversation.lastSeq ||
    outcome.revision !== outcome.previousRevision + 1 ||
    (current &&
      (outcome.previousRevision !== current.lastSeq ||
        !identityMatchesRecord(outcome.identityBefore, current)))
  ) {
    return stableError(
      'CONVERSATION_CONFLICT',
      'The aggregate mutation result is stale or does not advance the expected revision.'
    )
  }
  if (
    !sameValue(outcome.projectAttachment, outcome.conversation.projectAttachment) ||
    !sameValue(outcome.executionTarget, outcome.conversation.executionTarget)
  ) {
    return stableError(
      'CONVERSATION_AGGREGATE_INVALID',
      'The aggregate mutation result does not match the returned Conversation record.'
    )
  }
  if (!current) return null
  const attachmentChanged = !sameValue(current.projectAttachment, outcome.projectAttachment)
  const targetChanged = !sameValue(current.executionTarget, outcome.executionTarget)
  const actionIsValid =
    (outcome.action === 'attachProject' &&
      current.projectAttachment === null &&
      outcome.projectAttachment !== null &&
      !targetChanged) ||
    (outcome.action === 'detachProject' &&
      current.projectAttachment !== null &&
      outcome.projectAttachment === null &&
      !targetChanged) ||
    (outcome.action === 'updateExecutionTarget' && !attachmentChanged && targetChanged)
  return actionIsValid
    ? null
    : stableError(
        'CONVERSATION_AGGREGATE_INVALID',
        'The aggregate mutation changed fields outside its declared action.'
      )
}

function currentConversationRecord(
  state: ConversationState,
  conversationId: ConversationId
): ConversationRecordV2 | undefined {
  return state.summariesById[conversationId] ?? state.detailsById[conversationId]?.conversation
}

export function getCurrentConversation(
  state: ConversationState,
  conversationId: ConversationId
): ConversationRecordV2 | undefined {
  return currentConversationRecord(state, conversationId)
}

function indexRevisionOrderedSummaries(
  state: ConversationState,
  summaries: ConversationRecordV2[]
): ReturnType<typeof indexSummaries> {
  const ordered = new Map<ConversationId, ConversationRecordV2>()
  for (const incoming of summaries) {
    if (!isConversationId(incoming.conversationId)) continue
    if (incoming.lifecycleState === 'deleted') continue
    const knownRevision = state.lifecycleRevisionById[incoming.conversationId] ?? -1
    const deletedRevision = state.deletedRevisionById[incoming.conversationId]
    if (deletedRevision !== undefined && incoming.lastSeq <= deletedRevision) continue
    const current = currentConversationRecord(state, incoming.conversationId)
    if (incoming.lastSeq < knownRevision) {
      if (current && current.lastSeq >= knownRevision) ordered.set(incoming.conversationId, current)
      continue
    }
    ordered.set(incoming.conversationId, incoming)
  }
  return indexSummaries(Array.from(ordered.values()))
}

function withSummaryRevisions(
  revisions: Record<ConversationId, number | undefined>,
  summariesById: Record<ConversationId, ConversationRecordV2>
): Record<ConversationId, number | undefined> {
  const next = { ...revisions }
  for (const summary of Object.values(summariesById)) {
    next[summary.conversationId] = Math.max(next[summary.conversationId] ?? -1, summary.lastSeq)
  }
  return next
}

function lifecycleOutcomeKey(outcome: ConversationLifecycleOutcome): string {
  return `${outcome.status}:${outcome.action}:${outcome.revision}`
}

function withoutConversationKey<T>(
  record: Record<ConversationId, T>,
  conversationId: ConversationId
): Record<ConversationId, T> {
  const next = { ...record }
  delete next[conversationId]
  return next
}

function activationIsCurrent(
  state: ConversationState,
  conversationId: string,
  activationEpoch: number
): boolean {
  return state.activationEpoch === activationEpoch && state.activationRouteValue === conversationId
}

function logStaleActivation(
  conversationId: ConversationId,
  activationEpoch: number,
  stage: string
): void {
  void logFrontendError({
    level: 'warn',
    source: 'conversation-store.activation',
    message: `conversationId=${conversationId} epoch=${activationEpoch} stage=${stage} code=STALE_ACTIVATION`
  })
}

function bindingSessionId(
  state: {
    sessions: Record<string, { id: string; conversationId?: string; status: string }>
    sessionIndex: Array<{ id: string; conversationId?: string }>
  },
  conversationId: ConversationId
): string | null {
  return resolveConversationSessionId(state, conversationId)
}

export const useConversationStore = create<ConversationState>((set, get) => ({
  ...initialState,

  replaceSummaries: (summaries) =>
    set((state) => {
      const indexed = indexRevisionOrderedSummaries(state, summaries)
      return {
        ...indexed,
        lifecycleRevisionById: withSummaryRevisions(
          state.lifecycleRevisionById,
          indexed.summariesById
        )
      }
    }),

  renameConversation: async (conversationId, title) => {
    const result = await conversationApi.renameConversation(conversationId, title)
    if (!result.success) {
      throw new Error(result.error || result.code)
    }
    set((state) => ({
      summariesById: { ...state.summariesById, [conversationId]: result.data }
    }))
    return result.data
  },

  setRecoveryItems: (recoveryItems) => set({ recoveryItems: [...recoveryItems] }),

  setSearchQuery: (searchQuery) => set({ searchQuery }),

  setProjectFilter: (projectFilter) => set({ projectFilter }),

  setActiveConversationId: (activeConversationId) => {
    set({ activeConversationId })
    useSessionWorkspaceSyncStore.getState().setActiveConversationId(activeConversationId)
  },

  beginConversationActivation: (conversationId) => {
    let activationEpoch = 0
    let previousRouteValue: string | null = null
    set((state) => {
      activationEpoch = state.activationEpoch + 1
      previousRouteValue = state.activationRouteValue
      const openingById = { ...state.openingById }
      if (previousRouteValue) openingById[previousRouteValue] = false
      openingById[conversationId] = true
      return {
        activationEpoch,
        activationRouteValue: conversationId,
        openingById,
        errorsById: { ...state.errorsById, [conversationId]: undefined }
      }
    })
    if (previousRouteValue && isConversationId(previousRouteValue)) {
      useSessionWorkspaceSyncStore.getState().setRestoreInProgress(previousRouteValue, false)
    }
    return activationEpoch
  },

  cancelConversationActivation: (activationEpoch) => {
    let cancelledConversationId: ConversationId | null = null
    set((state) => {
      if (state.activationEpoch !== activationEpoch) return {}
      const routeValue = state.activationRouteValue
      const openingById = { ...state.openingById }
      if (routeValue) openingById[routeValue] = false
      if (routeValue && isConversationId(routeValue)) cancelledConversationId = routeValue
      return {
        activationEpoch: state.activationEpoch + 1,
        activationRouteValue: null,
        openingById
      }
    })
    if (cancelledConversationId) {
      useSessionWorkspaceSyncStore.getState().setRestoreInProgress(cancelledConversationId, false)
    }
  },

  loadConversations: async () => {
    set({ loadingList: true, listError: null })
    try {
      const result = await conversationApi.listConversations()
      if (!result.success) {
        set({ loadingList: false, listError: stableError(result.code, result.error) })
        void logFrontendError({
          level: 'warn',
          source: 'conversation-store.list',
          message: `code=${result.code}`
        })
        return false
      }
      set((state) => {
        const indexed = indexRevisionOrderedSummaries(state, result.data)
        return {
          ...indexed,
          lifecycleRevisionById: withSummaryRevisions(
            state.lifecycleRevisionById,
            indexed.summariesById
          ),
          loadingList: false,
          listError: null
        }
      })
      return true
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      set({ loadingList: false, listError: stableError('CONVERSATION_LIST_FAILED', message) })
      void logFrontendError({
        level: 'warn',
        source: 'conversation-store.list',
        message: 'code=CONVERSATION_LIST_FAILED'
      })
      return false
    }
  },

  openConversation: async (conversationId) => {
    if (!isConversationId(conversationId)) {
      const error = stableError(
        'CONVERSATION_INVALID_ID',
        'The Conversation address is not a canonical ConversationId.'
      )
      set((state) => ({
        errorsById: { ...state.errorsById, [conversationId]: error },
        openingById: { ...state.openingById, [conversationId]: false }
      }))
      void logFrontendError({
        level: 'warn',
        source: 'conversation-store.open',
        message: 'code=CONVERSATION_INVALID_ID'
      })
      return null
    }

    set((state) => ({
      openingById: { ...state.openingById, [conversationId]: true },
      errorsById: { ...state.errorsById, [conversationId]: undefined }
    }))
    try {
      const result = await conversationApi.openConversation(conversationId)
      if (!result.success) {
        set((state) => ({
          openingById: { ...state.openingById, [conversationId]: false },
          errorsById: {
            ...state.errorsById,
            [conversationId]: stableError(result.code, result.error)
          }
        }))
        void logFrontendError({
          level: 'warn',
          source: 'conversation-store.open',
          message: `conversationId=${conversationId} code=${result.code}`
        })
        return null
      }
      if (result.data.conversation.conversationId !== conversationId) {
        set((state) => ({
          openingById: { ...state.openingById, [conversationId]: false },
          errorsById: {
            ...state.errorsById,
            [conversationId]: stableError('CONVERSATION_OPEN_FAILED')
          }
        }))
        void logFrontendError({
          level: 'error',
          source: 'conversation-store.open',
          message: `conversationId=${conversationId} code=CONVERSATION_IDENTITY_MISMATCH`
        })
        return null
      }
      notifyAgentSkillsChanged(result.data.conversation.workspaceCwd)

      let opened: ConversationOpenOutcome | null = null
      set((state) => {
        const knownRevision = state.lifecycleRevisionById[conversationId] ?? -1
        const deletedRevision = state.deletedRevisionById[conversationId]
        const current = currentConversationRecord(state, conversationId)
        const incoming = result.data.conversation
        const conversation =
          deletedRevision !== undefined && incoming.lastSeq <= deletedRevision
            ? undefined
            : incoming.lastSeq < knownRevision && (!current || current.lastSeq < knownRevision)
              ? undefined
              : incoming.lastSeq < knownRevision
                ? current
                : incoming
        if (!conversation) {
          return {
            openingById: { ...state.openingById, [conversationId]: false },
            errorsById: {
              ...state.errorsById,
              [conversationId]: stableError('CONVERSATION_CONFLICT')
            }
          }
        }
        opened = {
          ...result.data,
          conversation: mergeConversationTitle(current, conversation)
        }
        const summaryAlreadyListed = Boolean(state.summariesById[conversationId])
        return {
          summariesById: {
            ...state.summariesById,
            [conversationId]: opened.conversation
          },
          conversationIds: summaryAlreadyListed
            ? state.conversationIds
            : [conversationId, ...state.conversationIds],
          detailsById: { ...state.detailsById, [conversationId]: opened },
          lifecycleRevisionById: {
            ...state.lifecycleRevisionById,
            [conversationId]: Math.max(knownRevision, conversation.lastSeq)
          },
          openingById: { ...state.openingById, [conversationId]: false },
          errorsById: { ...state.errorsById, [conversationId]: undefined }
        }
      })
      if (!opened) {
        void logFrontendError({
          level: 'warn',
          source: 'conversation-store.open',
          message: `conversationId=${conversationId} code=CONVERSATION_STALE_OPEN`
        })
      }
      return opened
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      set((state) => ({
        openingById: { ...state.openingById, [conversationId]: false },
        errorsById: {
          ...state.errorsById,
          [conversationId]: stableError('CONVERSATION_OPEN_FAILED', message)
        }
      }))
      void logFrontendError({
        level: 'warn',
        source: 'conversation-store.open',
        message: `conversationId=${conversationId} code=CONVERSATION_OPEN_FAILED`
      })
      return null
    }
  },

  activateConversation: async (routeValue, activationEpoch) => {
    const isCurrent = (): boolean => activationIsCurrent(get(), routeValue, activationEpoch)
    if (!isCurrent()) return false
    if (!isConversationId(routeValue)) {
      set((state) =>
        activationIsCurrent(state, routeValue, activationEpoch)
          ? {
              openingById: { ...state.openingById, [routeValue]: false },
              errorsById: {
                ...state.errorsById,
                [routeValue]: stableError(
                  'CONVERSATION_INVALID_ID',
                  'The Conversation address is not a canonical ConversationId.'
                )
              }
            }
          : {}
      )
      void logFrontendError({
        level: 'warn',
        source: 'conversation-store.activation',
        message: `epoch=${activationEpoch} stage=validate code=CONVERSATION_INVALID_ID`
      })
      return false
    }

    const conversationId = routeValue
    let openResult: Awaited<ReturnType<typeof conversationApi.openConversation>>
    try {
      openResult = await conversationApi.openConversation(conversationId)
    } catch (error) {
      if (!isCurrent()) {
        logStaleActivation(conversationId, activationEpoch, 'open-error')
        return false
      }
      const message = error instanceof Error ? error.message : String(error)
      set((state) => ({
        openingById: { ...state.openingById, [conversationId]: false },
        errorsById: {
          ...state.errorsById,
          [conversationId]: stableError('CONVERSATION_OPEN_FAILED', message)
        }
      }))
      void logFrontendError({
        level: 'warn',
        source: 'conversation-store.activation',
        message: `conversationId=${conversationId} epoch=${activationEpoch} stage=open code=CONVERSATION_OPEN_FAILED`
      })
      return false
    }
    if (!isCurrent()) {
      logStaleActivation(conversationId, activationEpoch, 'open')
      return false
    }
    if (!openResult.success) {
      set((state) => ({
        openingById: { ...state.openingById, [conversationId]: false },
        errorsById: {
          ...state.errorsById,
          [conversationId]: stableError(openResult.code, openResult.error)
        }
      }))
      void logFrontendError({
        level: 'warn',
        source: 'conversation-store.activation',
        message: `conversationId=${conversationId} epoch=${activationEpoch} stage=open code=${openResult.code}`
      })
      return false
    }
    if (openResult.data.conversation.conversationId !== conversationId) {
      set((state) => ({
        openingById: { ...state.openingById, [conversationId]: false },
        errorsById: {
          ...state.errorsById,
          [conversationId]: stableError('CONVERSATION_OPEN_FAILED')
        }
      }))
      void logFrontendError({
        level: 'error',
        source: 'conversation-store.activation',
        message: `conversationId=${conversationId} epoch=${activationEpoch} stage=open code=CONVERSATION_IDENTITY_MISMATCH`
      })
      return false
    }
    notifyAgentSkillsChanged(openResult.data.conversation.workspaceCwd)

    // Snapshot the project pane tree before this Conversation becomes active so
    // later project-layout writes cannot persist Conversation chrome.
    try {
      const { persistState } = await import('@/hooks/use-editor-persistence')
      const projectId = (await import('@/stores/project-store')).useProjectStore.getState()
        .activeProjectId
      if (projectId) persistState(projectId)
    } catch {
      // Project layout persist is best-effort; conversation open must continue.
    }
    if (!isCurrent()) {
      logStaleActivation(conversationId, activationEpoch, 'persist-project')
      return false
    }

    let activatedOutcome: ConversationOpenOutcome | null = null
    set((state) => {
      if (!activationIsCurrent(state, conversationId, activationEpoch)) return {}
      const knownRevision = state.lifecycleRevisionById[conversationId] ?? -1
      const deletedRevision = state.deletedRevisionById[conversationId]
      const current = currentConversationRecord(state, conversationId)
      const incoming = openResult.data.conversation
      const conversation =
        deletedRevision !== undefined && incoming.lastSeq <= deletedRevision
          ? undefined
          : incoming.lastSeq < knownRevision && (!current || current.lastSeq < knownRevision)
            ? undefined
            : incoming.lastSeq < knownRevision
              ? current
              : incoming
      if (!conversation) {
        return {
          openingById: { ...state.openingById, [conversationId]: false },
          errorsById: {
            ...state.errorsById,
            [conversationId]: stableError('CONVERSATION_CONFLICT')
          }
        }
      }
      activatedOutcome = {
        ...openResult.data,
        conversation: mergeConversationTitle(current, conversation)
      }
      const alreadyListed = Boolean(state.summariesById[conversationId])
      return {
        summariesById: {
          ...state.summariesById,
          [conversationId]: activatedOutcome.conversation
        },
        conversationIds: alreadyListed
          ? state.conversationIds
          : [conversationId, ...state.conversationIds],
        detailsById: { ...state.detailsById, [conversationId]: activatedOutcome },
        lifecycleRevisionById: {
          ...state.lifecycleRevisionById,
          [conversationId]: Math.max(knownRevision, conversation.lastSeq)
        },
        activeConversationId: conversationId,
        errorsById: { ...state.errorsById, [conversationId]: undefined }
      }
    })
    if (!activatedOutcome) {
      if (!isCurrent()) logStaleActivation(conversationId, activationEpoch, 'open-commit')
      else {
        void logFrontendError({
          level: 'warn',
          source: 'conversation-store.activation',
          message: `conversationId=${conversationId} epoch=${activationEpoch} stage=open-commit code=CONVERSATION_STALE_OPEN`
        })
      }
      return false
    }
    useSessionWorkspaceSyncStore.getState().setActiveConversationId(conversationId)

    const workspaceModule = await import('@/hooks/use-session-workspace-sync')
    if (!isCurrent()) {
      logStaleActivation(conversationId, activationEpoch, 'workspace-import')
      return false
    }
    await workspaceModule.loadSessionWorkspace(conversationId, isCurrent)
    if (!isCurrent()) {
      logStaleActivation(conversationId, activationEpoch, 'workspace')
      return false
    }

    const { useAcpStore } = await import('@/stores/acp-store')
    if (!isCurrent()) {
      logStaleActivation(conversationId, activationEpoch, 'binding-import')
      return false
    }
    const seedHostBoundSession = (bound: {
      sessionId: string
      runtimeAgentId: string
      executionCwd: string
    }): void => {
      useAcpStore.setState((state) => {
        const existing = state.sessions[bound.sessionId]
        const indexEntry = state.sessionIndex.find((entry) => entry.id === bound.sessionId)
        return {
          sessions: {
            ...state.sessions,
            [bound.sessionId]: {
              id: bound.sessionId,
              conversationId,
              agentId: existing?.agentId ?? bound.runtimeAgentId,
              cwd: existing?.cwd || bound.executionCwd,
              projectId: existing?.projectId ?? '',
              status: existing?.status ?? 'closed',
              title: existing?.title ?? null,
              activeTurn: existing?.activeTurn ?? false,
              openTurnId: existing?.openTurnId ?? null,
              modes: existing?.modes ?? null,
              models: existing?.models ?? null,
              configOptions: existing?.configOptions ?? [],
              lastError: existing?.lastError ?? null,
              createdAt: existing?.createdAt ?? Date.now()
            }
          },
          sessionIndex: indexEntry
            ? state.sessionIndex.map((entry) =>
                entry.id === bound.sessionId ? { ...entry, conversationId } : entry
              )
            : [
                ...state.sessionIndex,
                {
                  id: bound.sessionId,
                  conversationId,
                  agentId: bound.runtimeAgentId,
                  title: '',
                  cwd: bound.executionCwd,
                  projectId: '',
                  createdAt: Date.now(),
                  lastActivityAt: Date.now(),
                  messageCount: 0,
                  status: 'closed' as const
                }
              ]
        }
      })
    }
    try {
      await useAcpStore.getState().loadSessionIndex()
    } catch {
      // Index refresh is best-effort; live sessions and a prior index still bind.
    }
    if (!isCurrent()) {
      logStaleActivation(conversationId, activationEpoch, 'binding-index')
      return false
    }
    let acp = useAcpStore.getState()
    let sessionId = bindingSessionId(acp, conversationId)
    if (!sessionId) {
      const hostBound = await fetchHostBoundSession(conversationId)
      if (!isCurrent()) {
        logStaleActivation(conversationId, activationEpoch, 'host-binding')
        return false
      }
      if (hostBound) {
        seedHostBoundSession(hostBound)
        sessionId = hostBound.sessionId
        acp = useAcpStore.getState()
        void logFrontendError({
          level: 'warn',
          source: 'conversation-store.activation',
          message: `conversationId=${conversationId} epoch=${activationEpoch} stage=host-binding`
        })
      }
    }
    if (!sessionId) {
      acp.setActiveSession(null)
      useWorkspaceStore.getState().addAgentChatTab(conversationId, undefined, false)
      set((state) =>
        activationIsCurrent(state, conversationId, activationEpoch)
          ? { openingById: { ...state.openingById, [conversationId]: false } }
          : {}
      )
      return true
    }

    try {
      const live = acp.sessions[sessionId]
      if (!live || live.status === 'closed') {
        // Durable history belongs to Se and is rendered independently from
        // the agent process. Reconnect in the background so a slow ACP
        // session/resume (or load fallback) cannot keep the Conversation route
        // behind its full-screen opening state.
        void acp.openHistorySession(sessionId).catch((error) => {
          void logFrontendError({
            level: 'warn',
            source: 'conversation-store.activation',
            message: `conversationId=${conversationId} epoch=${activationEpoch} stage=binding-reconnect code=ACP_SESSION_REOPEN_FAILED error=${error instanceof Error ? error.message : String(error)}`
          })
        })
      }
      if (!isCurrent()) return false
      let boundSessionId = sessionId
      if (!isCurrent()) return false
      acp = useAcpStore.getState()
      const resolved = resolveConversationSessionId(acp, conversationId)
      if (resolved && isLiveAcpSession(acp.sessions[resolved])) {
        boundSessionId = resolved
      }
      acp.setActiveSession(boundSessionId)
      useAcpStore.setState((state) => {
        const session = state.sessions[boundSessionId]
        return {
          sessions: session
            ? {
                ...state.sessions,
                [boundSessionId]: { ...session, conversationId }
              }
            : state.sessions,
          sessionIndex: state.sessionIndex.map((entry) =>
            entry.id === boundSessionId || entry.id === sessionId
              ? { ...entry, conversationId }
              : entry
          )
        }
      })
      useWorkspaceStore.getState().addAgentChatTab(conversationId, undefined, false)
    } catch {
      if (!isCurrent()) {
        logStaleActivation(conversationId, activationEpoch, 'binding-error')
        return false
      }
      set((state) => ({
        openingById: { ...state.openingById, [conversationId]: false },
        errorsById: {
          ...state.errorsById,
          [conversationId]: stableError('CONVERSATION_BINDING_OPEN_FAILED')
        }
      }))
      void logFrontendError({
        level: 'warn',
        source: 'conversation-store.activation',
        message: `conversationId=${conversationId} epoch=${activationEpoch} stage=binding code=CONVERSATION_BINDING_OPEN_FAILED`
      })
      return false
    }

    set((state) =>
      activationIsCurrent(state, conversationId, activationEpoch)
        ? { openingById: { ...state.openingById, [conversationId]: false } }
        : {}
    )
    return true
  },

  applyLifecycleOutcome: (outcome) => {
    if (!isConversationId(outcome.conversationId)) {
      void logFrontendError({
        level: 'warn',
        source: 'conversation-store.lifecycle',
        message: 'code=CONVERSATION_INVALID_ID applied=false'
      })
      return false
    }

    const conversationId = outcome.conversationId
    const outcomeKey = lifecycleOutcomeKey(outcome)
    let applied = false
    let stale = false
    let duplicate = false
    let invariantViolation = false
    let deleted = false
    let deletedActive = false
    set((state) => {
      const summary = state.summariesById[conversationId]
      const detail = state.detailsById[conversationId]
      const storedRevision = Math.max(
        state.lifecycleRevisionById[conversationId] ?? -1,
        summary?.lastSeq ?? -1,
        detail?.conversation.lastSeq ?? -1
      )
      if (outcome.revision < storedRevision) {
        stale = true
        return {}
      }
      const deletedRevision = state.deletedRevisionById[conversationId]
      if (deletedRevision !== undefined && outcome.action !== 'deleteConversation') {
        if (outcome.revision <= deletedRevision) duplicate = true
        else invariantViolation = true
        return {}
      }
      if (state.lifecycleOutcomeKeyById[conversationId] === outcomeKey) {
        duplicate = true
        return {}
      }
      if (outcome.status === 'blocked') {
        applied = true
        return {
          lifecycleRevisionById: {
            ...state.lifecycleRevisionById,
            [conversationId]: Math.max(storedRevision, outcome.revision)
          },
          lifecycleOutcomeKeyById: {
            ...state.lifecycleOutcomeKeyById,
            [conversationId]: outcomeKey
          }
        }
      }

      const records = [summary, detail?.conversation].filter(
        (record): record is ConversationRecordV2 => Boolean(record)
      )
      if (records.some((record) => record.workspaceCwd !== outcome.workspaceCwd)) {
        invariantViolation = true
        return {}
      }
      if (outcome.action === 'deleteConversation') {
        const alreadyDeleted =
          !summary &&
          !detail &&
          !state.conversationIds.includes(conversationId) &&
          state.lifecycleRevisionById[conversationId] === outcome.revision
        if (alreadyDeleted) {
          duplicate = true
          return {}
        }
        applied = true
        deleted = true
        deletedActive = state.activeConversationId === conversationId
        const cancelsActivation = state.activationRouteValue === conversationId
        return {
          summariesById: withoutConversationKey(state.summariesById, conversationId),
          conversationIds: state.conversationIds.filter((id) => id !== conversationId),
          detailsById: withoutConversationKey(state.detailsById, conversationId),
          activeConversationId: deletedActive ? null : state.activeConversationId,
          lifecycleRevisionById: {
            ...state.lifecycleRevisionById,
            [conversationId]: outcome.revision
          },
          deletedRevisionById: {
            ...state.deletedRevisionById,
            [conversationId]: outcome.revision
          },
          lifecycleOutcomeKeyById: {
            ...state.lifecycleOutcomeKeyById,
            [conversationId]: outcomeKey
          },
          activationEpoch: cancelsActivation ? state.activationEpoch + 1 : state.activationEpoch,
          activationRouteValue: cancelsActivation ? null : state.activationRouteValue,
          openingById: withoutConversationKey(state.openingById, conversationId),
          aggregateBusyById: withoutConversationKey(state.aggregateBusyById, conversationId),
          errorsById: withoutConversationKey(state.errorsById, conversationId)
        }
      }

      applied = true
      const updateRecord = (record: ConversationRecordV2): ConversationRecordV2 => ({
        ...record,
        lifecycleState: outcome.lifecycleState,
        lastSeq: outcome.revision
      })
      return {
        summariesById: summary
          ? { ...state.summariesById, [conversationId]: updateRecord(summary) }
          : state.summariesById,
        detailsById: detail
          ? {
              ...state.detailsById,
              [conversationId]: {
                ...detail,
                conversation: updateRecord(detail.conversation)
              }
            }
          : state.detailsById,
        lifecycleRevisionById: {
          ...state.lifecycleRevisionById,
          [conversationId]: outcome.revision
        },
        lifecycleOutcomeKeyById: {
          ...state.lifecycleOutcomeKeyById,
          [conversationId]: outcomeKey
        },
        errorsById: { ...state.errorsById, [conversationId]: undefined }
      }
    })

    if (invariantViolation) {
      void logFrontendError({
        level: 'error',
        source: 'conversation-store.lifecycle',
        message: `conversationId=${conversationId} revision=${outcome.revision} action=${outcome.action} code=CONVERSATION_IDENTITY_MISMATCH`
      })
      return false
    }
    if (stale) {
      void logFrontendError({
        level: 'warn',
        source: 'conversation-store.lifecycle',
        message: `conversationId=${conversationId} revision=${outcome.revision} action=${outcome.action} applied=false reason=stale`
      })
      return false
    }
    if (duplicate) return false
    if (deleted) {
      if (deletedActive) {
        useSessionWorkspaceSyncStore.getState().setActiveConversationId(null)
      }
      useSessionWorkspaceSyncStore.getState().setRestoreInProgress(conversationId, false)
      useWorkspaceStore.getState().closeChatView(conversationId)
    }
    return applied
  },

  applyAggregateOutcome: (outcome) => {
    const validationError = validateAggregateOutcome(
      outcome,
      currentConversationRecord(get(), outcome.conversationId)
    )
    if (validationError) {
      set((state) => ({
        aggregateBusyById: {
          ...state.aggregateBusyById,
          [outcome.conversationId]: false
        },
        errorsById: {
          ...state.errorsById,
          [outcome.conversationId]: validationError
        }
      }))
      void logFrontendError({
        level: 'error',
        source: 'conversation-store.aggregate',
        message: `conversationId=${outcome.conversationId} code=${validationError.code}`
      })
      return false
    }

    set((state) => {
      const alreadyListed = Boolean(state.summariesById[outcome.conversationId])
      const detail = state.detailsById[outcome.conversationId]
      return {
        summariesById: {
          ...state.summariesById,
          [outcome.conversationId]: outcome.conversation
        },
        conversationIds: alreadyListed
          ? state.conversationIds
          : [outcome.conversationId, ...state.conversationIds],
        detailsById: detail
          ? {
              ...state.detailsById,
              [outcome.conversationId]: {
                ...detail,
                conversation: outcome.conversation
              }
            }
          : state.detailsById,
        lifecycleRevisionById: {
          ...state.lifecycleRevisionById,
          [outcome.conversationId]: outcome.revision
        },
        aggregateBusyById: {
          ...state.aggregateBusyById,
          [outcome.conversationId]: false
        },
        errorsById: {
          ...state.errorsById,
          [outcome.conversationId]: undefined
        }
      }
    })
    return true
  },

  attachProject: async (conversationId, attachment) => {
    const current = currentConversationRecord(get(), conversationId)
    if (!current) {
      set((state) => ({
        errorsById: {
          ...state.errorsById,
          [conversationId]: stableError('CONVERSATION_NOT_FOUND')
        }
      }))
      return null
    }
    set((state) => ({
      aggregateBusyById: { ...state.aggregateBusyById, [conversationId]: true },
      errorsById: { ...state.errorsById, [conversationId]: undefined }
    }))
    try {
      const result = await conversationApi.attachProject(
        conversationId,
        current.lastSeq,
        attachment
      )
      if (!result.success) {
        set((state) => ({
          aggregateBusyById: { ...state.aggregateBusyById, [conversationId]: false },
          errorsById: {
            ...state.errorsById,
            [conversationId]: stableError(result.code, result.error)
          }
        }))
        void logFrontendError({
          level: 'warn',
          source: 'conversation-store.attachProject',
          message: `conversationId=${conversationId} code=${result.code}`
        })
        return null
      }
      return get().applyAggregateOutcome(result.data) ? result.data : null
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      set((state) => ({
        aggregateBusyById: { ...state.aggregateBusyById, [conversationId]: false },
        errorsById: {
          ...state.errorsById,
          [conversationId]: stableError('CONVERSATION_MUTATION_FAILED', message)
        }
      }))
      void logFrontendError({
        level: 'warn',
        source: 'conversation-store.attachProject',
        message: `conversationId=${conversationId} code=CONVERSATION_MUTATION_FAILED`
      })
      return null
    }
  },

  detachProject: async (conversationId) => {
    const current = currentConversationRecord(get(), conversationId)
    if (!current) {
      set((state) => ({
        errorsById: {
          ...state.errorsById,
          [conversationId]: stableError('CONVERSATION_NOT_FOUND')
        }
      }))
      return null
    }
    set((state) => ({
      aggregateBusyById: { ...state.aggregateBusyById, [conversationId]: true },
      errorsById: { ...state.errorsById, [conversationId]: undefined }
    }))
    try {
      const result = await conversationApi.detachProject(conversationId, current.lastSeq)
      if (!result.success) {
        set((state) => ({
          aggregateBusyById: { ...state.aggregateBusyById, [conversationId]: false },
          errorsById: {
            ...state.errorsById,
            [conversationId]: stableError(result.code, result.error)
          }
        }))
        void logFrontendError({
          level: 'warn',
          source: 'conversation-store.detachProject',
          message: `conversationId=${conversationId} code=${result.code}`
        })
        return null
      }
      return get().applyAggregateOutcome(result.data) ? result.data : null
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      set((state) => ({
        aggregateBusyById: { ...state.aggregateBusyById, [conversationId]: false },
        errorsById: {
          ...state.errorsById,
          [conversationId]: stableError('CONVERSATION_MUTATION_FAILED', message)
        }
      }))
      void logFrontendError({
        level: 'warn',
        source: 'conversation-store.detachProject',
        message: `conversationId=${conversationId} code=CONVERSATION_MUTATION_FAILED`
      })
      return null
    }
  },

  updateExecutionTarget: async (conversationId, executionTarget) => {
    const current = currentConversationRecord(get(), conversationId)
    if (!current) {
      set((state) => ({
        errorsById: {
          ...state.errorsById,
          [conversationId]: stableError('CONVERSATION_NOT_FOUND')
        }
      }))
      return null
    }
    set((state) => ({
      aggregateBusyById: { ...state.aggregateBusyById, [conversationId]: true },
      errorsById: { ...state.errorsById, [conversationId]: undefined }
    }))
    try {
      const result = await conversationApi.updateExecutionTarget(
        conversationId,
        current.lastSeq,
        executionTarget
      )
      if (!result.success) {
        set((state) => ({
          aggregateBusyById: { ...state.aggregateBusyById, [conversationId]: false },
          errorsById: {
            ...state.errorsById,
            [conversationId]: stableError(result.code, result.error)
          }
        }))
        void logFrontendError({
          level: 'warn',
          source: 'conversation-store.updateExecutionTarget',
          message: `conversationId=${conversationId} code=${result.code}`
        })
        return null
      }
      return get().applyAggregateOutcome(result.data) ? result.data : null
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      set((state) => ({
        aggregateBusyById: { ...state.aggregateBusyById, [conversationId]: false },
        errorsById: {
          ...state.errorsById,
          [conversationId]: stableError('CONVERSATION_MUTATION_FAILED', message)
        }
      }))
      void logFrontendError({
        level: 'warn',
        source: 'conversation-store.updateExecutionTarget',
        message: `conversationId=${conversationId} code=CONVERSATION_MUTATION_FAILED`
      })
      return null
    }
  },

  clearConversationError: (conversationId) =>
    set((state) => ({
      errorsById: { ...state.errorsById, [conversationId]: undefined }
    })),

  reset: () => {
    let restoringConversationId: ConversationId | null = null
    set((state) => {
      if (state.activationRouteValue && isConversationId(state.activationRouteValue)) {
        restoringConversationId = state.activationRouteValue
      }
      return { ...initialState, activationEpoch: state.activationEpoch + 1 }
    })
    useSessionWorkspaceSyncStore.getState().setActiveConversationId(null)
    if (restoringConversationId) {
      useSessionWorkspaceSyncStore.getState().setRestoreInProgress(restoringConversationId, false)
    }
  }
}))

export function selectVisibleConversations(state: ConversationState): ConversationRecordV2[] {
  const query = state.searchQuery.trim().toLowerCase()
  return state.conversationIds
    .map((conversationId) => state.summariesById[conversationId])
    .filter((summary): summary is ConversationRecordV2 => Boolean(summary))
    .filter((summary) => summary.lifecycleState !== 'deleted')
    .filter((summary) => {
      if (state.projectFilter === 'projectless') return summary.projectAttachment === null
      if (state.projectFilter) {
        return summary.projectAttachment?.projectId === state.projectFilter
      }
      return true
    })
    .filter((summary) => {
      if (!query) return true
      const attachment = summary.projectAttachment
      return [
        summary.conversationId,
        summary.workspaceCwd,
        attachment?.projectId,
        attachment?.projectPathSnapshot,
        attachment?.worktreePath,
        attachment?.worktreeBranch
      ].some((value) => value?.toLowerCase().includes(query))
    })
}

export function useVisibleConversations(): ConversationRecordV2[] {
  return useConversationStore(useShallow(selectVisibleConversations))
}

export function recoveryCountForConversation(
  recoveryItems: readonly RecoveryItemV1[],
  conversationId: ConversationId
): number {
  return recoveryItems.filter(
    (item) => item.status === 'unresolved' && item.conversationIds.includes(conversationId)
  ).length
}

export function applyConversationHostStatus(status: ConversationHostStatus): void {
  useConversationStore.getState().setRecoveryItems(status.recoveryItems)
}
