/**
 * Live request test for the iPhone companion chat path.
 * Reads the desktop remote-access token from the OS keychain and never prints it.
 *
 * Usage: bun scripts/companion-chat-request-test.ts
 * Optional: SE_ACCESS_URL=https://host/#access_token=...
 */
import { acceptedBrandValues } from '../src/shared/brand'

const HOST = process.env.SE_HOST ?? 'http://127.0.0.1:18787'

type IpcBody<T> = {
  success: boolean
  data?: T
  error?: string
  code?: string
}

type WsReply = {
  id?: string
  ok?: boolean
  payload?: unknown
  err?: { code?: string; message?: string }
}

function fail(message: string): never {
  console.error(`FAIL ${message}`)
  process.exit(1)
}

function pass(message: string): void {
  console.log(`PASS ${message}`)
}

function tokenFromAccessUrl(url: string): { origin: string; token: string } | null {
  try {
    const parsed = new URL(url)
    const fragment = parsed.hash.startsWith('#') ? parsed.hash.slice(1) : parsed.hash
    const token = new URLSearchParams(fragment).get('access_token')
    if (!token) return null
    parsed.hash = ''
    return { origin: parsed.origin, token }
  } catch {
    return null
  }
}

async function tokenFromKeychain(): Promise<string | null> {
  const accounts = ['remote-access-generation-v2', 'remote-access-v1']
  // Both spellings, current first: this runs against whichever desktop build the
  // developer happens to have launched, and one that has not yet performed a
  // compatibility read still has the token only under the pre-rename service.
  // Sourced from the brand module rather than spelled here, so the name cannot
  // drift away from what the app writes.
  const services = acceptedBrandValues('keychainService')
  for (const service of services) {
    for (const account of accounts) {
      const proc = Bun.spawn({
        cmd: ['security', 'find-generic-password', '-s', service, '-a', account, '-w'],
        stdout: 'pipe',
        stderr: 'pipe'
      })
      const text = (await new Response(proc.stdout).text()).trim()
      const code = await proc.exited
      if (code === 0 && text.length > 0) return text
    }
  }
  return null
}

async function resolveAuth(): Promise<{ origin: string; token: string }> {
  const fromEnv = process.env.SE_ACCESS_URL
  if (fromEnv) {
    const parsed = tokenFromAccessUrl(fromEnv)
    if (parsed) return parsed
  }
  const token = process.env.SE_ACCESS_TOKEN || (await tokenFromKeychain())
  if (!token) {
    fail('no remote-access token in SE_ACCESS_URL / keychain')
  }
  return { origin: HOST, token }
}

async function ipc<T>(origin: string, token: string, path: string): Promise<IpcBody<T>> {
  const response = await fetch(`${origin}${path}`, {
    headers: { authorization: `Bearer ${token}` }
  })
  if (!response.ok) {
    fail(`${path} HTTP ${response.status}`)
  }
  return (await response.json()) as IpcBody<T>
}

function wsUrl(origin: string): string {
  const url = new URL(origin)
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
  url.pathname = '/ws'
  url.hash = ''
  url.search = ''
  return url.toString()
}

class HostSocket {
  private readonly pending = new Map<string, (reply: WsReply) => void>()
  private serial = 0
  private opened!: Promise<void>
  private socket!: WebSocket

  async connect(origin: string, token: string): Promise<void> {
    this.socket = new WebSocket(wsUrl(origin), {
      headers: { authorization: `Bearer ${token}` }
    })
    this.opened = new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('ws timeout')), 8000)
      this.socket.addEventListener('error', () => {
        clearTimeout(timer)
        reject(new Error('ws error'))
      })
      this.socket.addEventListener('open', () => clearTimeout(timer))
      this.socket.addEventListener('message', (event) => {
        const frame = JSON.parse(String(event.data)) as WsReply & { type?: string }
        if (frame.type === 'auth_required') {
          resolve()
          return
        }
        if (frame.id && this.pending.has(frame.id)) {
          this.pending.get(frame.id)!(frame)
          this.pending.delete(frame.id)
        }
      })
    })
    await this.opened
    const auth = await this.request('authenticate', { token })
    if (!auth.ok) fail(`authenticate ${auth.err?.code ?? 'failed'}`)
    pass('ws authenticate')
  }

  request(type: string, payload: Record<string, unknown> = {}): Promise<WsReply> {
    const id = `live-${++this.serial}`
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`${type} timed out`)), 10000)
      this.pending.set(id, (reply) => {
        clearTimeout(timer)
        resolve(reply)
      })
      this.socket.send(JSON.stringify({ id, type, payload }))
    })
  }

  close(): void {
    this.socket.close()
  }
}

async function main(): Promise<void> {
  const health = await fetch(`${HOST}/health`)
  if (!health.ok) fail(`/health HTTP ${health.status}`)
  pass('/health')

  const { origin, token } = await resolveAuth()
  const conversations = await ipc<Array<{ conversationId: string; lastSeq?: number }>>(
    origin,
    token,
    '/conversations'
  )
  if (!conversations.success || !conversations.data?.length) {
    fail('/conversations did not return data')
  }
  pass(`/conversations ${conversations.data.length}`)

  const conversation = conversations.data[0]!
  const binding = await ipc<{
    conversationId: string
    binding: { agentSessionId?: string; runtimeAgentId?: string } | null
  }>(origin, token, `/conversations/${conversation.conversationId}/binding`)
  if (!binding.success) fail(`/conversations/{id}/binding ${binding.code}`)
  pass(
    binding.data?.binding?.agentSessionId
      ? '/conversations/{id}/binding has session'
      : '/conversations/{id}/binding is empty'
  )

  const catalog = await ipc<{
    host: { os?: string; arch?: string }
    agents: Array<{
      id: string
      status?: string
      installed?: unknown
      distribution?: { npx?: unknown }
    }>
  }>(origin, token, '/acp/catalog')
  if (!catalog.success || !catalog.data?.agents.length) fail('/acp/catalog empty')
  const readyWithoutInstalled = catalog.data.agents.filter(
    (agent) => agent.status === 'ready' && agent.installed == null
  )
  if (readyWithoutInstalled.length === 0) {
    fail('no ready catalog agent without installed overlay')
  }
  pass(`/acp/catalog ${readyWithoutInstalled.length} ready without installed`)

  const socket = new HostSocket()
  await socket.connect(origin, token)

  const wsCatalog = await socket.request('list_acp_catalog', {})
  if (!wsCatalog.ok) fail(`list_acp_catalog ${wsCatalog.err?.code}`)
  const wsAgents = (wsCatalog.payload as { agents?: unknown[] } | undefined)?.agents
  if (!Array.isArray(wsAgents) || wsAgents.length === 0) fail('list_acp_catalog empty')
  pass('ws list_acp_catalog')

  const liveAgents = await socket.request('list_agents', {})
  if (!liveAgents.ok) fail(`list_agents ${liveAgents.err?.code}`)
  pass(`ws list_agents ${Array.isArray(liveAgents.payload) ? liveAgents.payload.length : 0}`)

  const sessionId = binding.data?.binding?.agentSessionId
  if (sessionId) {
    const cursor = await socket.request('get_session_cursor', { sessionId })
    if (!cursor.ok) fail(`get_session_cursor ${cursor.err?.code}`)
    const watermark = (cursor.payload as { watermark?: number } | undefined)?.watermark ?? 0
    pass(`ws get_session_cursor watermark=${watermark}`)

    const afterSeq = watermark > 60 ? watermark - 60 : watermark > 3 ? watermark - 3 : 0
    const page = await socket.request('get_session_payload_page', {
      sessionId,
      afterSeq,
      limit: 80,
      ...(watermark > 0 ? { targetLastSeq: watermark } : {})
    })
    if (!page.ok) fail(`get_session_payload_page ${page.err?.code}`)
    const records =
      (page.payload as { records?: Array<{ seq: number }> } | undefined)?.records ?? []
    if (records.some((record) => record.seq <= afterSeq)) {
      fail('history page returned records at or before afterSeq')
    }
    if (watermark > 60 && afterSeq > 0 && records.length > 80) {
      fail(`history page loaded too many records: ${records.length}`)
    }
    pass(`ws get_session_payload_page afterSeq=${afterSeq} records=${records.length}`)
  } else {
    console.log('SKIP get_session_cursor / get_session_payload_page (no current binding)')
  }

  socket.close()
  console.log('OK companion chat request test')
}

await main()
