import type { IpcResult } from '@shared/types/ipc.types'
import type { PluginError, PluginEvent, PluginInfo } from '@shared/types/plugin.types'
import { create } from 'zustand'
import { pluginApi } from '@/features/plugins/plugin-api'
import { logFrontendError } from '@/lib/log-api'
import { isTauriContext } from '@/lib/tauri-runtime'

interface PluginsState {
  plugins: PluginInfo[]
  connected: boolean
  loading: boolean
  busy: string | null
  error: PluginError | null
  bootId: string | null
  seq: number
  load: () => Promise<void>
  subscribe: () => () => void
  install: (path: string) => Promise<void>
  setEnabled: (id: string, enabled: boolean) => Promise<void>
  reload: (id: string, path?: string) => Promise<void>
  uninstall: (id: string) => Promise<void>
}

// No configuration persistence: the core alone owns canonical registry/configuration.
let epoch = 0
let consumers = 0
let release: (() => void) | null = null
let disconnected = false

function reportFailure(operation: string): void {
  void logFrontendError({
    level: 'warn',
    source: 'plugins-store',
    message: `Plugin ${operation} failed`
  })
}

export const usePluginsStore = create<PluginsState>((set, get) => {
  async function mutate<T>(
    operation: string,
    request: () => Promise<IpcResult<T>>,
    apply: (value: T) => void
  ): Promise<void> {
    if (!isTauriContext() || get().busy || !get().connected) return
    set({ busy: operation, error: null })
    // An older list must not overwrite this operation's authoritative result.
    ++epoch
    try {
      const result = await request()
      if (!result.success) {
        set({ error: { code: result.code, message: 'Plugin operation failed' } })
        if (result.code === 'PLUGIN_CORE_UNAVAILABLE') set({ connected: false })
        reportFailure(operation)
        return
      }
      ++epoch
      apply(result.data)
      await get().load()
    } catch {
      set({ error: { code: 'PLUGIN_IO', message: 'Plugin operation failed' } })
      reportFailure(operation)
    } finally {
      set({ busy: null, loading: false })
    }
  }

  function upsert(info: PluginInfo): void {
    set((state) => ({
      plugins: state.plugins.some((plugin) => plugin.id === info.id)
        ? state.plugins.map((plugin) => (plugin.id === info.id ? info : plugin))
        : [...state.plugins, info]
    }))
  }

  function changed(event: PluginEvent): void {
    const current = get()
    const changedBoot = current.bootId !== event.bootId
    const gap = event.seq !== current.seq + 1
    if (!changedBoot && event.seq <= current.seq) return
    set({ bootId: event.bootId, seq: event.seq })
    if (changedBoot || gap || event.type !== 'view_changed') {
      // Until list reconciles lifecycle notifications, old commands are not executable.
      if (changedBoot || gap || event.type !== 'config_changed') set({ connected: false })
      void get().load()
    }
  }

  return {
    plugins: [],
    connected: false,
    loading: false,
    busy: null,
    error: null,
    bootId: null,
    seq: 0,
    load: async () => {
      if (!isTauriContext()) return
      const request = ++epoch
      set({ loading: true })
      try {
        const result = await pluginApi.list()
        if (request !== epoch) return
        if (!result.success) {
          set({
            error: { code: result.code, message: 'Plugin list failed' },
            connected: false
          })
          reportFailure('list')
          return
        }
        // The host may have emitted another event while producing this snapshot.
        if (result.data.bootId === get().bootId && result.data.seq < get().seq) {
          void get().load()
          return
        }
        set({
          plugins: result.data.plugins,
          bootId: result.data.bootId,
          seq: result.data.seq,
          connected: !disconnected,
          error: null
        })
      } catch {
        if (request !== epoch) return
        set({ connected: false, error: { code: 'PLUGIN_IO', message: 'Plugin list failed' } })
        reportFailure('list')
      } finally {
        if (request === epoch) set({ loading: false })
      }
    },
    subscribe: () => {
      if (!isTauriContext()) return () => {}
      consumers += 1
      if (consumers === 1) {
        disconnected = false
        const offChanged = pluginApi.onChanged(changed)
        const offConnection = pluginApi.onConnectionChanged((event) => {
          disconnected = event.state === 'disconnected'
          ++epoch
          set({ connected: false, bootId: event.bootId, seq: 0 })
          void get().load()
        })
        release = () => {
          offChanged()
          offConnection()
        }
        // Register both listeners before requesting the initial snapshot.
        void get().load()
      }
      let disposed = false
      return () => {
        if (disposed) return
        disposed = true
        consumers -= 1
        if (consumers === 0) {
          release?.()
          release = null
          ++epoch
          set({ connected: false, loading: false })
        }
      }
    },
    install: (path) => mutate('install', () => pluginApi.install({ path }), upsert),
    setEnabled: (id, enabled) =>
      mutate('setEnabled', () => pluginApi.setEnabled({ id, enabled }), upsert),
    reload: (id, path) =>
      mutate('reload', () => pluginApi.reload(path === undefined ? { id } : { id, path }), upsert),
    uninstall: (id) =>
      mutate(
        'uninstall',
        () => pluginApi.uninstall({ id }),
        () => set((state) => ({ plugins: state.plugins.filter((plugin) => plugin.id !== id) }))
      )
  }
})
