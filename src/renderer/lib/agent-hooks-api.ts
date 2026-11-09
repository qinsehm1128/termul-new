import type {
  AdapterId,
  AdapterStatusDto,
  HookConfirmRequest,
  HookMutationResult,
  HookPreviewDto,
  HookPreviewRequest,
  HookStateDto,
  HookStateSyncRequest,
  HookStateSyncResponse,
  ResumePlanDto,
  ResumePlanRequest,
  ResumeSpawnRequest,
  ResumeSpawnResult
} from '@shared/types/agent-hooks.types'
import type { IpcResult } from '@shared/types/ipc.types'
import { invoke } from '@tauri-apps/api/core'
import { logFrontendError } from './log-api'
import { isTauriContext } from './tauri-runtime'
import { webServerAgentHooks } from './web-server-api'

export interface HookAdaptersApi {
  status(): Promise<AdapterStatusDto[]>
  reload(): Promise<AdapterStatusDto[]>
  probe(adapterId: AdapterId): Promise<AdapterStatusDto>
  preview(request: HookPreviewRequest): Promise<HookPreviewDto>
  confirm(request: HookConfirmRequest): Promise<HookMutationResult>
  install(request: HookConfirmRequest): Promise<HookMutationResult>
  uninstall(request: HookConfirmRequest): Promise<HookMutationResult>
  openFolder(): Promise<void>
  state(request?: HookStateSyncRequest): Promise<HookStateSyncResponse>
  resumePlan(request: ResumePlanRequest): Promise<ResumePlanDto>
  resumeSpawn(request: ResumeSpawnRequest): Promise<ResumeSpawnResult>
}

function unwrap<T>(result: IpcResult<T>): T {
  if (!result.success) throw Object.assign(new Error(result.error), { code: result.code })
  return result.data
}

export function createTauriAgentHooksApi(): HookAdaptersApi {
  return {
    status: () => invoke('agent_hooks_status_cmd'),
    reload: () => invoke('agent_hooks_reload_cmd'),
    probe: (adapter_id) => invoke('agent_hooks_probe_cmd', { adapterId: adapter_id }),
    preview: (request) => invoke('agent_hooks_preview_cmd', { request }),
    confirm: (request) => invoke('agent_hooks_confirm_cmd', { request }),
    install: (request) => invoke('agent_hooks_install_cmd', { request }),
    uninstall: (request) => invoke('agent_hooks_uninstall_cmd', { request }),
    openFolder: () => invoke('agent_hooks_open_folder_cmd'),
    state: (request = {}) => invoke('agent_hooks_state_cmd', { request }),
    resumePlan: (request) => invoke('agent_hooks_resume_plan_cmd', { request }),
    resumeSpawn: (request) => invoke('agent_hooks_resume_spawn_cmd', { request })
  }
}

export const webAgentHooksApi: HookAdaptersApi = {
  status: async () => unwrap(await webServerAgentHooks.status()),
  reload: async () => unwrap(await webServerAgentHooks.reload()),
  probe: async (adapterId) => unwrap(await webServerAgentHooks.probe(adapterId)),
  preview: async (request) => unwrap(await webServerAgentHooks.preview(request)),
  confirm: async (request) => unwrap(await webServerAgentHooks.confirm(request)),
  install: async (request) => unwrap(await webServerAgentHooks.install(request)),
  uninstall: async (request) => unwrap(await webServerAgentHooks.uninstall(request)),
  openFolder: async () => {
    throw Object.assign(new Error('Open folder is available on desktop only'), {
      code: 'UNSUPPORTED_PLATFORM'
    })
  },
  state: async (request = {}) => unwrap(await webServerAgentHooks.state(request)),
  resumePlan: async (request) => unwrap(await webServerAgentHooks.resumePlan(request)),
  resumeSpawn: async (request) => unwrap(await webServerAgentHooks.resumeSpawn(request))
}

export type HookStateListener = (response: HookStateSyncResponse) => void
export interface HookStateTransport {
  state(request: HookStateSyncRequest): Promise<HookStateSyncResponse>
  /** Report true only once the authenticated push channel is subscribed. */
  connect(
    onState: HookStateListener,
    onConnection: (connected: boolean) => void
  ): Promise<() => void>
}

const RECONCILE_DELAYS = [1000, 2000, 4000, 8000, 16000, 30000] as const
function warn(message: string) {
  void logFrontendError({ level: 'warn', source: 'agent-hooks-api', message })
}

/** One shared revision cursor, push subscription, in-flight request and fallback timer. */
export function createHookStateSubscription(transport: HookStateTransport) {
  const listeners = new Set<HookStateListener>()
  const fallbackListeners = new Map<() => void, (() => boolean) | undefined>()
  const states = new Map<string, HookStateDto>()
  let cursor: HookStateSyncRequest = {}
  let timer: ReturnType<typeof setTimeout> | undefined
  let connected = false
  let inFlight = false
  let retry = 0
  let epoch = 0
  let disconnect: (() => void) | undefined
  let queuedPush: HookStateSyncResponse[] | null = null
  let resync = false
  let reconcileFailed = false
  const visible = () => typeof document === 'undefined' || !document.hidden
  const active = () => listeners.size > 0

  function clearTimer() {
    if (timer !== undefined) clearTimeout(timer)
    timer = undefined
  }

  function publish(response: HookStateSyncResponse) {
    // The response cursor may skip unauthorized projects. Never infer gaps from rows.
    if (
      response.instance_id === cursor.instance_id &&
      response.current_revision < (cursor.since_revision ?? 0)
    )
      return
    if (response.kind === 'snapshot' || response.instance_id !== cursor.instance_id) states.clear()
    for (const state of response.states) states.set(state.terminal_id, state)
    for (const tombstone of response.tombstones) states.delete(tombstone.terminal_id)
    cursor = { instance_id: response.instance_id, since_revision: response.current_revision }
    for (const listener of listeners) {
      try {
        listener(response)
      } catch {
        warn('[AgentHooks] state subscriber failed')
      }
    }
    refreshFallback()
  }

  const needsLegacyDiscovery = () => [...fallbackListeners.values()].some((needed) => needed?.())
  function runFallbacks() {
    if (!active() || !visible()) return
    for (const [listener, needed] of fallbackListeners) {
      if (needed ? !needed() : connected) continue
      try {
        listener()
      } catch {
        /* discovery cannot disrupt state transport */
      }
    }
  }
  function refreshFallback() {
    if (!visible() || (connected && !needsLegacyDiscovery())) clearTimer()
    else schedule()
  }
  function schedule() {
    if (!active() || !visible() || inFlight || timer !== undefined) return
    if (connected && !needsLegacyDiscovery()) return
    timer = setTimeout(
      () => {
        timer = undefined
        if (connected) {
          runFallbacks()
          schedule()
        } else void reconcile()
      },
      connected ? 15_000 : RECONCILE_DELAYS[Math.min(retry++, RECONCILE_DELAYS.length - 1)]
    )
  }

  async function reconcile() {
    if (!active() || !visible()) return
    if (inFlight) {
      resync = true
      return
    }
    clearTimer()
    inFlight = true
    const currentEpoch = epoch
    queuedPush = []
    try {
      const response = await transport.state(cursor)
      if (currentEpoch === epoch && active()) {
        reconcileFailed = false
        publish(response)
      }
    } catch {
      if (currentEpoch === epoch && active() && !reconcileFailed) {
        reconcileFailed = true
        warn('[AgentHooks] state reconcile failed')
      }
    } finally {
      inFlight = false
      const pending = queuedPush
      queuedPush = null
      if (currentEpoch === epoch && active()) {
        for (const response of pending ?? []) receivePush(response)
        if (!connected) runFallbacks()
      }
      if (resync && active()) {
        resync = false
        void reconcile()
      } else {
        schedule()
      }
    }
  }

  function receivePush(response: HookStateSyncResponse) {
    if (queuedPush) queuedPush.push(response)
    else if (response.kind === 'delta' && response.instance_id !== cursor.instance_id) {
      // A delta cannot establish an instance's complete baseline (especially
      // when subscribing in a hidden tab). Keep the old/empty cursor so the
      // host returns a full snapshot, not just changes after this push.
      void reconcile()
    } else publish(response)
  }

  function onConnection(next: boolean) {
    if (!active() || connected === next) return
    connected = next
    clearTimer()
    if (next) {
      retry = 0
      void reconcile()
    } else schedule()
  }

  function onVisibility() {
    clearTimer()
    if (visible()) {
      retry = 0
      void reconcile()
    }
  }

  function start() {
    const currentEpoch = ++epoch
    connected = false
    retry = 0
    reconcileFailed = false
    document.addEventListener('visibilitychange', onVisibility)
    void transport
      .connect(
        (response) => {
          if (epoch !== currentEpoch || !active()) return
          receivePush(response)
        },
        (next) => {
          if (epoch === currentEpoch) onConnection(next)
        }
      )
      .then((stop) => {
        if (epoch !== currentEpoch || !active()) stop()
        else disconnect = stop
      })
      .catch(() => {
        if (epoch === currentEpoch && active()) {
          warn('[AgentHooks] state push unavailable')
          connected = false
          schedule()
        }
      })
    void reconcile()
  }

  return (
    listener: HookStateListener,
    onFallback?: () => void,
    needsDiscovery?: () => boolean
  ): (() => void) & { refreshFallback?: () => void } => {
    if (onFallback) fallbackListeners.set(onFallback, needsDiscovery)
    listeners.add(listener)
    if (listeners.size === 1) start()
    else if (cursor.instance_id != null)
      listener({
        kind: 'snapshot',
        instance_id: cursor.instance_id,
        current_revision: cursor.since_revision ?? 0,
        states: [...states.values()],
        tombstones: []
      })
    let disposed = false
    const stop = () => {
      if (disposed) return
      disposed = true
      listeners.delete(listener)
      if (onFallback) fallbackListeners.delete(onFallback)
      if (!active()) {
        ++epoch
        clearTimer()
        disconnect?.()
        disconnect = undefined
        document.removeEventListener('visibilitychange', onVisibility)
        connected = false
        cursor = {}
        states.clear()
        resync = false
      } else refreshFallback()
    }
    refreshFallback()
    return Object.assign(stop, {
      refreshFallback: () => {
        if (!disposed) refreshFallback()
      }
    })
  }
}

async function connectStatePush(
  onState: HookStateListener,
  onConnection: (connected: boolean) => void
): Promise<() => void> {
  if (isTauriContext()) {
    const { listen } = await import('@tauri-apps/api/event')
    const unlisten = await listen<HookStateSyncResponse>('agent-hooks-state', ({ payload }) =>
      onState(payload)
    )
    onConnection(true)
    return unlisten
  }
  const [{ getAcpTransport }, { useAcpStore }] = await Promise.all([
    import('./acp-transport'),
    import('@/features/agent-session/stores/acp-store')
  ])
  const transport = getAcpTransport()
  const terminalState = () =>
    'getTerminalState' in transport && typeof transport.getTerminalState === 'function'
      ? transport.getTerminalState()
      : null
  const unlisten = transport.onEvent<HookStateSyncResponse>('agent-hooks-state', onState)
  const unlistenRevocation = transport.onEvent('reauthentication_required', () =>
    onConnection(false)
  )
  // ACP already owns the socket's single reconnect listener. Observe its published state;
  // never replace that listener or open another socket just for hooks.
  const unsubscribe = useAcpStore.subscribe((state, previous) => {
    if (state.transportReconnecting !== previous.transportReconnecting || terminalState())
      onConnection(!state.transportReconnecting && !terminalState())
  })
  try {
    await transport.connect()
    onConnection(!terminalState())
  } catch {
    // The shared transport retries its socket; keep listeners so a later reconnect
    // stops reconciliation instead of leaving this subscription permanently in fallback.
    onConnection(false)
    warn('[AgentHooks] state push disconnected')
  }
  return () => {
    unlisten()
    unlistenRevocation()
    unsubscribe()
  }
}

const desktop = createTauriAgentHooksApi()
function runtime(): HookAdaptersApi {
  return isTauriContext() ? desktop : webAgentHooksApi
}

// The push connector is defined below; consumers never own polling or reconnection timers.
const subscribeState = createHookStateSubscription({
  state: (request) => runtime().state(request),
  connect: connectStatePush
})

export const subscribeAgentHookState = subscribeState

/** Resolve the runtime at call time so both renderer roots share identical behavior. */
export const agentHooksApi: HookAdaptersApi & { subscribeState: typeof subscribeState } = {
  status: () => runtime().status(),
  reload: () => runtime().reload(),
  probe: (adapterId) => runtime().probe(adapterId),
  preview: (request) => runtime().preview(request),
  confirm: (request) => runtime().confirm(request),
  install: (request) => runtime().install(request),
  uninstall: (request) => runtime().uninstall(request),
  openFolder: () => runtime().openFolder(),
  state: (request) => runtime().state(request),
  resumePlan: (request) => runtime().resumePlan(request),
  resumeSpawn: (request) => runtime().resumeSpawn(request),
  subscribeState
}
