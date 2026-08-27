import { useEffect } from 'react'
import { conversationLifecycleApi } from '@/lib/conversation-lifecycle-api'
import { useAcpStore } from '@/stores/acp-store'
import { useConversationStore } from '@/stores/conversation-store'

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
