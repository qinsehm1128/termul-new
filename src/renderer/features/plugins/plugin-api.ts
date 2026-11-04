import type { IpcDataDecoder, IpcResult } from '@shared/types/ipc.types'
import {
  type PluginApi,
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
  parseHostContextAck,
  parseHostProcessLogsReply,
  parseHostProcessStatus,
  parseHostUiDismissResult,
  parseHostUiSnapshot
} from '@shared/types/plugin-host.types'
import {
  parseWebCloseReply,
  parseWebOpenReply,
  parseWebReadReply,
  parseWebRequestReply,
  type WebCloseReply,
  type WebOpenReply,
  type WebOpenRequest,
  type WebReadReply,
  type WebRequest,
  type WebRequestReply
} from '@shared/types/plugin-web.types'
import { type HostEventSubscription, onHostEvent } from '@/lib/host-events'
import { invokeDecodedIpcResult } from '@/lib/invoke-decoded-ipc-result'
import { logFrontendError } from '@/lib/log-api'
import { isTauriContext } from '@/lib/tauri-runtime'

// URL is a GUI transport capability; the Plugin Core WebOpenReply deliberately has no URL.
export type PluginWebSession = WebOpenReply & { url: string }
export interface PluginProcessOwner {
  id: string
  generation: string
}
export interface PluginProcessList {
  processes: { id: string; status: HostProcessStatus }[]
}

function parseWebSession(value: unknown): PluginWebSession {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new TypeError('Invalid session')
  const { url, ...reply } = value as Record<string, unknown>
  if (typeof url !== 'string' || url.length > 8192) throw new TypeError('Invalid session URL')
  const parsed = new URL(url)
  if (
    !(
      (parsed.protocol === 'se-plugin:' && parsed.hostname === 'localhost') ||
      (parsed.protocol === 'http:' && parsed.hostname === 'se-plugin.localhost')
    ) ||
    parsed.port ||
    parsed.username ||
    parsed.password ||
    parsed.search
  )
    throw new TypeError('Invalid session origin')
  const session = parseWebOpenReply(reply)
  // Manifest entries are relative to ui/; the scheme serves them at their package path.
  const entry = session.entry.startsWith('ui/') ? session.entry : `ui/${session.entry}`
  const expectedPath = `/${[session.pluginId, session.generation, session.sessionId, ...entry.split('/')].map(encodeURIComponent).join('/')}`
  if (parsed.pathname !== expectedPath) throw new TypeError('Invalid session path')
  return { ...session, url }
}
function parseProcessList(value: unknown): PluginProcessList {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new TypeError('Invalid process list')
  const record = value as Record<string, unknown>
  if (
    Object.keys(record).some((key) => key !== 'processes') ||
    !Array.isArray(record.processes) ||
    record.processes.length > 64
  )
    throw new TypeError('Invalid process list')
  const ids = new Set<string>()
  return {
    processes: record.processes.map((item: unknown) => {
      if (!item || typeof item !== 'object' || Array.isArray(item))
        throw new TypeError('Invalid process')
      const entry = item as Record<string, unknown>
      if (
        Object.keys(entry).some((key) => key !== 'id' && key !== 'status') ||
        typeof entry.id !== 'string' ||
        !entry.id ||
        entry.id.length > 256 ||
        ids.has(entry.id)
      )
        throw new TypeError('Invalid process')
      ids.add(entry.id)
      return { id: entry.id, status: parseHostProcessStatus(entry.status) }
    })
  }
}

export interface TauriPluginApi extends PluginApi {
  openWebSession(request: WebOpenRequest): Promise<IpcResult<PluginWebSession>>
  requestWeb(request: WebRequest): Promise<IpcResult<WebRequestReply>>
  readWebSession(request: {
    sessionId: string
    afterSequence: number
  }): Promise<IpcResult<WebReadReply>>
  closeWebSession(request: { sessionId: string }): Promise<IpcResult<WebCloseReply>>
  listProcesses(request: PluginProcessOwner): Promise<IpcResult<PluginProcessList>>
  getProcessLogs(
    request: PluginProcessOwner & { processId: string; tail: number }
  ): Promise<IpcResult<HostProcessLogsReply>>
  controlProcess(
    request: PluginProcessOwner & { processId: string; action: 'start' | 'stop' | 'restart' }
  ): Promise<IpcResult<HostProcessStatus>>
  onChanged(handler: (event: PluginEvent) => void): HostEventSubscription
  onConnectionChanged(handler: (event: PluginConnectionChanged) => void): HostEventSubscription
}

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

/** No web fallback: neither invoke nor HTTP nor event transport is touched in a browser. */
export function createTauriPluginApi(deps: PluginApiDependencies = {}): TauriPluginApi {
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
export const pluginApi = createTauriPluginApi()
