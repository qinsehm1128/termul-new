/**
 * Live e2e probe that walks the iPhone TermulRemote path against a desktop host.
 *
 * Start the desktop first (`bun run dev`) and turn on Remote Access. Then:
 *
 *   bun scripts/mobile-host-probe.ts
 *   bun scripts/mobile-host-probe.ts --wait 180
 *
 * Auth (never printed): `SE_ACCESS_URL`, `SE_ACCESS_TOKEN`, or
 * `<app data>/remote-tunnel/secrets.json`. Does not read the macOS keychain.
 *
 * Default walks the iPhone path and then the interactive flows: resume/load
 * an existing session, create a new conversation session, and send a prompt.
 * Use `--read-only` to stop after discovery/history. Auth is never printed
 * and the macOS keychain is not read.
 */
import { homedir } from 'node:os'
import { join } from 'node:path'

export const DEFAULT_LOCAL_ORIGIN = 'http://127.0.0.1:18787'
export const HISTORY_TAIL_WINDOW = 60
export const HISTORY_PAGE_LIMIT = 80
export const SEND_PROMPT_TIMEOUT_MS = 180_000
export const CREATE_SESSION_TIMEOUT_MS = 75_000
export const RESUME_SESSION_TIMEOUT_MS = 120_000

export type StepStatus = 'pass' | 'fail' | 'skip'

export type ProbeStep = {
  id: string
  status: StepStatus
  detail: string
  ms: number
}

export type IpcBody<T> = {
  success: boolean
  data?: T
  error?: string
  code?: string
}

export type WsReply = {
  id?: string
  ok?: boolean
  payload?: unknown
  err?: { code?: string; message?: string }
  type?: string
  sid?: string
  seq?: number
  success?: boolean
  data?: unknown
  error?: string
  code?: string
}

export function normalizeWsReply(frame: WsReply): WsReply {
  if (typeof frame.success === 'boolean' && frame.ok === undefined) {
    return {
      ...frame,
      ok: frame.success,
      payload: frame.data,
      err: frame.success ? undefined : { code: frame.code, message: frame.error }
    }
  }
  return frame
}

export type HostConversation = {
  conversationId: string
  title?: string | null
  workspaceCwd: string
  lifecycleState?: string
  lastSeq?: number
}

export type AgentSessionBinding = {
  agentSessionId: string
  runtimeAgentId: string
  executionCwd?: string | null
}

export type ConversationBindingSnapshot = {
  conversationId: string
  binding: AgentSessionBinding | null
}

export type ProjectListPayload = {
  projects?: Array<{ id: string; name: string; path?: string | null }>
  defaultProjectId?: string | null
}

export type AcpCatalog = {
  host?: { os?: string; arch?: string }
  agents?: Array<{ id: string; name?: string; status?: string | null }>
}

export type SessionCursor = {
  sessionId?: string
  watermark?: number
}

export type HistoryPage = {
  records?: Array<{ seq: number; type?: string; payload?: unknown }>
  nextCursor?: number
  complete?: boolean
  targetLastSeq?: number
}

export type ProbeEvent = {
  type: string
  sid?: string
  seq?: number
  payload?: Record<string, unknown>
}

export type NewSessionOutcome = {
  sessionId?: string
  conversationId?: string
  workspaceCwd?: string
  executionCwd?: string
}

export type ResolvedAuth = {
  origin: string
  token: string
  source: 'env-url' | 'env-token' | 'secrets'
  secretsPath?: string
}

export function tokenFromAccessUrl(url: string): { origin: string; token: string } | null {
  try {
    const parsed = new URL(url)
    const fragment = parsed.hash.startsWith('#') ? parsed.hash.slice(1) : parsed.hash
    const token = new URLSearchParams(fragment).get('access_token')
    if (!token) return null
    parsed.hash = ''
    parsed.username = ''
    parsed.password = ''
    return { origin: parsed.origin, token }
  } catch {
    return null
  }
}

export function redactOrigin(origin: string): string {
  try {
    const parsed = new URL(origin)
    parsed.hash = ''
    parsed.username = ''
    parsed.password = ''
    parsed.search = ''
    return parsed.origin
  } catch {
    return 'invalid-origin'
  }
}

export function pairingTokenFromSecrets(raw: string): string | null {
  try {
    const parsed = JSON.parse(raw) as { pairingToken?: unknown }
    return typeof parsed.pairingToken === 'string' && parsed.pairingToken.length > 0
      ? parsed.pairingToken
      : null
  } catch {
    return null
  }
}

export function historyAfterSeq(hostWatermark: number, cachedWatermark = 0): number | null {
  if (hostWatermark <= cachedWatermark) return null
  if (cachedWatermark > 0) return cachedWatermark
  if (hostWatermark > HISTORY_TAIL_WINDOW) return hostWatermark - HISTORY_TAIL_WINDOW
  return 0
}

export function appDataCandidates(home = homedir()): string[] {
  return [
    join(home, 'Library', 'Application Support', 'com.termul-manager.app.dev'),
    join(home, 'Library', 'Application Support', 'com.termul-manager.app'),
    join(home, '.local', 'share', 'com.termul-manager.app.dev'),
    join(home, '.local', 'share', 'com.termul-manager.app')
  ]
}

export function secretsPaths(home = homedir()): string[] {
  return appDataCandidates(home).map((root) => join(root, 'remote-tunnel', 'secrets.json'))
}

export function remoteAccessIntentPaths(home = homedir()): string[] {
  return appDataCandidates(home).map((root) => join(root, 'remote-tunnel', 'remote-access.json'))
}

export type RemoteAccessIntent = {
  wanted: boolean
  publishMode: string
}

export function parseRemoteAccessIntent(raw: string): RemoteAccessIntent | null {
  try {
    const parsed = JSON.parse(raw) as { wanted?: unknown; publishMode?: unknown }
    if (typeof parsed.wanted !== 'boolean') return null
    return {
      wanted: parsed.wanted,
      publishMode: typeof parsed.publishMode === 'string' ? parsed.publishMode : 'unknown'
    }
  } catch {
    return null
  }
}

export async function loadRemoteAccessIntent(home = homedir()): Promise<RemoteAccessIntent | null> {
  for (const path of remoteAccessIntentPaths(home)) {
    const raw = await readTextFile(path)
    if (!raw) continue
    const intent = parseRemoteAccessIntent(raw)
    if (intent) return intent
  }
  return null
}

export function pickBrowsableFile(
  entries: Array<{ path: string; type?: string; name?: string; size?: number }>
): string | null {
  const files = entries.filter((entry) => {
    if ((entry.type ?? 'file') !== 'file') return false
    if (typeof entry.size === 'number' && entry.size > 256 * 1024) return false
    const name = entry.name ?? entry.path.split('/').pop() ?? ''
    return /\.(md|json|txt|ts|tsx|js|rs|toml)$/i.test(name)
  })
  return files[0]?.path ?? null
}

export function asStringList(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === 'string' && item.length > 0)
    : []
}

export function pickLiveAgentId(live: unknown, preferred?: string | null): string | null {
  const ids = asStringList(live)
  if (preferred && ids.includes(preferred)) return preferred
  if (ids[0]) return ids[0]
  return preferred && preferred.length > 0 ? preferred : null
}

export function pickPermissionOption(payload: unknown): string | null {
  const options = (payload as { options?: Array<{ optionId?: string; name?: string }> } | undefined)
    ?.options
  if (!Array.isArray(options) || options.length === 0) return null
  const allow = options.find((option) =>
    /allow|once|yes|accept/i.test(`${option.name ?? ''} ${option.optionId ?? ''}`)
  )
  return allow?.optionId ?? options[0]?.optionId ?? null
}

export function historyContainsMarker(
  records: Array<{ payload?: unknown }> | undefined,
  marker: string
): boolean {
  if (!records) return false
  return records.some((record) => JSON.stringify(record.payload ?? {}).includes(marker))
}

export function e2ePrompt(marker: string): string {
  return `Reply with exactly ${marker} and nothing else.`
}

export function subscribePayload(
  sessionId: string,
  lastSeq?: number,
  stale = false
): Record<string, unknown> {
  if (stale || lastSeq === undefined) return { sessionId }
  return { sessionId, lastSeq }
}

export function wsUrl(origin: string, path: '/ws' | '/terminal/ws'): string {
  const url = new URL(origin)
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
  url.pathname = path
  url.hash = ''
  url.search = ''
  return url.toString()
}

function failStep(id: string, detail: string, started: number): ProbeStep {
  return { id, status: 'fail', detail, ms: Math.round(performance.now() - started) }
}

function passStep(id: string, detail: string, started: number): ProbeStep {
  return { id, status: 'pass', detail, ms: Math.round(performance.now() - started) }
}

function skipStep(id: string, detail: string): ProbeStep {
  return { id, status: 'skip', detail, ms: 0 }
}

async function readTextFile(path: string): Promise<string | null> {
  const file = Bun.file(path)
  if (!(await file.exists())) return null
  return file.text()
}

export async function resolveAuthFromDisk(
  env: NodeJS.ProcessEnv = process.env,
  home = homedir()
): Promise<ResolvedAuth | null> {
  const fromUrl = env.SE_ACCESS_URL ? tokenFromAccessUrl(env.SE_ACCESS_URL) : null
  if (fromUrl) {
    return { origin: fromUrl.origin, token: fromUrl.token, source: 'env-url' }
  }
  const host = env.SE_HOST ?? DEFAULT_LOCAL_ORIGIN
  if (env.SE_ACCESS_TOKEN) {
    return { origin: host, token: env.SE_ACCESS_TOKEN, source: 'env-token' }
  }
  for (const path of secretsPaths(home)) {
    const raw = await readTextFile(path)
    if (!raw) continue
    const token = pairingTokenFromSecrets(raw)
    if (token) return { origin: host, token, source: 'secrets', secretsPath: path }
  }
  return null
}

async function probeHealth(origin: string, token?: string, timeoutMs = 400): Promise<boolean> {
  const headers: Record<string, string> = {}
  if (token) headers.authorization = `Bearer ${token}`
  const response = await fetch(`${origin}/health`, {
    headers,
    signal: AbortSignal.timeout(timeoutMs)
  })
  if (!response.ok) return false
  const text = (await response.text()).trim()
  return text === 'OK' || text.startsWith('OK')
}

export function listenPortsFromLsof(text: string): {
  named: string[]
  wildcard: string[]
  loopback: string[]
} {
  const named = new Set<string>()
  const wildcard = new Set<string>()
  const loopback = new Set<string>()
  for (const line of text.split('\n')) {
    const match = line.match(/(?:(127\.0\.0\.1|\[::1\])|(\*|0\.0\.0\.0|\[::\])):(\d+)/)
    const port = match?.[3]
    if (!port) continue
    if (match?.[1]) loopback.add(port)
    if (match?.[2]) wildcard.add(port)
    if (/termul|Termul/i.test(line)) named.add(port)
  }
  return { named: [...named], wildcard: [...wildcard], loopback: [...loopback] }
}

async function readLsofListenTable(): Promise<string> {
  const proc = Bun.spawn({
    cmd: ['lsof', '-nP', '-iTCP', '-sTCP:LISTEN'],
    stdout: 'pipe',
    stderr: 'pipe'
  })
  const text = await new Response(proc.stdout).text()
  await proc.exited
  return text
}

function originsFromPorts(ports: string[]): string[] {
  return ports.map((port) => `http://127.0.0.1:${port}`)
}

async function firstHealthyOrigin(
  origins: string[],
  token?: string,
  timeoutMs = 400
): Promise<string | null> {
  const seen = new Set<string>()
  const unique: string[] = []
  for (const origin of origins) {
    const key = redactOrigin(origin)
    if (seen.has(key)) continue
    seen.add(key)
    unique.push(origin)
  }
  const batchSize = 12
  for (let index = 0; index < unique.length; index += batchSize) {
    const batch = unique.slice(index, index + batchSize)
    const hits = await Promise.all(
      batch.map(async (origin) => {
        try {
          return (await probeHealth(origin, token, timeoutMs)) ? origin : null
        } catch {
          return null
        }
      })
    )
    const found = hits.find((origin): origin is string => origin !== null)
    if (found) return found
  }
  return null
}

export async function discoverOrigin(preferred: string, token?: string): Promise<string | null> {
  const table = await readLsofListenTable()
  const ports = listenPortsFromLsof(table)
  return firstHealthyOrigin(
    [
      preferred,
      DEFAULT_LOCAL_ORIGIN,
      ...originsFromPorts(ports.named),
      ...originsFromPorts(ports.wildcard),
      ...originsFromPorts(ports.loopback)
    ],
    token
  )
}

async function ipc<T>(
  origin: string,
  token: string,
  path: string,
  init: RequestInit = {}
): Promise<IpcBody<T>> {
  const headers = new Headers(init.headers)
  headers.set('authorization', `Bearer ${token}`)
  if (init.body && !headers.has('content-type')) {
    headers.set('content-type', 'application/json')
  }
  const response = await fetch(`${origin}${path}`, {
    ...init,
    headers,
    signal: init.signal ?? AbortSignal.timeout(15000)
  })
  if (!response.ok) {
    throw new Error(`${path} HTTP ${response.status}`)
  }
  return (await response.json()) as IpcBody<T>
}

class HostSocket {
  readonly events: ProbeEvent[] = []
  autoRespondPermissions = false
  private readonly pending = new Map<string, (reply: WsReply) => void>()
  private readonly waiters: Array<() => void> = []
  private serial = 0
  private socket: WebSocket | null = null

  async connect(origin: string, token: string, path: '/ws' | '/terminal/ws'): Promise<void> {
    const hostOrigin = redactOrigin(origin)
    this.socket = new WebSocket(wsUrl(origin, path), {
      headers: {
        authorization: `Bearer ${token}`,
        origin: hostOrigin
      }
    })
    await new Promise<void>((resolve, reject) => {
      let settled = false
      const finish = (error?: Error) => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        if (error) reject(error)
        else resolve()
      }
      const timer = setTimeout(() => finish(new Error(`${path} open timed out`)), 12000)
      this.socket?.addEventListener('error', () => {
        finish(new Error(`${path} upgrade rejected`))
      })
      this.socket?.addEventListener('close', (event) => {
        finish(new Error(`${path} closed ${event.code}`))
      })
      this.socket?.addEventListener('open', () => {
        finish()
      })
      this.socket?.addEventListener('message', (event) => {
        this.handleIncoming(JSON.parse(String(event.data)) as WsReply, finish)
      })
    })
    const auth = await this.request('authenticate', { token })
    if (!auth.ok) {
      throw new Error(`authenticate ${auth.err?.code ?? auth.err?.message ?? 'failed'}`)
    }
  }

  request(
    type: string,
    payload: Record<string, unknown> = {},
    timeoutMs = 15000
  ): Promise<WsReply> {
    const id = `probe-${++this.serial}`
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`${type} timed out`)), timeoutMs)
      this.pending.set(id, (reply) => {
        clearTimeout(timer)
        resolve(reply)
      })
      this.socket?.send(JSON.stringify({ id, type, payload }))
    })
  }

  waitForEvent(
    predicate: (event: ProbeEvent) => boolean,
    timeoutMs: number,
    label: string
  ): Promise<ProbeEvent> {
    const existing = this.events.find(predicate)
    if (existing) return Promise.resolve(existing)
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`${label} timed out`)), timeoutMs)
      const check = () => {
        const hit = this.events.find(predicate)
        if (!hit) return
        clearTimeout(timer)
        const index = this.waiters.indexOf(check)
        if (index >= 0) this.waiters.splice(index, 1)
        resolve(hit)
      }
      this.waiters.push(check)
    })
  }

  close(): void {
    this.socket?.close()
    this.socket = null
  }

  private handleIncoming(raw: WsReply, onHello?: (error?: Error) => void): void {
    const frame = normalizeWsReply(raw)
    if (frame.type === 'auth_required') {
      onHello?.()
      return
    }
    if (frame.id && this.pending.has(frame.id)) {
      this.pending.get(frame.id)!(frame)
      this.pending.delete(frame.id)
      return
    }
    if (!frame.type) return
    const event: ProbeEvent = {
      type: frame.type,
      sid: typeof frame.sid === 'string' ? frame.sid : undefined,
      seq: typeof frame.seq === 'number' ? frame.seq : undefined,
      payload:
        frame.payload && typeof frame.payload === 'object'
          ? (frame.payload as Record<string, unknown>)
          : undefined
    }
    this.events.push(event)
    for (const waiter of [...this.waiters]) waiter()
    if (this.autoRespondPermissions) {
      void this.autoRespond(event)
    }
  }

  private async autoRespond(event: ProbeEvent): Promise<void> {
    if (event.type === 'permission_request') {
      const optionId = pickPermissionOption(event.payload)
      const agentId = typeof event.payload?.agentId === 'string' ? event.payload.agentId : null
      const requestId =
        typeof event.payload?.requestId === 'string' ? event.payload.requestId : null
      if (!optionId || !agentId || !requestId) return
      try {
        await this.request('respond_permission', { agentId, requestId, optionId })
      } catch {
        /* keep waiting for the prompt turn */
      }
      return
    }
    if (event.type === 'question_request') {
      const agentId = typeof event.payload?.agentId === 'string' ? event.payload.agentId : null
      const questionId =
        typeof event.payload?.questionId === 'string' ? event.payload.questionId : null
      const first = (event.payload?.options as Array<{ value?: string }> | undefined)?.[0]?.value
      if (!agentId || !questionId || !first) return
      try {
        await this.request('answer_question', { agentId, questionId, values: [first] })
      } catch {
        /* keep waiting for the prompt turn */
      }
    }
  }
}

function unwrap<T>(body: IpcBody<T>, path: string): T {
  if (!body.success || body.data === undefined) {
    throw new Error(`${path} ${body.code ?? 'failed'}: ${body.error ?? 'no data'}`)
  }
  return body.data
}

async function requireOk(
  socket: HostSocket,
  type: string,
  payload: Record<string, unknown> = {},
  timeoutMs = 15000
): Promise<WsReply> {
  const reply = await socket.request(type, payload, timeoutMs)
  if (!reply.ok) {
    throw new Error(reply.err?.code ?? reply.err?.message ?? 'failed')
  }
  return reply
}

async function subscribeSession(
  socket: HostSocket,
  sessionId: string,
  lastSeq?: number
): Promise<void> {
  let reply = await socket.request('subscribe', subscribePayload(sessionId, lastSeq))
  if (!reply.ok && reply.err?.code === 'stale') {
    reply = await socket.request('subscribe', subscribePayload(sessionId, lastSeq, true))
  }
  if (!reply.ok) {
    throw new Error(reply.err?.code ?? reply.err?.message ?? 'failed')
  }
}

async function reopenSession(
  socket: HostSocket,
  payload: Record<string, unknown>
): Promise<string> {
  const resume = await socket.request('resume_session', payload, RESUME_SESSION_TIMEOUT_MS)
  if (resume.ok) return 'resume_session'
  const load = await socket.request('load_session', payload, RESUME_SESSION_TIMEOUT_MS)
  if (load.ok) return 'load_session'
  throw new Error(
    load.err?.code ?? resume.err?.code ?? load.err?.message ?? resume.err?.message ?? 'failed'
  )
}

async function runInteractiveSteps(
  steps: ProbeStep[],
  socket: HostSocket,
  input: {
    origin: string
    token: string
    conversation?: HostConversation
    binding: AgentSessionBinding | null
    project?: { id: string; name: string; path?: string | null }
    liveAgentIds: string[]
  }
): Promise<void> {
  socket.autoRespondPermissions = true
  const agentId = pickLiveAgentId(input.liveAgentIds, input.binding?.runtimeAgentId)
  const historicalSessionId = input.binding?.agentSessionId
  const historicalCwd =
    input.binding?.executionCwd ?? input.conversation?.workspaceCwd ?? input.project?.path ?? null

  let started = performance.now()
  if (!historicalSessionId || !agentId || !historicalCwd) {
    steps.push(skipStep('ws.resume-history', 'no bound ACP session to resume'))
  } else {
    try {
      const before = await requireOk(socket, 'get_session_cursor', {
        sessionId: historicalSessionId
      })
      const beforeMark = (before.payload as SessionCursor | undefined)?.watermark ?? 0
      const method = await reopenSession(socket, {
        agentId,
        sessionId: historicalSessionId,
        cwd: historicalCwd,
        conversationId: input.conversation?.conversationId
      })
      const after = await requireOk(socket, 'get_session_cursor', {
        sessionId: historicalSessionId
      })
      const afterMark = (after.payload as SessionCursor | undefined)?.watermark ?? 0
      steps.push(
        passStep('ws.resume-history', `${method} watermark ${beforeMark}->${afterMark}`, started)
      )
    } catch (error) {
      steps.push(
        failStep(
          'ws.resume-history',
          error instanceof Error ? error.message : String(error),
          started
        )
      )
    }
  }

  const cwd = input.project?.path ?? input.conversation?.workspaceCwd
  started = performance.now()
  if (!agentId) {
    steps.push(skipStep('ws.create-session', 'no live ACP agent on the host'))
    steps.push(skipStep('ws.send-prompt', 'no new session'))
    steps.push(skipStep('ws.send-accepted', 'no new session'))
    steps.push(skipStep('ws.send-complete', 'no new session'))
    steps.push(skipStep('http.conversations-sync', 'no new session'))
    steps.push(skipStep('ws.reload-created', 'no new session'))
    return
  }
  if (!cwd) {
    steps.push(skipStep('ws.create-session', 'no project or conversation cwd'))
    steps.push(skipStep('ws.send-prompt', 'no new session'))
    steps.push(skipStep('ws.send-accepted', 'no new session'))
    steps.push(skipStep('ws.send-complete', 'no new session'))
    steps.push(skipStep('http.conversations-sync', 'no new session'))
    steps.push(skipStep('ws.reload-created', 'no new session'))
    return
  }

  let created: NewSessionOutcome | null = null
  try {
    const reply = await requireOk(
      socket,
      'create_session',
      { agentId, cwd, ephemeral: false },
      CREATE_SESSION_TIMEOUT_MS
    )
    created = (reply.payload ?? {}) as NewSessionOutcome
    if (!created.sessionId) throw new Error('create_session returned no sessionId')
    steps.push(
      passStep(
        'ws.create-session',
        created.conversationId ? `session + conversation` : 'session without conversation id',
        started
      )
    )
  } catch (error) {
    steps.push(
      failStep('ws.create-session', error instanceof Error ? error.message : String(error), started)
    )
    steps.push(skipStep('ws.send-prompt', 'create_session failed'))
    steps.push(skipStep('ws.send-accepted', 'create_session failed'))
    steps.push(skipStep('ws.send-complete', 'create_session failed'))
    steps.push(skipStep('http.conversations-sync', 'create_session failed'))
    steps.push(skipStep('ws.reload-created', 'create_session failed'))
    return
  }

  started = performance.now()
  try {
    if (!created.conversationId) {
      throw new Error('create_session returned no conversationId')
    }
    const listed = unwrap(
      await ipc<HostConversation[]>(input.origin, input.token, '/conversations'),
      '/conversations'
    ).filter((item) => item.lifecycleState !== 'deleted')
    if (!listed.some((item) => item.conversationId === created.conversationId)) {
      throw new Error('host conversation list is missing the phone-created conversation')
    }
    steps.push(
      passStep(
        'http.conversations-sync',
        'GET /conversations includes the new conversation',
        started
      )
    )
  } catch (error) {
    steps.push(
      failStep(
        'http.conversations-sync',
        error instanceof Error ? error.message : String(error),
        started
      )
    )
  }

  started = performance.now()
  try {
    const cursor = await requireOk(socket, 'get_session_cursor', { sessionId: created.sessionId })
    const watermark = (cursor.payload as SessionCursor | undefined)?.watermark ?? 0
    await subscribeSession(socket, created.sessionId, watermark)
    steps.push(passStep('ws.create-subscribe', `lastSeq=${watermark}`, started))
  } catch (error) {
    steps.push(
      failStep(
        'ws.create-subscribe',
        error instanceof Error ? error.message : String(error),
        started
      )
    )
  }

  const marker = `e2e-ok-${Date.now()}`
  const turnId = crypto.randomUUID()
  const sendStarted = performance.now()
  const sendPromise = socket.request(
    'send_prompt',
    {
      agentId,
      sessionId: created.sessionId,
      text: e2ePrompt(marker),
      turnId
    },
    SEND_PROMPT_TIMEOUT_MS
  )
  started = performance.now()
  try {
    const accepted = await Promise.race([
      socket.waitForEvent(
        (event) => event.type === 'user_prompt',
        SEND_PROMPT_TIMEOUT_MS,
        'user_prompt'
      ),
      sendPromise.then((reply) => {
        if (!reply.ok) throw new Error(reply.err?.code ?? reply.err?.message ?? 'failed')
        return 'send_prompt'
      })
    ])
    steps.push(
      passStep(
        'ws.send-accepted',
        accepted === 'send_prompt' ? 'send_prompt reply' : 'user_prompt event',
        started
      )
    )
  } catch (error) {
    steps.push(
      failStep('ws.send-accepted', error instanceof Error ? error.message : String(error), started)
    )
  }

  started = sendStarted
  try {
    const reply = await sendPromise
    if (!reply.ok) throw new Error(reply.err?.code ?? reply.err?.message ?? 'failed')
    const complete = socket.events.some(
      (event) => event.type === 'prompt_complete' && event.sid === created?.sessionId
    )
    steps.push(
      passStep(
        'ws.send-complete',
        complete ? 'send_prompt + prompt_complete' : 'send_prompt reply',
        started
      )
    )
  } catch (error) {
    const complete = socket.events.some((event) => event.type === 'prompt_complete')
    if (complete) {
      steps.push(passStep('ws.send-complete', 'prompt_complete after wait ended', started))
    } else {
      steps.push(
        failStep(
          'ws.send-complete',
          error instanceof Error ? error.message : String(error),
          started
        )
      )
    }
  }

  started = performance.now()
  try {
    const cursor = await requireOk(socket, 'get_session_cursor', { sessionId: created.sessionId })
    const watermark = (cursor.payload as SessionCursor | undefined)?.watermark ?? 0
    const afterSeq = historyAfterSeq(watermark) ?? 0
    const page = await requireOk(socket, 'get_session_payload_page', {
      sessionId: created.sessionId,
      afterSeq,
      limit: HISTORY_PAGE_LIMIT,
      ...(watermark > 0 ? { targetLastSeq: watermark } : {})
    })
    const records = (page.payload as HistoryPage | undefined)?.records ?? []
    if (
      watermark === 0 &&
      records.length === 0 &&
      !socket.events.some((event) => event.type === 'user_prompt')
    ) {
      throw new Error('new session history is empty after send')
    }
    const sawMarker =
      historyContainsMarker(records, marker) ||
      socket.events.some((event) => JSON.stringify(event.payload ?? {}).includes(marker))
    steps.push(
      passStep(
        'ws.reload-created',
        `watermark=${watermark} records=${records.length}${sawMarker ? ' marker' : ''}`,
        started
      )
    )
  } catch (error) {
    steps.push(
      failStep('ws.reload-created', error instanceof Error ? error.message : String(error), started)
    )
  }

  started = performance.now()
  try {
    const method = await reopenSession(socket, {
      agentId,
      sessionId: created.sessionId,
      cwd: created.executionCwd ?? created.workspaceCwd ?? cwd,
      ...(created.conversationId ? { conversationId: created.conversationId } : {})
    })
    steps.push(passStep('ws.resume-created', method, started))
  } catch (error) {
    steps.push(
      failStep('ws.resume-created', error instanceof Error ? error.message : String(error), started)
    )
  }
}

export async function runMobileHostProbe(options: {
  origin?: string
  token?: string
  json?: boolean
  waitMs?: number
  interactive?: boolean
}): Promise<ProbeStep[]> {
  const steps: ProbeStep[] = []
  const auth = options.token
    ? {
        origin: options.origin ?? DEFAULT_LOCAL_ORIGIN,
        token: options.token,
        source: 'env-token' as const
      }
    : await resolveAuthFromDisk()
  const preferred = options.origin ?? auth?.origin ?? DEFAULT_LOCAL_ORIGIN
  let started = performance.now()

  let origin = await discoverOrigin(preferred, auth?.token)
  const deadline = started + Math.max(0, options.waitMs ?? 0)
  while (!origin && performance.now() < deadline) {
    await Bun.sleep(1000)
    origin = await discoverOrigin(preferred, auth?.token)
  }
  if (!origin) {
    const intent = await loadRemoteAccessIntent()
    const intentHint = intent
      ? ` wanted=${intent.wanted} publishMode=${intent.publishMode}`
      : ' remote-access.json missing'
    steps.push(
      failStep(
        'discover',
        `no live /health at ${redactOrigin(preferred)} or LAN listeners — start bun run dev and turn on Remote Access.${intentHint}`,
        started
      )
    )
    return steps
  }
  steps.push(
    passStep('discover', `${redactOrigin(origin)} token=${auth ? auth.source : 'none'}`, started)
  )

  started = performance.now()
  try {
    const ok = await probeHealth(origin, auth?.token)
    if (!ok) throw new Error('body was not OK')
    steps.push(passStep('http.health', 'GET /health', started))
  } catch (error) {
    steps.push(
      failStep('http.health', error instanceof Error ? error.message : String(error), started)
    )
    return steps
  }

  if (!auth) {
    steps.push(
      skipStep('auth', 'no pairing token in SE_ACCESS_URL / SE_ACCESS_TOKEN / secrets.json')
    )
    return steps
  }

  let conversations: HostConversation[] = []
  started = performance.now()
  try {
    conversations = unwrap(
      await ipc<HostConversation[]>(origin, auth.token, '/conversations'),
      '/conversations'
    ).filter((item) => item.lifecycleState !== 'deleted')
    steps.push(passStep('http.conversations', `${conversations.length} ready`, started))
  } catch (error) {
    steps.push(
      failStep(
        'http.conversations',
        error instanceof Error ? error.message : String(error),
        started
      )
    )
  }

  started = performance.now()
  try {
    unwrap(
      await ipc<unknown>(origin, auth.token, '/conversations/host-status'),
      '/conversations/host-status'
    )
    steps.push(passStep('http.host-status', 'GET /conversations/host-status', started))
  } catch (error) {
    steps.push(
      failStep('http.host-status', error instanceof Error ? error.message : String(error), started)
    )
  }

  let projects: ProjectListPayload = {}
  started = performance.now()
  try {
    projects = unwrap(await ipc<ProjectListPayload>(origin, auth.token, '/projects'), '/projects')
    steps.push(passStep('http.projects', `${projects.projects?.length ?? 0} projects`, started))
  } catch (error) {
    steps.push(
      failStep('http.projects', error instanceof Error ? error.message : String(error), started)
    )
  }

  started = performance.now()
  try {
    const catalog = unwrap(
      await ipc<AcpCatalog>(origin, auth.token, '/acp/catalog'),
      '/acp/catalog'
    )
    steps.push(passStep('http.catalog', `${catalog.agents?.length ?? 0} agents`, started))
  } catch (error) {
    steps.push(
      failStep('http.catalog', error instanceof Error ? error.message : String(error), started)
    )
  }

  const conversation = conversations[0]
  const project = projects.projects?.[0]
  let binding: AgentSessionBinding | null = null
  let workspaceEntries: Array<{ path: string; type?: string; name?: string; size?: number }> = []
  if (!conversation) {
    steps.push(skipStep('http.conversation', 'no conversation to probe'))
    steps.push(skipStep('http.open', 'no conversation to open'))
    steps.push(skipStep('http.binding', 'no conversation to probe'))
    steps.push(skipStep('http.fs', 'no conversation workspace'))
  } else {
    started = performance.now()
    try {
      unwrap(
        await ipc<HostConversation>(
          origin,
          auth.token,
          `/conversations/${conversation.conversationId}`
        ),
        '/conversations/{id}'
      )
      steps.push(passStep('http.conversation', conversation.conversationId, started))
    } catch (error) {
      steps.push(
        failStep(
          'http.conversation',
          error instanceof Error ? error.message : String(error),
          started
        )
      )
    }

    started = performance.now()
    try {
      unwrap(
        await ipc<unknown>(
          origin,
          auth.token,
          `/conversations/${conversation.conversationId}/open`,
          {
            method: 'POST',
            body: '{}'
          }
        ),
        '/conversations/{id}/open'
      )
      steps.push(passStep('http.open', 'POST /conversations/{id}/open', started))
    } catch (error) {
      steps.push(
        failStep('http.open', error instanceof Error ? error.message : String(error), started)
      )
    }

    started = performance.now()
    try {
      const snapshot = unwrap(
        await ipc<ConversationBindingSnapshot>(
          origin,
          auth.token,
          `/conversations/${conversation.conversationId}/binding`
        ),
        '/conversations/{id}/binding'
      )
      binding = snapshot.binding
      steps.push(
        passStep(
          'http.binding',
          binding?.agentSessionId ? 'has current ACP session' : 'no current ACP binding',
          started
        )
      )
    } catch (error) {
      steps.push(
        failStep('http.binding', error instanceof Error ? error.message : String(error), started)
      )
    }

    started = performance.now()
    try {
      workspaceEntries = unwrap(
        await ipc<Array<{ path: string; type?: string; name?: string; size?: number }>>(
          origin,
          auth.token,
          `/fs/browse?path=${encodeURIComponent(conversation.workspaceCwd)}`
        ),
        '/fs/browse'
      )
      steps.push(
        passStep('http.fs', `${workspaceEntries.length} entries in conversation workspace`, started)
      )
    } catch (error) {
      steps.push(
        failStep('http.fs', error instanceof Error ? error.message : String(error), started)
      )
    }
  }

  let projectEntries: Array<{ path: string; type?: string; name?: string; size?: number }> = []
  if (!project) {
    steps.push(skipStep('http.project-fs', 'no project to browse'))
  } else if (!project.path) {
    steps.push(skipStep('http.project-fs', 'project has no path'))
  } else {
    started = performance.now()
    try {
      projectEntries = unwrap(
        await ipc<Array<{ path: string; type?: string; name?: string; size?: number }>>(
          origin,
          auth.token,
          `/fs/browse?path=${encodeURIComponent(project.path)}`
        ),
        '/fs/browse'
      )
      steps.push(
        passStep('http.project-fs', `${projectEntries.length} entries in ${project.name}`, started)
      )
    } catch (error) {
      steps.push(
        failStep('http.project-fs', error instanceof Error ? error.message : String(error), started)
      )
    }
  }

  const filePath = pickBrowsableFile(workspaceEntries) ?? pickBrowsableFile(projectEntries)
  if (!filePath) {
    steps.push(skipStep('http.fs-read', 'no small text file in workspace or project root'))
  } else {
    started = performance.now()
    try {
      unwrap(
        await ipc<{ content?: string; size?: number }>(
          origin,
          auth.token,
          `/fs/read?path=${encodeURIComponent(filePath)}`
        ),
        '/fs/read'
      )
      steps.push(passStep('http.fs-read', 'GET /fs/read', started))
    } catch (error) {
      steps.push(
        failStep('http.fs-read', error instanceof Error ? error.message : String(error), started)
      )
    }
  }

  const acp = new HostSocket()
  started = performance.now()
  try {
    await acp.connect(origin, auth.token, '/ws')
    steps.push(passStep('ws.authenticate', 'ACP /ws', started))
  } catch (error) {
    steps.push(
      failStep('ws.authenticate', error instanceof Error ? error.message : String(error), started)
    )
    return steps
  }

  let liveAgentIds: string[] = []
  const wsCalls: Array<[string, Record<string, unknown>]> = [
    ['ping', {}],
    ['list_acp_catalog', {}],
    ['list_agents', {}],
    ['list_conversations', {}],
    ['list_persisted_sessions', {}],
    ...(conversation
      ? ([['get_conversation_binding', { conversationId: conversation.conversationId }]] as Array<
          [string, Record<string, unknown>]
        >)
      : [])
  ]
  for (const [type, payload] of wsCalls) {
    started = performance.now()
    try {
      const reply = await acp.request(type, payload)
      if (!reply.ok) throw new Error(reply.err?.code ?? 'failed')
      if (type === 'list_agents') {
        liveAgentIds = asStringList(reply.payload)
      }
      const extra =
        type === 'list_agents' && Array.isArray(reply.payload)
          ? ` ${reply.payload.length} live`
          : type === 'list_acp_catalog'
            ? ` ${((reply.payload as AcpCatalog | undefined)?.agents ?? []).length} agents`
            : type === 'get_conversation_binding'
              ? (reply.payload as ConversationBindingSnapshot | undefined)?.binding?.agentSessionId
                ? ' has current ACP session'
                : ' no current ACP binding'
              : ''
      steps.push(passStep(`ws.${type}`, extra.trim() || type, started))
    } catch (error) {
      steps.push(
        failStep(`ws.${type}`, error instanceof Error ? error.message : String(error), started)
      )
    }
  }

  const sessionId = binding?.agentSessionId
  if (!sessionId) {
    steps.push(skipStep('ws.cursor', 'no current ACP session'))
    steps.push(skipStep('ws.history', 'no current ACP session'))
    steps.push(skipStep('ws.subscribe', 'no current ACP session'))
  } else {
    started = performance.now()
    try {
      const cursor = await acp.request('get_session_cursor', { sessionId })
      if (!cursor.ok) throw new Error(cursor.err?.code ?? 'failed')
      const watermark = (cursor.payload as SessionCursor | undefined)?.watermark ?? 0
      steps.push(passStep('ws.cursor', `watermark=${watermark}`, started))

      const afterSeq = historyAfterSeq(watermark) ?? Math.max(0, watermark)
      started = performance.now()
      const page = await acp.request('get_session_payload_page', {
        sessionId,
        afterSeq,
        limit: HISTORY_PAGE_LIMIT,
        ...(watermark > 0 ? { targetLastSeq: watermark } : {})
      })
      if (!page.ok) throw new Error(page.err?.code ?? 'failed')
      const records = (page.payload as HistoryPage | undefined)?.records ?? []
      if (records.some((record) => record.seq <= afterSeq)) {
        throw new Error('page included records at or before afterSeq')
      }
      if (records.length > HISTORY_PAGE_LIMIT) {
        throw new Error(`page loaded ${records.length} records`)
      }
      steps.push(passStep('ws.history', `afterSeq=${afterSeq} records=${records.length}`, started))

      started = performance.now()
      const subscribed = await acp.request('subscribe', { sessionId, lastSeq: watermark })
      if (!subscribed.ok) throw new Error(subscribed.err?.code ?? 'failed')
      steps.push(passStep('ws.subscribe', `lastSeq=${watermark}`, started))
    } catch (error) {
      steps.push(
        failStep('ws.session', error instanceof Error ? error.message : String(error), started)
      )
    }
  }

  if (options.interactive !== false) {
    await runInteractiveSteps(steps, acp, {
      origin,
      token: auth.token,
      conversation,
      binding,
      project,
      liveAgentIds
    })
  } else {
    steps.push(skipStep('ws.resume-history', '--read-only'))
    steps.push(skipStep('ws.create-session', '--read-only'))
    steps.push(skipStep('ws.send-accepted', '--read-only'))
    steps.push(skipStep('http.conversations-sync', '--read-only'))
  }

  acp.close()

  const terminal = new HostSocket()
  started = performance.now()
  try {
    await terminal.connect(origin, auth.token, '/terminal/ws')
    steps.push(passStep('terminal.authenticate', 'GET /terminal/ws', started))
    started = performance.now()
    const listed = await terminal.request('list', {
      ...(conversation ? { conversationId: conversation.conversationId } : {}),
      ...(project && !conversation ? { projectId: project.id } : {})
    })
    if (!listed.ok) throw new Error(listed.err?.code ?? 'failed')
    const count = ((listed.payload as { terminals?: unknown[] } | undefined)?.terminals ?? [])
      .length
    steps.push(passStep('terminal.list', `${count} live PTYs`, started))
    if (project) {
      started = performance.now()
      const projectListed = await terminal.request('list', { projectId: project.id })
      if (!projectListed.ok) throw new Error(projectListed.err?.code ?? 'failed')
      const projectCount = (
        (projectListed.payload as { terminals?: unknown[] } | undefined)?.terminals ?? []
      ).length
      steps.push(passStep('terminal.list-project', `${projectCount} project PTYs`, started))
    } else {
      steps.push(skipStep('terminal.list-project', 'no project to list'))
    }
  } catch (error) {
    steps.push(
      failStep('terminal', error instanceof Error ? error.message : String(error), started)
    )
  }
  terminal.close()
  return steps
}

function printReport(steps: ProbeStep[], json: boolean): number {
  const failed = steps.filter((step) => step.status === 'fail').length
  const passed = steps.filter((step) => step.status === 'pass').length
  const skipped = steps.filter((step) => step.status === 'skip').length
  if (json) {
    console.log(JSON.stringify({ passed, failed, skipped, steps }, null, 2))
  } else {
    for (const step of steps) {
      const tag = step.status.toUpperCase().padEnd(4)
      console.log(`${tag} ${step.id} (${step.ms}ms) ${step.detail}`)
    }
    console.log(`${passed} passed, ${failed} failed, ${skipped} skipped`)
  }
  return failed === 0 ? 0 : 1
}

function parseArgs(argv: string[]): {
  origin?: string
  token?: string
  json: boolean
  waitMs: number
  interactive: boolean
} {
  const out: {
    origin?: string
    token?: string
    json: boolean
    waitMs: number
    interactive: boolean
  } = {
    json: false,
    waitMs: 0,
    interactive: true
  }
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i]
    const next = argv[i + 1]
    if (arg === '--json') out.json = true
    if (arg === '--read-only') out.interactive = false
    if (arg === '--origin' && next) {
      out.origin = next
      i += 1
    }
    if (arg === '--token' && next) {
      out.token = next
      i += 1
    }
    if (arg === '--wait' && next) {
      out.waitMs = Math.max(0, Number(next) * 1000)
      i += 1
    }
  }
  return out
}

export async function main(argv = process.argv.slice(2)): Promise<number> {
  const options = parseArgs(argv)
  const steps = await runMobileHostProbe(options)
  return printReport(steps, options.json)
}

if (import.meta.main) {
  process.exit(await main())
}
