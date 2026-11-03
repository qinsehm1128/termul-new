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
import { type HostEventSubscription, onHostEvent } from '@/lib/host-events'
import { invokeDecodedIpcResult } from '@/lib/invoke-decoded-ipc-result'
import { logFrontendError } from '@/lib/log-api'
import { isTauriContext } from '@/lib/tauri-runtime'

export interface TauriPluginApi extends PluginApi {
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
    if (!result.success)
      void logFrontendError({
        level: 'warn',
        source: 'plugin-api',
        message: `${command}: request failed`
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
    onChanged: (handler) => listen('plugins:changed', parsePluginEvent, handler),
    onConnectionChanged: (handler) =>
      listen('plugins:connection_changed', parsePluginConnectionChanged, handler)
  }
}
export const pluginApi = createTauriPluginApi()
