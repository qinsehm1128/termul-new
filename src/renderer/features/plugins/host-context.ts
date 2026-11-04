import type { IpcResult } from '@shared/types/ipc.types'
import type {
  AgentSessionContext,
  HostContextAck,
  HostContextPublishRequest,
  HostContextState,
  HostSourceEvent,
  PluginApi
} from '@shared/types/plugin.types'

export interface HostContextSources {
  projects: readonly { id: string; name: string; path?: string; isArchived?: boolean }[]
  activeProjectId: string
  terminals: readonly { id: string; ptyId?: string | null; projectId?: string | null }[]
  sessions: readonly {
    id: string
    projectId: string
    status: AgentSessionContext['status']
    activeTurn: boolean
    openTurnId: string | null
    replaying?: unknown
  }[]
  activeTerminalId: string | null
  activeSessionId: string | null
  focus: HostContextState['focus']
}
const scalarText = (value: string, max: number): string =>
  Array.from(value)
    .slice(0, max)
    .join('')
    .replace(/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/gu, '\uFFFD')
const validId = (value: string | null | undefined): value is string =>
  !!value && Array.from(value).length <= 128 && scalarText(value, 128) === value
const size = (value: unknown): number => new TextEncoder().encode(JSON.stringify(value)).length

/** Explicit field construction is a privacy boundary: never spread source records. */
export function projectHostContext(source: HostContextSources): HostContextState {
  const projects = source.projects.filter((p) => !p.isArchived && validId(p.id))
  const active = projects.find((p) => p.id === source.activeProjectId && !!p.path)
  const terminals = source.terminals
    .filter((t) => validId(t.id))
    .map((t) => ({
      id: t.id,
      ptyId: validId(t.ptyId) ? t.ptyId : null,
      projectId: validId(t.projectId) ? t.projectId : null
    }))
  const sessions = source.sessions
    .filter((s) => validId(s.id) && validId(s.projectId))
    .map((s) => ({
      id: s.id,
      projectId: s.projectId,
      status: s.status,
      activeTurn: s.activeTurn,
      openTurnId: validId(s.openTurnId) ? s.openTurnId : null,
      replaying: !!s.replaying
    }))
  // Keep the focused records when deterministic truncation is required.
  const byActive = (id: string | null) => (a: { id: string }, b: { id: string }) =>
    Number(b.id === id) - Number(a.id === id) || a.id.localeCompare(b.id)
  terminals.sort(byActive(source.activeTerminalId))
  sessions.sort(byActive(source.activeSessionId))
  const snapshot: HostContextState = {
    activeProject: active
      ? {
          id: active.id,
          name: scalarText(active.name || active.id, 128),
          path: scalarText(active.path!, 4096)
        }
      : null,
    projectIds: [...new Set(projects.map((p) => p.id))]
      .sort(
        (a, b) =>
          Number(b === source.activeProjectId) - Number(a === source.activeProjectId) ||
          a.localeCompare(b)
      )
      .slice(0, 256),
    activeTerminalId: terminals.some((t) => t.id === source.activeTerminalId)
      ? source.activeTerminalId
      : null,
    activeSessionId: sessions.some((s) => s.id === source.activeSessionId)
      ? source.activeSessionId
      : null,
    terminals: terminals.slice(0, 256),
    agentSessions: sessions.slice(0, 256),
    focus: { textInput: source.focus.textInput, terminal: source.focus.terminal },
    truncated:
      projects.length > 256 ||
      terminals.length > 256 ||
      sessions.length > 256 ||
      (!!active && (Array.from(active.name).length > 128 || Array.from(active.path!).length > 4096))
  }
  while (size(snapshot) > 128 * 1024) {
    snapshot.truncated = true
    if (snapshot.agentSessions.length > Number(snapshot.activeSessionId !== null))
      snapshot.agentSessions.pop()
    else if (snapshot.terminals.length > Number(snapshot.activeTerminalId !== null))
      snapshot.terminals.pop()
    else if (snapshot.projectIds.length > Number(snapshot.activeProject !== null))
      snapshot.projectIds.pop()
    else break
  }
  snapshot.activeTerminalId = snapshot.terminals.some((t) => t.id === source.activeTerminalId)
    ? source.activeTerminalId
    : null
  snapshot.activeSessionId = snapshot.agentSessions.some((s) => s.id === source.activeSessionId)
    ? source.activeSessionId
    : null
  return snapshot
}

export function diffHostContext(
  before: HostContextState,
  after: HostContextState
): HostSourceEvent[] {
  const events: HostSourceEvent[] = []
  if (before.activeProject?.id !== after.activeProject?.id)
    events.push({
      topic: 'project.activeChanged',
      projectId: after.activeProject?.id ?? null,
      data: {
        previousProjectId: before.activeProject?.id ?? null,
        projectId: after.activeProject?.id ?? null
      }
    })
  const oldTerminals = new Map(before.terminals.map((t) => [t.id, t]))
  const newTerminals = new Map(after.terminals.map((t) => [t.id, t]))
  for (const terminal of after.terminals)
    if (!oldTerminals.has(terminal.id))
      events.push({
        topic: 'terminal.created',
        projectId: terminal.projectId,
        data: { terminal }
      })
  for (const terminal of before.terminals)
    if (!newTerminals.has(terminal.id))
      events.push({
        topic: 'terminal.closed',
        projectId: terminal.projectId,
        data: { terminal }
      })
  if (before.activeTerminalId !== after.activeTerminalId)
    events.push({
      topic: 'terminal.activeChanged',
      projectId: newTerminals.get(after.activeTerminalId ?? '')?.projectId ?? null,
      data: {
        terminalId: after.activeTerminalId,
        projectId: newTerminals.get(after.activeTerminalId ?? '')?.projectId ?? null
      }
    })
  const oldSessions = new Map(before.agentSessions.map((s) => [s.id, s]))
  const newSessions = new Map(after.agentSessions.map((s) => [s.id, s]))
  for (const session of after.agentSessions)
    if (JSON.stringify(oldSessions.get(session.id)) !== JSON.stringify(session))
      events.push({
        topic: 'agent.session.stateChanged',
        projectId: session.projectId,
        data: { session, removed: false }
      })
  for (const session of before.agentSessions)
    if (!newSessions.has(session.id))
      events.push({
        topic: 'agent.session.stateChanged',
        projectId: session.projectId,
        data: { session, removed: true }
      })
  return events
}

export interface HostContextPublisher {
  capture(snapshot: HostContextState, reset?: boolean): void
  reconnect(): void
  disconnect(): void
  dispose(): void
}

/** Captures transitions synchronously; only transport publication is coalesced. */
export function createHostContextPublisher(options: {
  api: Pick<PluginApi, 'publishHostContext' | 'heartbeatHostContext'>
  sourceId: string
  initial: HostContextState
  onAck?: (ack: HostContextAck, snapshot?: HostContextState) => void
  onUnavailable?: () => void
}): HostContextPublisher {
  let snapshot = options.initial
  let events: HostSourceEvent[] = []
  let reset = true
  let dirty = true
  let connected = false
  let disposed = false
  let inFlight = false
  let sequence = 0
  let epoch = 0
  let lastPublish = -Infinity
  let timer: ReturnType<typeof setTimeout> | undefined
  const schedule = () => {
    if (disposed || !connected || inFlight || timer || !dirty) return
    timer = setTimeout(
      () => {
        timer = undefined
        void flush()
      },
      Math.max(0, 50 - (Date.now() - lastPublish))
    )
  }
  const acknowledge = (
    result: IpcResult<HostContextAck>,
    requestEpoch: number,
    published?: HostContextState
  ) => {
    if (disposed || requestEpoch !== epoch) return
    if (result.success) options.onAck?.(result.data, published)
    else {
      options.onUnavailable?.()
      reset = true
      events = []
      dirty = true
    }
  }
  const flush = async () => {
    if (disposed || !connected || inFlight || !dirty) return
    const request: HostContextPublishRequest = {
      sourceId: options.sourceId,
      sourceSequence: ++sequence,
      reset,
      snapshot,
      events
    }
    // The entire envelope is bounded, too. Lost detail explicitly requires resync.
    if (size(request) > 256 * 1024) {
      request.events = []
      request.reset = true
    }
    events = []
    reset = false
    dirty = false
    inFlight = true
    lastPublish = Date.now()
    const requestEpoch = epoch
    try {
      acknowledge(await options.api.publishHostContext(request), requestEpoch, request.snapshot)
    } catch {
      if (!disposed && requestEpoch === epoch) {
        options.onUnavailable?.()
        reset = true
        dirty = true
        events = []
      }
    } finally {
      inFlight = false
      schedule()
    }
  }
  const heartbeat = setInterval(() => {
    if (disposed || !connected || inFlight) return
    const requestEpoch = epoch
    void options.api
      .heartbeatHostContext({ sourceId: options.sourceId })
      .then((result) => {
        acknowledge(result, requestEpoch)
        schedule()
      })
      .catch(() => {
        if (!disposed && requestEpoch === epoch) {
          options.onUnavailable?.()
          reset = true
          events = []
          dirty = true
          schedule()
        }
      })
  }, 2000)
  return {
    capture(next, forceReset = false) {
      if (disposed) return
      if (!forceReset && JSON.stringify(snapshot) === JSON.stringify(next)) return
      const wasTruncated = snapshot.truncated
      const transitions = diffHostContext(snapshot, next)
      snapshot = next
      if (
        forceReset ||
        wasTruncated ||
        snapshot.truncated ||
        events.length + transitions.length > 128
      ) {
        events = []
        reset = true
      } else if (!reset) events.push(...transitions)
      dirty = true
      schedule()
    },
    reconnect() {
      ++epoch
      connected = true
      reset = true
      events = []
      dirty = true
      schedule()
    },
    disconnect() {
      ++epoch
      connected = false
      events = []
      reset = true
      dirty = true
      if (timer) clearTimeout(timer)
      timer = undefined
    },
    dispose() {
      disposed = true
      ++epoch
      clearInterval(heartbeat)
      if (timer) clearTimeout(timer)
    }
  }
}
