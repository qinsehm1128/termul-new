import type { IpcDataDecoder, IpcResult } from '@shared/types/ipc.types'
import {
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
  parseHostContextAck,
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
import { type HostEventSubscription, onHostEvent } from '@/lib/host-events'
import { invokeDecodedIpcResult } from '@/lib/invoke-decoded-ipc-result'
import { logFrontendError } from '@/lib/log-api'
import { isTauriContext } from '@/lib/tauri-runtime'
import { type PluginRuntimeApi, parseProcessList, parseWebSession } from './plugin-contract'
import { createWebPluginApi } from './web/web-plugin-api'

export type {
  PluginProcessList,
  PluginProcessOwner,
  PluginRuntimeApi,
  PluginWebSession,
  TauriPluginApi
} from './plugin-contract'

export interface PluginApiDependencies {
  isDesktop?: () => boolean
  invokeDecoded?: <T>(
    command: string,
    decoder: IpcDataDecoder<T>,
    args?: { payload: unknown }
  ) => Promise<IpcResult<T>>
  onEvent?: (
    event: string,
    handler: (payload: unknown) => void
  ) => (() => void) & { ready?: Promise<void> }
}
const notSupported = (): IpcResult<never> => ({
  success: false,
  code: 'PLUGIN_NOT_SUPPORTED',
  error: 'Native plugins require the desktop application'
})

/**
 * True when a plugin host is reachable from this runtime: the desktop webview,
 * or a browser served by `se-server` (same-origin HTTP proxy to the core). A
 * standalone server still returns this true and surfaces
 * `PLUGIN_CORE_UNAVAILABLE` as a disconnected state on first request.
 */
export function pluginTransportAvailable(): boolean {
  if (isTauriContext()) return true
  return typeof window !== 'undefined' && !!window.location?.origin
}

/**
 * Desktop transport. Off-desktop this stays inert (no invoke, HTTP or event
 * transport) — the singleton below selects the browser transport instead.
 */
export function createTauriPluginApi(deps: PluginApiDependencies = {}): PluginRuntimeApi {
  const desktop = deps.isDesktop ?? isTauriContext
  const invoke = deps.invokeDecoded ?? invokeDecodedIpcResult
  const on = deps.onEvent ?? onHostEvent
  const call = async <T>(
    command: string,
    decoder: IpcDataDecoder<T>,
    payload?: unknown
  ): Promise<IpcResult<T>> => {
    if (!desktop()) return notSupported()
    const result = await invoke(command, decoder, payload === undefined ? undefined : { payload })
    if (!result.success && result.code !== 'PLUGIN_NOT_SUPPORTED')
      void logFrontendError({
        level: 'warn',
        source: 'plugin-api',
        message: `${command}: request failed code=${result.code ?? 'unknown'}`
      })
    return result
  }
  const listen = <T>(
    name: string,
    decoder: IpcDataDecoder<T>,
    handler: (event: T) => void
  ): HostEventSubscription => {
    if (!desktop()) return Object.assign(() => undefined, { ready: Promise.resolve() })
    const stop = on(name, (payload) => {
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
    return Object.assign(() => stop(), { ready: stop.ready ?? Promise.resolve() })
  }
  return {
    openWebSession: (request) => call('plugin_web_open', parseWebSession, request),
    requestWeb: (request) => call('plugin_web_request', parseWebRequestReply, request),
    readWebSession: (request) => call('plugin_web_read', parseWebReadReply, request),
    closeWebSession: (request) => call('plugin_web_close', parseWebCloseReply, request),
    listProcesses: (request) => call('plugin_process_list', parseProcessList, request),
    getProcessLogs: (request) => call('plugin_process_logs', parseHostProcessLogsReply, request),
    controlProcess: (request) => call('plugin_process_control', parseHostProcessStatus, request),
    list: () => call('plugin_list', parsePluginSnapshot),
    install: (request) => call('plugin_install', parsePluginInfo, request),
    setEnabled: (request) => call('plugin_set_enabled', parsePluginInfo, request),
    reload: (request) => call('plugin_reload', parsePluginInfo, request),
    uninstall: (request) => call('plugin_uninstall', parseUninstallResult, request),
    getView: (request) => call('plugin_get_view', parseViewSnapshot, request),
    dispatchAction: (request) => call('plugin_dispatch_action', parseViewSnapshot, request),
    executeCommand: (request) => call('plugin_execute_command', parseCommandReply, request),
    getConfig: (request) => call('plugin_get_config', parseConfigSnapshot, request),
    setConfig: (request) => call('plugin_set_config', parseConfigSnapshot, request),
    publishHostContext: (request) =>
      call('plugin_publish_host_context', parseHostContextAck, request),
    heartbeatHostContext: (request) =>
      call('plugin_heartbeat_host_context', parseHostContextAck, request),
    getHostUi: () => call('plugin_get_host_ui', parseHostUiSnapshot, {}),
    dismissHostUi: (request) => call('plugin_dismiss_host_ui', parseHostUiDismissResult, request),
    executeContribution: (request) =>
      call('plugin_execute_contribution', parseCommandReply, request),
    onChanged: (handler) => listen('plugins:changed', parsePluginEvent, handler),
    onConnectionChanged: (handler) =>
      listen('plugins:connection_changed', parsePluginConnectionChanged, handler)
  }
}

/**
 * Exact transport singleton imported by production hooks: Tauri invoke on the
 * desktop, same-origin HTTP on the browser/phone client.
 */
export const pluginApi: PluginRuntimeApi = isTauriContext()
  ? createTauriPluginApi()
  : createWebPluginApi()
