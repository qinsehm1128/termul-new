import type { ConversationId } from '@shared/types/conversation.types'
import type { RecoveryItemV1 } from '@shared/types/conversation-recovery.types'
import type { SessionWorkspaceLoadOutcome } from '@shared/types/session-workspace.types'
import { create } from 'zustand'

export interface SessionWorkspaceConflictBody {
  conversationId: ConversationId
  currentRevision: number
  currentUpdatedAtUtc: string
  currentUpdateIdentity?: string | null
}

interface SessionWorkspaceSyncState {
  activeConversationId: ConversationId | null
  basedRevisionByConversation: Record<ConversationId, number | null>
  conflictsByConversation: Record<ConversationId, SessionWorkspaceConflictBody | undefined>
  recoveryByConversation: Record<ConversationId, readonly RecoveryItemV1[] | undefined>
  loadOutcomeByConversation: Record<ConversationId, SessionWorkspaceLoadOutcome | undefined>
  restoreInProgressByConversation: Record<ConversationId, boolean | undefined>
  setActiveConversationId: (conversationId: ConversationId | null) => void
  setBasedRevision: (conversationId: ConversationId, revision: number | null) => void
  setConflict: (
    conversationId: ConversationId,
    conflict: SessionWorkspaceConflictBody | null
  ) => void
  setRecoveryItems: (
    conversationId: ConversationId,
    recoveryItems: readonly RecoveryItemV1[]
  ) => void
  setLoadOutcome: (conversationId: ConversationId, outcome: SessionWorkspaceLoadOutcome) => void
  setRestoreInProgress: (conversationId: ConversationId, inProgress: boolean) => void
  getBasedRevision: (conversationId: ConversationId) => number | null
  getConflict: (conversationId: ConversationId) => SessionWorkspaceConflictBody | null
  getRecoveryItems: (conversationId: ConversationId) => readonly RecoveryItemV1[]
  isRestoreInProgress: (conversationId: ConversationId) => boolean
}

export const useSessionWorkspaceSyncStore = create<SessionWorkspaceSyncState>((set, get) => ({
  activeConversationId: null,
  basedRevisionByConversation: {},
  conflictsByConversation: {},
  recoveryByConversation: {},
  loadOutcomeByConversation: {},
  restoreInProgressByConversation: {},

  setActiveConversationId: (activeConversationId) => set({ activeConversationId }),

  setBasedRevision: (conversationId, revision) =>
    set((state) => ({
      basedRevisionByConversation: {
        ...state.basedRevisionByConversation,
        [conversationId]: revision
      }
    })),

  setConflict: (conversationId, conflict) =>
    set((state) => ({
      conflictsByConversation: {
        ...state.conflictsByConversation,
        [conversationId]: conflict ?? undefined
      }
    })),

  setRecoveryItems: (conversationId, recoveryItems) =>
    set((state) => ({
      recoveryByConversation: {
        ...state.recoveryByConversation,
        [conversationId]: recoveryItems
      }
    })),

  setLoadOutcome: (conversationId, outcome) =>
    set((state) => ({
      loadOutcomeByConversation: {
        ...state.loadOutcomeByConversation,
        [conversationId]: outcome
      }
    })),

  setRestoreInProgress: (conversationId, inProgress) =>
    set((state) => ({
      restoreInProgressByConversation: {
        ...state.restoreInProgressByConversation,
        [conversationId]: inProgress
      }
    })),

  getBasedRevision: (conversationId) => get().basedRevisionByConversation[conversationId] ?? null,
  getConflict: (conversationId) => get().conflictsByConversation[conversationId] ?? null,
  getRecoveryItems: (conversationId) => get().recoveryByConversation[conversationId] ?? [],
  isRestoreInProgress: (conversationId) =>
    get().restoreInProgressByConversation[conversationId] === true
}))
