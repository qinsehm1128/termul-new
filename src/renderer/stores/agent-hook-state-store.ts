import type {
  AgentHookKind,
  HookStateDto,
  HookStateSyncResponse
} from '@shared/types/agent-hooks.types'
import { create } from 'zustand'

/** Runtime-only decoration. None of these fields may enter TerminalAgentSession. */
export type AgentHookRuntimeState = HookStateDto & {
  hook_kind?: AgentHookKind
  hook_at?: number
  tool_name?: string
}

export const agentHookStateKey = (instanceId: string, terminalId: string): string =>
  `${instanceId}\0${terminalId}`

export interface AgentHookStateStore {
  instanceId: string | null
  currentRevision: number
  states: Record<string, AgentHookRuntimeState>
  /** Revisions survive removal so delayed events cannot resurrect a tombstone. */
  revisions: Record<string, number>
  retiredInstances: ReadonlySet<string>
  applySync: (response: HookStateSyncResponse) => void
  reset: () => void
}

const emptyState = () => ({
  instanceId: null,
  currentRevision: 0,
  states: {} as Record<string, AgentHookRuntimeState>,
  revisions: {} as Record<string, number>,
  retiredInstances: new Set<string>()
})

/** One subscription-fed runtime store, intentionally without persistence or polling. */
export const useAgentHookStateStore = create<AgentHookStateStore>((set) => ({
  ...emptyState(),
  reset: () => set(emptyState()),
  applySync: (response) =>
    set((previous) => {
      if (
        !response.instance_id ||
        !Number.isSafeInteger(response.current_revision) ||
        response.current_revision < 0
      )
        return previous
      const changedInstance = previous.instanceId !== response.instance_id
      if (previous.retiredInstances.has(response.instance_id)) return previous
      // A delta cannot establish a new host epoch: require its authoritative snapshot.
      if (changedInstance && response.kind !== 'snapshot') return previous
      if (!changedInstance && response.current_revision <= previous.currentRevision) return previous

      const retiredInstances = new Set(previous.retiredInstances)
      if (changedInstance && previous.instanceId) retiredInstances.add(previous.instanceId)
      const states = changedInstance ? {} : { ...previous.states }
      const revisions = changedInstance ? {} : { ...previous.revisions }
      const applyState = (state: HookStateDto): void => {
        if (
          state.instance_id !== response.instance_id ||
          state.revision > response.current_revision ||
          state.revision < 0
        )
          return
        const key = agentHookStateKey(state.instance_id, state.terminal_id)
        if (state.revision <= (revisions[key] ?? -1)) return
        states[key] = state
        revisions[key] = state.revision
      }
      if (response.kind === 'snapshot') {
        const present = new Set(
          response.states
            .filter(
              (state) =>
                state.instance_id === response.instance_id &&
                state.revision <= response.current_revision
            )
            .map((state) => agentHookStateKey(state.instance_id, state.terminal_id))
        )
        for (const key of Object.keys(states)) {
          if (!present.has(key)) {
            delete states[key]
            revisions[key] = response.current_revision
          }
        }
        for (const state of response.states) applyState(state)
      } else {
        // Merge rows and tombstones by revision; global revision gaps are legal.
        const changes = [
          ...response.states.map((state) => ({ revision: state.revision, state })),
          ...response.tombstones.map((tombstone) => ({ revision: tombstone.revision, tombstone }))
        ].sort((a, b) => a.revision - b.revision)
        for (const change of changes) {
          if (change.revision > response.current_revision || change.revision < 0) continue
          if ('state' in change) applyState(change.state)
          else {
            if (change.tombstone.instance_id !== response.instance_id) continue
            const key = agentHookStateKey(response.instance_id, change.tombstone.terminal_id)
            if (change.revision <= (revisions[key] ?? -1)) continue
            delete states[key]
            revisions[key] = change.revision
          }
        }
      }
      return {
        instanceId: response.instance_id,
        currentRevision: response.current_revision,
        states,
        revisions,
        retiredInstances
      }
    })
}))

/** Sessions still reporting evidence; `ended` stays visible in views but is not active. */
export function countActiveHookSessions(states: Record<string, AgentHookRuntimeState>): number {
  return Object.values(states).filter((state) => state.state !== 'ended').length
}

export function getAgentHookRuntimeState(terminalId: string): AgentHookRuntimeState | undefined {
  const store = useAgentHookStateStore.getState()
  return store.instanceId
    ? store.states[agentHookStateKey(store.instanceId, terminalId)]
    : undefined
}
