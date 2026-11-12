import type { IpcDataDecoder, IpcResult } from '@shared/types/ipc.types'
import {
  type PluginConnectionChanged,
  type PluginEvent,
  parseCommandReply,
  parseConfigSnapshot,
  parsePluginConnectionChanged,
  parsePluginEvent,
  parsePluginInfo,
  parsePluginSnapshot,
  parseUninstallResult,
  parseViewSnapshot
} from '@shared/types/plugin.types'
import {
  type HostProcessLogsReply,
  type HostProcessStatus,
  parseHostProcessLogsReply,
  parseHostProcessStatus,
  parseHostUiDismissResult,
  parseHostUiSnapshot
} from '@shared/types/plugin-host.types'
import {
  parseWebCloseReply,
  parseWebReadReply,
  parseWebRequestReply
} from '@shared/types/plugin-web.types'
import type { HostEventSubscription } from '@/lib/host-events'
import { decodeHttpIpcResult, HTTP_IPC_NETWORK_ERROR_MESSAGE } from '@/lib/http-ipc-result'
import { logFrontendError } from '@/lib/log-api'
import { remoteAccessHeaders } from '@/lib/remote-access-credential'
import {
  type PluginProcessList,
  type PluginProcessOwner,
  type PluginRuntimeApi,
  parseProcessList,
  parseWebSession
} from '../plugin-contract'

/**
 * Browser transport for the desktop plugin host.
 *
 * The web build is served by the same `se-server` origin that proxies the
 * desktop Plugin Core, so every command becomes one same-origin HTTP request
 * with the exact payload object the Tauri command receives. Replies are the
 * shared `IpcResult` envelope; `requestHttpIpcResult` maps transport/parse
 * failures to `NETWORK_ERROR` and preserves host failure codes (for example
 * `FORBIDDEN` for a paired phone calling a management route, or
 * `PLUGIN_CORE_UNAVAILABLE` when no plugin core is attached).
 */
export interface WebPluginApiDependencies {
  base?: () => string
  /** Injectable for tests; defaults to the global fetch. */
  fetch?: typeof fetch
  onEvent?: (
    name: string,
    handler: (payload: unknown) => void
  ) => (() => void) & { ready?: Promise<void> }
}

function serverBase(): string {
  if (typeof window === 'undefined' || !window.location) return ''
  return window.location.origin
}

const notSupported = (): IpcResult<never> => ({
  success: false,
  code: 'PLUGIN_NOT_SUPPORTED',
  error: 'Native plugins require the desktop application'
})

export function createWebPluginApi(deps: WebPluginApiDependencies = {}): PluginRuntimeApi {
  const base = deps.base ?? serverBase
  const fetchImpl =
    deps.fetch ?? ((input: RequestInfo | URL, init?: RequestInit) => fetch(input, init))

  async function call<T>(
    operation: string,
    path: string,
    init: RequestInit,
    decoder: IpcDataDecoder<T>
  ): Promise<IpcResult<T>> {
    let result: IpcResult<T>
    try {
      result = await decodeHttpIpcResult(await fetchImpl(`${base()}${path}`, init), decoder)
    } catch {
      result = { success: false, error: HTTP_IPC_NETWORK_ERROR_MESSAGE, code: 'NETWORK_ERROR' }
    }
    if (!result.success && result.code !== 'PLUGIN_NOT_SUPPORTED') {
      void logFrontendError({
        level: 'warn',
        source: 'plugin-api',
        message: `${operation}: request failed code=${result.code ?? 'unknown'}`
      })
    }
    return result
  }

  function get<T>(
    operation: string,
    path: string,
    decoder: IpcDataDecoder<T>
  ): Promise<IpcResult<T>> {
    return call(operation, path, { method: 'GET', headers: remoteAccessHeaders() }, decoder)
  }

  function post<T>(
    operation: string,
    path: string,
    body: unknown,
    decoder: IpcDataDecoder<T>
  ): Promise<IpcResult<T>> {
    return call(
      operation,
      path,
      {
        method: 'POST',
        headers: remoteAccessHeaders({ 'content-type': 'application/json' }),
        body: JSON.stringify(body ?? {})
      },
      decoder
    )
  }

  function listen<T>(
    name: string,
    decoder: IpcDataDecoder<T>,
    handler: (event: T) => void
  ): HostEventSubscription {
    let disposed = false
    let stop: (() => void) | undefined
    const ready = (async () => {
      try {
        if (deps.onEvent) {
          const subscription = deps.onEvent(name, (payload) => {
            let event: T
            try {
              event = decoder(payload)
            } catch {
              void logFrontendError({
                level: 'warn',
                source: 'plugin-api',
                message: `${name}: invalid event ignored`
              })
              return
            }
            handler(event)
          })
          if (disposed) subscription()
          else stop = subscription
          await subscription.ready
          return
        }
        // The shared ACP WebSocket already carries host events; never open another socket.
        const { getAcpTransport } = await import('@/lib/acp-transport')
        const transport = getAcpTransport()
        if (disposed) return
        stop = transport.onEvent<unknown>(name, (payload) => {
          let event: T
          try {
            event = decoder(payload)
          } catch {
            void logFrontendError({
              level: 'warn',
              source: 'plugin-api',
              message: `${name}: invalid event ignored`
            })
            return
          }
          handler(event)
        })
      } catch {
        void logFrontendError({
          level: 'warn',
          source: 'plugin-api',
          message: `${name}: event transport unavailable`
        })
      }
    })()
    return Object.assign(
      () => {
        if (disposed) return
        disposed = true
        stop?.()
        stop = undefined
      },
      { ready }
    )
  }

  return {
    openWebSession: (request) =>
      post('openWebSession', '/plugins/web/open', request, (value) =>
        parseWebSession(value, 'web')
      ),
    requestWeb: (request) =>
      post('requestWeb', '/plugins/web/request', request, parseWebRequestReply),
    readWebSession: (request) =>
      post('readWebSession', '/plugins/web/read', request, parseWebReadReply),
    closeWebSession: (request) =>
      post('closeWebSession', '/plugins/web/close', request, parseWebCloseReply),
    listProcesses: (request: PluginProcessOwner) =>
      post<PluginProcessList>('listProcesses', '/plugins/processes', request, parseProcessList),
    getProcessLogs: (request) =>
      post<HostProcessLogsReply>(
        'getProcessLogs',
        '/plugins/processes/logs',
        request,
        parseHostProcessLogsReply
      ),
    controlProcess: (request) =>
      post<HostProcessStatus>(
        'controlProcess',
        '/plugins/processes/control',
        request,
        parseHostProcessStatus
      ),
    list: () => get('list', '/plugins', parsePluginSnapshot),
    install: (request) => post('install', '/plugins/install', request, parsePluginInfo),
    setEnabled: (request) => post('setEnabled', '/plugins/enabled', request, parsePluginInfo),
    reload: (request) => post('reload', '/plugins/reload', request, parsePluginInfo),
    uninstall: (request) => post('uninstall', '/plugins/uninstall', request, parseUninstallResult),
    getView: (request) => post('getView', '/plugins/view', request, parseViewSnapshot),
    dispatchAction: (request) =>
      post('dispatchAction', '/plugins/action', request, parseViewSnapshot),
    executeCommand: (request) =>
      post('executeCommand', '/plugins/command', request, parseCommandReply),
    getConfig: (request) => post('getConfig', '/plugins/config/get', request, parseConfigSnapshot),
    setConfig: (request) => post('setConfig', '/plugins/config/set', request, parseConfigSnapshot),
    // Host context is desktop-only; it never touches the web transport.
    publishHostContext: () => Promise.resolve(notSupported()),
    heartbeatHostContext: () => Promise.resolve(notSupported()),
    getHostUi: () => get('getHostUi', '/plugins/host-ui', parseHostUiSnapshot),
    dismissHostUi: (request) =>
      post('dismissHostUi', '/plugins/host-ui/dismiss', request, parseHostUiDismissResult),
    executeContribution: (request) =>
      post('executeContribution', '/plugins/contribution', request, parseCommandReply),
    onChanged: (handler: (event: PluginEvent) => void) =>
      listen('plugins:changed', parsePluginEvent, handler),
    onConnectionChanged: (handler: (event: PluginConnectionChanged) => void) =>
      listen('plugins:connection_changed', parsePluginConnectionChanged, handler)
  }
}
