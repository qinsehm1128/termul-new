import type { IpcResult } from '@shared/types/ipc.types'
import type {
  CommandReply,
  ExecuteContributionRequest,
  HostContextState,
  HostUiDismissRequest,
  HostUiSnapshot
} from '@shared/types/plugin.types'
import { create } from 'zustand'
import { pluginApi, pluginTransportAvailable, type TauriPluginApi } from './plugin-api'

interface PluginHostUiState {
  snapshot: HostUiSnapshot | null
  contextRevision: number
  context: HostContextState | null
  contextPending: boolean
  setContextPending(pending: boolean): void
  connected: boolean
  load(): Promise<void>
  subscribe(): () => void
  acknowledgeContext(revision: number, context?: HostContextState): void
  execute(request: ExecuteContributionRequest): Promise<IpcResult<CommandReply>>
  dismiss(request: HostUiDismissRequest): Promise<void>
}

/** Native host owns records and TTLs. This cache is ephemeral and boot/revision scoped. */
export function createPluginHostUiStore(
  api: TauriPluginApi = pluginApi,
  isDesktop: () => boolean = pluginTransportAvailable
) {
  let epoch = 0
  let consumers = 0
  let release: (() => void) | undefined
  let unsupported = false
  let ready = false
  let bootId: string | null = null
  const retiredBoots = new Set<string>()
  const selectBoot = (next: string | null) => {
    if (next && retiredBoots.has(next)) return false
    if (bootId && next && next !== bootId) retiredBoots.add(bootId)
    if (next) bootId = next
    return true
  }
  let sawConnection = false
  return create<PluginHostUiState>((set, get) => ({
    snapshot: null,
    contextRevision: 0,
    context: null,
    contextPending: true,
    setContextPending: (contextPending) => set({ contextPending }),
    connected: false,
    acknowledgeContext(revision, context) {
      if (revision >= get().contextRevision)
        set({ contextRevision: revision, ...(context ? { context, contextPending: false } : {}) })
    },
    async load() {
      if (!isDesktop() || unsupported || !ready || !get().connected) return
      const requestEpoch = epoch
      try {
        const result = await api.getHostUi()
        if (requestEpoch !== epoch || !get().connected) return
        if (!result.success) {
          if (result.code === 'PLUGIN_NOT_SUPPORTED') {
            unsupported = true
            set({ snapshot: null })
          }
          return
        }
        if (unsupported) return
        if (bootId && result.data.bootId !== bootId) return
        const previous = get().snapshot
        if (previous?.bootId === result.data.bootId && previous.revision >= result.data.revision)
          return
        bootId = result.data.bootId
        set({ snapshot: result.data })
      } catch {
        /* Keep the last valid cache until disconnect or next event. */
      }
    },
    subscribe() {
      if (!isDesktop()) return () => {}
      ++consumers
      if (consumers === 1) {
        let active = true
        const changed = api.onChanged((event) => {
          if (!active || retiredBoots.has(event.bootId)) return
          const changedBoot = !!bootId && bootId !== event.bootId
          if (!selectBoot(event.bootId)) return
          if (changedBoot) {
            unsupported = false
            ++epoch
            set({ snapshot: null, contextRevision: 0, context: null, contextPending: true })
          }
          if (ready && (event.type === 'host_ui_changed' || !get().snapshot)) void get().load()
        })
        const connection = api.onConnectionChanged((event) => {
          if (!active || (event.bootId && retiredBoots.has(event.bootId))) return
          sawConnection = true
          const sameConnection =
            event.state === 'connected' && get().connected && event.bootId === bootId
          if (!selectBoot(event.bootId)) return
          if (sameConnection) {
            if (ready) void get().load()
            return
          }
          ++epoch
          unsupported = false
          set({
            snapshot: null,
            contextRevision: 0,
            context: null,
            contextPending: true,
            connected: event.state === 'connected'
          })
          if (ready && event.state === 'connected') void get().load()
        })
        release = () => {
          active = false
          ready = false
          changed()
          connection()
        }
        // Do not race a fetch against asynchronous Tauri listener registration.
        void Promise.all([changed.ready, connection.ready])
          .then(() => {
            if (!active) return
            ready = true
            // Initial connection events may have arrived during setup.
            if (!sawConnection) set({ connected: true })
            void get().load()
          })
          .catch(() => {
            if (!active) return
            release?.()
            set({
              snapshot: null,
              connected: false,
              contextRevision: 0,
              context: null,
              contextPending: true
            })
          })
      }
      let disposed = false
      return () => {
        if (disposed) return
        disposed = true
        if (--consumers !== 0) return
        ++epoch
        release?.()
        release = undefined
        bootId = null
        unsupported = false
        sawConnection = false
        set({
          snapshot: null,
          connected: false,
          contextRevision: 0,
          context: null,
          contextPending: true
        })
      }
    },
    execute: (request) => api.executeContribution(request),
    async dismiss(request) {
      const result = await api.dismissHostUi(request)
      if (result.success) await get().load()
    }
  }))
}
export const usePluginHostUiStore = createPluginHostUiStore()
