import type { ConversationHostStatus } from '@shared/types/conversation-api.types'
import { useEffect } from 'react'
import { create } from 'zustand'
import { conversationApi } from '@/lib/conversation-api'
import { logFrontendError } from '@/lib/log-api'
import { applyConversationHostStatus, useConversationStore } from '@/stores/conversation-store'

interface ConversationHostBootstrapState {
  status: ConversationHostStatus | null
  loading: boolean
  setReady(status: ConversationHostStatus): void
  setError(code: string): void
  reset(): void
}

export const useConversationHostBootstrapStore = create<ConversationHostBootstrapState>((set) => ({
  status: null,
  loading: true,
  setReady: (status) => set({ status, loading: false }),
  setError: (code) =>
    set({
      loading: false,
      status: {
        hostKind: 'desktop',
        state: 'error',
        code,
        migrationPhase: 'detected',
        readerPrecedence: 'conversationV2Only',
        recoveryItemCount: 0,
        recoveryItems: []
      }
    }),
  reset: () => set({ status: null, loading: true })
}))

async function refreshHostState(active: () => boolean): Promise<void> {
  const [status, conversations] = await Promise.all([
    conversationApi.getHostStatus(),
    conversationApi.listConversations()
  ])
  if (!active()) return
  if (!status.success) {
    useConversationHostBootstrapStore.getState().setError(status.code)
    void logFrontendError({
      level: 'warn',
      source: 'conversation-host-bootstrap',
      message: status.code
    })
    return
  }
  if (!conversations.success) {
    useConversationHostBootstrapStore.getState().setError(conversations.code)
    void logFrontendError({
      level: 'warn',
      source: 'conversation-host-bootstrap',
      message: conversations.code
    })
    return
  }
  useConversationStore.getState().replaceSummaries(conversations.data)
  applyConversationHostStatus(status.data)
  useConversationHostBootstrapStore.getState().setReady(status.data)
}

/** Load the shared host status, Conversation list, recovery queue, and reconnect updates. */
export function useConversationHostBootstrap(): void {
  useEffect(() => {
    let active = true
    const isActive = () => active
    void refreshHostState(isActive)
    const unsubscribe = conversationApi.subscribeHostStatus(() => {
      if (active) void refreshHostState(isActive)
    })
    return () => {
      active = false
      unsubscribe()
    }
  }, [])
}
