import { useEffect } from 'react'
import { useAcpStore } from '@/features/agent-session/stores/acp-store'
import { useConversationStore } from '@/features/agent-session/stores/conversation-store'
import { conversationLifecycleApi } from '@/lib/conversation-lifecycle-api'

/** Commit canonical lifecycle state first, then reconcile the derived ACP binding projection. */
export function useConversationLifecycle(): void {
  useEffect(
    () =>
      conversationLifecycleApi.subscribe((outcome) => {
        const applied = useConversationStore.getState().applyLifecycleOutcome(outcome)
        if (applied) useAcpStore.getState()._onConversationLifecycle(outcome)
      }),
    []
  )
}
