import type { IpcResult } from '@shared/types/ipc.types'
import type { PluginApi, PluginConnectionChanged, PluginEvent } from '@shared/types/plugin.types'
import {
  type HostProcessLogsReply,
  type HostProcessStatus,
  parseHostProcessStatus
} from '@shared/types/plugin-host.types'
import {
  parseWebOpenReply,
  type WebCloseReply,
  type WebOpenReply,
  type WebOpenRequest,
  type WebReadReply,
  type WebRequest,
  type WebRequestReply
} from '@shared/types/plugin-web.types'
import type { HostEventSubscription } from '@/lib/host-events'

// URL is a GUI transport capability; the Plugin Core WebOpenReply deliberately has no URL.
export type PluginWebSession = WebOpenReply & { url: string }
export interface PluginProcessOwner {
  id: string
  generation: string
}
export interface PluginProcessList {
  processes: { id: string; status: HostProcessStatus }[]
}

/**
 * One plugin facade shape shared by the Tauri (invoke) and browser (HTTP)
 * transports. Runtime-specific methods are part of the contract so callers do
 * not branch on where the plugin host lives.
 */
export interface PluginRuntimeApi extends PluginApi {
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

/** Desktop transport name kept for existing imports and tests. */
export type TauriPluginApi = PluginRuntimeApi

/** Browser sessions are same-origin relative paths served by `se-server`. */
export const PLUGIN_UI_PATH_PREFIX = '/plugin-ui/'

function assertRelativeSessionUrl(url: string, session: WebOpenReply): void {
  // No scheme, authority, query, fragment or traversal: a same-origin relative path only.
  if (
    !url.startsWith(PLUGIN_UI_PATH_PREFIX) ||
    url.includes('://') ||
    url.includes('?') ||
    url.includes('#') ||
    url.includes('\\') ||
    url.slice(PLUGIN_UI_PATH_PREFIX.length).includes('//')
  )
    throw new TypeError('Invalid session URL')
  const entry = session.entry.startsWith('ui/') ? session.entry : `ui/${session.entry}`
  const expectedPath = `${PLUGIN_UI_PATH_PREFIX}${[
    session.pluginId,
    session.generation,
    session.sessionId,
    ...entry.split('/')
  ]
    .map(encodeURIComponent)
    .join('/')}`
  if (url !== expectedPath) throw new TypeError('Invalid session path')
}

/**
 * Desktop sessions are capability URLs served by the plugin scheme; browser
 * sessions are same-origin relative paths served by `se-server`.
 */
export function parseWebSession(
  value: unknown,
  transport: 'desktop' | 'web' = 'desktop'
): PluginWebSession {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new TypeError('Invalid session')
  const { url, ...reply } = value as Record<string, unknown>
  if (typeof url !== 'string' || url.length > 8192) throw new TypeError('Invalid session URL')
  const session = parseWebOpenReply(reply)
  if (transport === 'web') {
    assertRelativeSessionUrl(url, session)
    return { ...session, url }
  }
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
  // Manifest entries are relative to ui/; the scheme serves them at their package path.
  const entry = session.entry.startsWith('ui/') ? session.entry : `ui/${session.entry}`
  const expectedPath = `/${[session.pluginId, session.generation, session.sessionId, ...entry.split('/')].map(encodeURIComponent).join('/')}`
  if (parsed.pathname !== expectedPath) throw new TypeError('Invalid session path')
  return { ...session, url }
}

export function parseProcessList(value: unknown): PluginProcessList {
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
