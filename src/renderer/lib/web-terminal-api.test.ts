import {
  WEB_TERMINAL_BINARY_KIND,
  WEB_TERMINAL_BINARY_PROTOCOL
} from '@shared/types/web-terminal-protocol.types'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { resolveTerminalWsUrl, WebTerminalClient } from './web-terminal-api'

vi.mock('@/lib/log-api', () => ({
  logFrontendError: vi.fn()
}))

vi.mock('@/lib/acp-transport', () => ({
  getRemoteAccessCredential: () => 'test-access-token'
}))

/**
 * Minimal FakeWebSocket for the terminal protocol (`{id,type,payload}` requests
 * → `{id,success,data}` / `{id,success:false,error,code}` replies). Mirrors the
 * FakeWebSocket shape in `acp-transport.test.ts`: auto-opens on construction so
 * `connect()` resolves, records sent frames, and can be driven to fail attach.
 */
class FakeWebSocket {
  static OPEN = 1
  static CONNECTING = 0
  static CLOSING = 2
  static CLOSED = 3

  readyState = FakeWebSocket.CONNECTING
  onopen: ((ev: Event) => void) | null = null
  onmessage: ((ev: MessageEvent) => void) | null = null
  onerror: ((ev: Event) => void) | null = null
  onclose: ((ev: CloseEvent) => void) | null = null
  sent: string[] = []
  binaryType: BinaryType = 'blob'

  constructor(
    public url: string,
    public protocols?: string | string[]
  ) {
    queueMicrotask(() => {
      this.readyState = FakeWebSocket.OPEN
      this.onopen?.(new Event('open'))
    })
  }

  send(data: string): void {
    this.sent.push(data)
    const req = JSON.parse(data) as { id: string; type: string; payload: Record<string, unknown> }
    if (req.type === 'spawn') {
      if (spawnReply === 'compound-failure') {
        this.emitReply({
          id: req.id,
          success: false,
          error: JSON.stringify({
            terminalId: 'terminal-recoverable-1',
            primaryCode: 'CONVERSATION_DURABILITY_FAILED',
            cleanupStage: 'kill'
          }),
          code: 'TERMINAL_RESOURCE_ROLLBACK_FAILED'
        })
        return
      }
      // CAP-3: spawn is the only issuance path — the reply carries the claim.
      this.emitReply({ id: req.id, success: true, data: spawnReplyData })
      return
    }
    if (req.type === 'resume') {
      if (resumeReply === 'unauthorized') {
        this.emitReply({
          id: req.id,
          success: false,
          error: 'Unauthorized',
          code: 'UNAUTHORIZED'
        })
        return
      }
      queueMicrotask(() => {
        if (resumeReply === 'ok') {
          this.emit({
            type: 'replay',
            terminalId: req.payload.terminalId,
            chunks: resumeReplayChunks,
            gap: false,
            latestSeq: resumeGrantData.terminal.latestSeq,
            snapshot: {
              cwd: resumeGrantData.terminal.cwd,
              gitBranch: null,
              gitStatus: null,
              exitCode: null,
              exited: false
            }
          })
        }
        this.emit({
          id: req.id,
          success: true,
          data:
            resumeReply === 'invalid'
              ? { ...resumeGrantData, terminal: { ...resumeGrantData.terminal, id: 'wrong-id' } }
              : resumeGrantData
        })
      })
      return
    }
    if (req.type === 'attach') {
      if (attachReply === 'unauthorized') {
        // The single generic rejection — no distinguishing detail. The real
        // host returns this for unknown terminal AND bad/rotated/revoked claim
        // alike (existence is never revealed).
        this.emitReply({
          id: req.id,
          success: false,
          error: 'Unauthorized',
          code: 'UNAUTHORIZED'
        })
        return
      }
      this.emitReply({
        id: req.id,
        success: true,
        data: {
          id: req.payload.terminalId,
          shell: 'bash',
          cwd: '/tmp',
          pid: 1,
          cols: 80,
          rows: 24,
          latestSeq: (req.payload.lastSeq as number) ?? 0,
          gap: false
        }
      })
      return
    }
    if (req.type === 'rotate_claim') {
      this.emitReply({ id: req.id, success: true, data: { claim: rotateReplyClaim } })
      return
    }
    if (req.type === 'list') {
      this.emitReply({
        id: req.id,
        success: true,
        data: {
          terminals: [
            {
              id: 'pty-desktop-1',
              shell: 'zsh',
              cwd: '/tmp/termul',
              pid: 9,
              cols: 80,
              rows: 24,
              projectId: req.payload.projectId,
              title: 'termul',
              gitBranch: 'dev'
            }
          ]
        }
      })
      return
    }
    if (req.type === 'watch') {
      this.emitReply({
        id: req.id,
        success: true,
        data: {
          id: req.payload.terminalId,
          shell: 'zsh',
          cwd: '/tmp/termul',
          pid: 9,
          cols: 80,
          rows: 24,
          latestSeq: (req.payload.lastSeq as number) ?? 0,
          gap: false
        }
      })
      return
    }
    if ((req.type === 'terminate' || req.type === 'kill') && terminateReply === 'cleanup-failure') {
      this.emitReply({
        id: req.id,
        success: false,
        error: JSON.stringify({
          terminalId: req.payload.terminalId,
          primaryCode: 'TERMINATE_FAILED',
          cleanupStage: 'flusher_join'
        }),
        code: 'TERMINATE_FAILED'
      })
      return
    }
    this.emitReply({ id: req.id, success: true, data: undefined })
  }

  close(): void {
    this.readyState = FakeWebSocket.CLOSED
    this.onclose?.(new CloseEvent('close'))
  }

  emit(obj: unknown): void {
    this.onmessage?.(new MessageEvent('message', { data: JSON.stringify(obj) }))
  }

  emitBinary(data: ArrayBuffer): void {
    this.onmessage?.(new MessageEvent('message', { data }))
  }

  emitReply(obj: unknown): void {
    queueMicrotask(() => this.emit(obj))
  }
}

function encodeBinaryFrame(
  kind: number,
  terminalId: string,
  seq: number,
  data: number[]
): ArrayBuffer {
  const terminalIdBytes = new TextEncoder().encode(terminalId)
  const buffer = new ArrayBuffer(15 + terminalIdBytes.length + data.length)
  const bytes = new Uint8Array(buffer)
  bytes.set([0x54, 0x4d, 0x4c, 0x32, kind], 0)
  const view = new DataView(buffer)
  view.setUint16(5, terminalIdBytes.length, false)
  view.setUint32(7, Math.floor(seq / 0x1_0000_0000), false)
  view.setUint32(11, seq >>> 0, false)
  bytes.set(terminalIdBytes, 15)
  bytes.set(data, 15 + terminalIdBytes.length)
  return buffer
}

/** Test knob: make `attach` replies fail with the generic UNAUTHORIZED. */
let attachReply: 'ok' | 'unauthorized' = 'ok'
let spawnReply: 'ok' | 'compound-failure' = 'ok'
let terminateReply: 'ok' | 'cleanup-failure' = 'ok'

/** Test knob: the spawn reply data (CAP-3 issuance carries the claim). */
let spawnReplyData: Record<string, unknown> = {
  id: 'pty-spawn-1',
  shell: 'bash',
  cwd: '/tmp',
  pid: 42,
  cols: 80,
  rows: 24,
  claim: 'issued-claim-64-hex'
}

/** Test knob: credential returned by rotate_claim replies. */
let rotateReplyClaim = 'rotated-claim-64-hex'

/** Test knobs for authenticated cold resume. */
let resumeReply: 'ok' | 'unauthorized' | 'invalid' = 'ok'
let resumeGrantData = {
  terminal: {
    id: 't1',
    shell: 'bash',
    cwd: '/workspace/resumed',
    pid: 77,
    cols: 100,
    rows: 30,
    latestSeq: 12,
    gap: false
  },
  claim: 'resume-claim-rotated'
}
let resumeReplayChunks = [
  { seq: 8, data: [114, 101, 112, 108, 97, 121, 45] },
  { seq: 12, data: [111, 107] }
]

type Tracker = {
  lastSeq: number
  exited: boolean
  refCount: number
  streamAttached: boolean
  claim?: string
  disconnected: boolean
  cleanupOnly: boolean
}

type ClientInternals = {
  socket: FakeWebSocket
  trackers: Map<string, Tracker>
  reconnectAttempt: number
  reconnectTimer: ReturnType<typeof setTimeout> | null
  lastHiddenAt: number | null
  visibilityHandler: (() => void) | null
  focusHandler: (() => void) | null
}

/** Override `document.visibilityState` + dispatch `visibilitychange` (jsdom's
 * default is not reliable for the hidden/visible transitions under test). */
function dispatchVisibility(state: 'visible' | 'hidden'): void {
  Object.defineProperty(document, 'visibilityState', {
    configurable: true,
    value: state
  })
  document.dispatchEvent(new Event('visibilitychange'))
}

/** Restore an own `visibilityState = 'visible'` so later suites read visible. */
function restoreVisibility(): void {
  Object.defineProperty(document, 'visibilityState', {
    configurable: true,
    value: 'visible'
  })
}

afterEach(() => {
  attachReply = 'ok'
  spawnReply = 'ok'
  terminateReply = 'ok'
  resumeReply = 'ok'
  resumeGrantData = {
    terminal: {
      id: 't1',
      shell: 'bash',
      cwd: '/workspace/resumed',
      pid: 77,
      cols: 100,
      rows: 30,
      latestSeq: 12,
      gap: false
    },
    claim: 'resume-claim-rotated'
  }
  resumeReplayChunks = [
    { seq: 8, data: [114, 101, 112, 108, 97, 121, 45] },
    { seq: 12, data: [111, 107] }
  ]
})

/** Find the LAST sent request frame of a given type on a FakeWebSocket. */
function findSentRequest(
  sock: FakeWebSocket,
  type: string
): { id: string; type: string; payload: Record<string, unknown> } | undefined {
  for (const raw of [...sock.sent].reverse()) {
    const parsed = JSON.parse(raw) as {
      id: string
      type: string
      payload: Record<string, unknown>
    }
    if (parsed.type === type) return parsed
  }
  return undefined
}

describe('WebTerminalClient visibility-triggered reconnect (AFK recovery)', () => {
  afterEach(() => {
    restoreVisibility()
    attachReply = 'ok'
    spawnReplyData = {
      id: 'pty-spawn-1',
      shell: 'bash',
      cwd: '/tmp',
      pid: 42,
      cols: 80,
      rows: 24,
      claim: 'issued-claim-64-hex'
    }
    rotateReplyClaim = 'rotated-claim-64-hex'
    vi.useRealTimers()
  })

  it('reconnects + re-attaches trackers with their lastSeq after a long hide', async () => {
    vi.useFakeTimers()
    const client = new WebTerminalClient(
      'ws://test/terminal/ws',
      FakeWebSocket as unknown as typeof WebSocket
    )
    const internals = client as unknown as ClientInternals
    await client.connect()
    // Track a terminal with its lease credential and advance its cursor.
    await client.attach('t1', 'claim-t1')
    const oldSocket = internals.socket
    oldSocket.emit({ type: 'data', terminalId: 't1', seq: 7, data: [65] })
    await Promise.resolve() // flush handleFrame
    expect(internals.trackers.get('t1')?.lastSeq).toBe(7)

    // Long hide (> 30s threshold) → return → proactive force-reconnect.
    dispatchVisibility('hidden')
    await vi.advanceTimersByTimeAsync(31_000)
    dispatchVisibility('visible')
    await Promise.resolve()

    // Advance past the 500ms backoff → connect re-opens + re-attaches.
    await vi.advanceTimersByTimeAsync(600)
    await Promise.resolve()

    expect(internals.socket).not.toBe(oldSocket) // torn down + replaced
    expect(internals.socket.readyState).toBe(FakeWebSocket.OPEN)
    // The new socket re-attached the tracker carrying its stored claim +
    // cursor (CAP-3: reattach requires the lease credential).
    const attachReq = findSentRequest(internals.socket, 'attach')
    expect(attachReq).toBeDefined()
    expect(attachReq?.payload).toEqual({ terminalId: 't1', claim: 'claim-t1', lastSeq: 7 })

    if (internals.reconnectTimer) {
      clearTimeout(internals.reconnectTimer)
      internals.reconnectTimer = null
    }
    client.dispose()
  })

  it('does not double-reconnect when a focus follows visibilitychange', async () => {
    vi.useFakeTimers()
    const client = new WebTerminalClient(
      'ws://test/terminal/ws',
      FakeWebSocket as unknown as typeof WebSocket
    )
    const internals = client as unknown as ClientInternals
    await client.connect()
    await client.attach('t1', 'claim-t1')
    const forceSpy = vi.spyOn(
      client as unknown as { forceReconnect: (reason: string) => void },
      'forceReconnect'
    )

    // Long hide → visible triggers forceReconnect (consumes lastHiddenAt).
    dispatchVisibility('hidden')
    await vi.advanceTimersByTimeAsync(31_000)
    dispatchVisibility('visible')
    await Promise.resolve()
    expect(forceSpy).toHaveBeenCalledTimes(1)
    expect(internals.lastHiddenAt).toBeNull() // consumed

    // A `focus` right after (the fallback path) must NOT trigger a 2nd
    // forceReconnect — lastHiddenAt was consumed. `focus` is window-level.
    window.dispatchEvent(new Event('focus'))
    await Promise.resolve()
    expect(forceSpy).toHaveBeenCalledTimes(1)

    await vi.advanceTimersByTimeAsync(600)
    await Promise.resolve()
    // Exactly one new socket opened (one reconnect, not two).
    expect(internals.socket.readyState).toBe(FakeWebSocket.OPEN)
    expect(findSentRequest(internals.socket, 'attach')).toBeDefined()

    forceSpy.mockRestore()
    if (internals.reconnectTimer) {
      clearTimeout(internals.reconnectTimer)
      internals.reconnectTimer = null
    }
    client.dispose()
  })

  it('drops the claim and marks disconnected when re-attach is rejected (terminal torn down while AFK)', async () => {
    vi.useFakeTimers()
    const client = new WebTerminalClient(
      'ws://test/terminal/ws',
      FakeWebSocket as unknown as typeof WebSocket
    )
    const internals = client as unknown as ClientInternals
    await client.connect()
    await client.attach('t1', 'claim-t1')
    expect(internals.trackers.get('t1')?.exited).toBe(false)

    // The server tore the terminal down during AFK — the reconnect's re-attach
    // now receives the single generic UNAUTHORIZED (the host never distinguishes
    // terminal-gone from credential-gone, so TERMINAL_NOT_FOUND is never sent).
    attachReply = 'unauthorized'

    dispatchVisibility('hidden')
    await vi.advanceTimersByTimeAsync(31_000)
    dispatchVisibility('visible')
    await Promise.resolve()
    await vi.advanceTimersByTimeAsync(600)
    // The rejection reply + the onopen re-attach `.then` settle.
    await vi.advanceTimersByTimeAsync(0)
    await Promise.resolve()

    // CAP-3: the credential is dropped and never re-presented; the tracker is
    // marked disconnected. It is NOT marked exited — the generic rejection gives
    // the client no signal that the PTY is dead vs. the claim merely invalid.
    expect(internals.trackers.get('t1')?.claim).toBeUndefined()
    expect(internals.trackers.get('t1')?.disconnected).toBe(true)
    expect(internals.trackers.get('t1')?.exited).toBe(false)

    if (internals.reconnectTimer) {
      clearTimeout(internals.reconnectTimer)
      internals.reconnectTimer = null
    }
    client.dispose()
  })

  it('resets reconnectAttempt on visibility recovery so AFK never strands the terminal', async () => {
    vi.useFakeTimers()
    const client = new WebTerminalClient(
      'ws://test/terminal/ws',
      FakeWebSocket as unknown as typeof WebSocket
    )
    const internals = client as unknown as ClientInternals
    await client.connect()
    await client.attach('t1', 'claim-t1')
    const oldSocket = internals.socket

    // Simulate prior suspensions having exhausted the backoff ceiling. At MAX,
    // a normal `scheduleReconnect` (e.g. from `onclose`) would no-op.
    internals.reconnectAttempt = 10 // RECONNECT_MAX_ATTEMPTS
    internals.reconnectTimer = null

    // Long hide → return → visibility recovery path.
    dispatchVisibility('hidden')
    await vi.advanceTimersByTimeAsync(31_000)
    dispatchVisibility('visible')
    await Promise.resolve()

    // forceReconnect reset the counter (MAX → 0) and scheduleReconnect then
    // scheduled a fresh reconnect (0 → 1) — at MAX this would have been a no-op.
    expect(internals.reconnectAttempt).toBe(1)
    expect(internals.reconnectTimer).not.toBeNull()

    await vi.advanceTimersByTimeAsync(600)
    await Promise.resolve()

    // A fresh socket re-opened despite the prior exhaustion.
    expect(internals.socket).not.toBe(oldSocket)
    expect(internals.socket.readyState).toBe(FakeWebSocket.OPEN)
    expect(findSentRequest(internals.socket, 'attach')).toBeDefined()

    if (internals.reconnectTimer) {
      clearTimeout(internals.reconnectTimer)
      internals.reconnectTimer = null
    }
    client.dispose()
  })

  it('attaches visibility listeners on first connect and detaches on dispose', async () => {
    vi.useFakeTimers()
    const client = new WebTerminalClient(
      'ws://test/terminal/ws',
      FakeWebSocket as unknown as typeof WebSocket
    )
    const internals = client as unknown as ClientInternals
    expect(internals.visibilityHandler).toBeNull()
    expect(internals.focusHandler).toBeNull()

    await client.connect()
    expect(internals.visibilityHandler).not.toBeNull()
    expect(internals.focusHandler).not.toBeNull()

    client.dispose()
    expect(internals.visibilityHandler).toBeNull()
    expect(internals.focusHandler).toBeNull()
  })

  it('force-reconnects on a short hide when the socket is already down (socketDown branch)', async () => {
    vi.useFakeTimers()
    const client = new WebTerminalClient(
      'ws://test/terminal/ws',
      FakeWebSocket as unknown as typeof WebSocket
    )
    const internals = client as unknown as ClientInternals
    await client.connect()
    await client.attach('t1', 'claim-t1')
    const oldSocket = internals.socket
    // The server tore the socket down during AFK (CLOSED), but the client
    // hasn't received onclose yet (suspended-tab / half-open link).
    oldSocket.readyState = FakeWebSocket.CLOSED

    const forceSpy = vi.spyOn(
      client as unknown as { forceReconnect: (reason: string) => void },
      'forceReconnect'
    )

    // SHORT hide (< 30s threshold) — only the `|| socketDown` clause carries.
    dispatchVisibility('hidden')
    await vi.advanceTimersByTimeAsync(5_000)
    dispatchVisibility('visible')
    await Promise.resolve()
    expect(forceSpy).toHaveBeenCalledTimes(1)

    // Advance past the 500ms backoff → a new socket opens + re-attaches.
    await vi.advanceTimersByTimeAsync(600)
    await Promise.resolve()
    expect(internals.socket).not.toBe(oldSocket)
    expect(internals.socket.readyState).toBe(FakeWebSocket.OPEN)
    expect(findSentRequest(internals.socket, 'attach')).toBeDefined()

    forceSpy.mockRestore()
    if (internals.reconnectTimer) {
      clearTimeout(internals.reconnectTimer)
      internals.reconnectTimer = null
    }
    client.dispose()
  })
})

describe('resolveTerminalWsUrl', () => {
  // Pure mapping — no socket involved.
  it('maps https→wss and http→ws and appends /terminal/ws', () => {
    expect(resolveTerminalWsUrl({ protocol: 'https:', host: 'app.example.com' })).toBe(
      'wss://app.example.com/terminal/ws'
    )
    expect(resolveTerminalWsUrl({ protocol: 'http:', host: 'localhost:8080' })).toBe(
      'ws://localhost:8080/terminal/ws'
    )
  })
})

describe('WebTerminalClient frame handling & request lifecycle', () => {
  afterEach(() => {
    vi.useRealTimers()
    attachReply = 'ok'
  })

  it('sends authenticate after the socket opens and before other requests', async () => {
    const client = new WebTerminalClient(
      'ws://test/terminal/ws',
      FakeWebSocket as unknown as typeof WebSocket
    )
    const internals = client as unknown as ClientInternals
    await client.connect()
    const first = JSON.parse(internals.socket.sent[0] ?? '{}') as {
      type?: string
      payload?: { token?: string }
    }
    expect(first.type).toBe('authenticate')
    expect(first.payload?.token).toBe('test-access-token')
    client.dispose()
  })

  it('delivers a data frame as a Uint8Array to onData subscribers', async () => {
    vi.useFakeTimers()
    const client = new WebTerminalClient(
      'ws://test/terminal/ws',
      FakeWebSocket as unknown as typeof WebSocket
    )
    const internals = client as unknown as ClientInternals
    const received: Array<{ terminalId: string; bytes: Uint8Array }> = []
    const off = client.onData((terminalId, bytes) => {
      received.push({ terminalId, bytes })
    })

    await client.connect()
    const sock = internals.socket
    sock.emit({ type: 'data', terminalId: 't1', seq: 1, data: [72, 101, 108, 108, 111] })

    expect(received).toHaveLength(1)
    expect(received[0].terminalId).toBe('t1')
    expect(received[0].bytes).toBeInstanceOf(Uint8Array)
    expect(Array.from(received[0].bytes)).toEqual([72, 101, 108, 108, 111])

    off()
    client.dispose()
  })

  it('delivers data only to scoped subscribers for the matching terminal', async () => {
    vi.useFakeTimers()
    const client = new WebTerminalClient(
      'ws://test/terminal/ws',
      FakeWebSocket as unknown as typeof WebSocket
    )
    const internals = client as unknown as ClientInternals
    const matching = vi.fn()
    const unrelated = vi.fn()
    const offMatching = client.onDataForTerminal('t1', matching)
    const offUnrelated = client.onDataForTerminal('t2', unrelated)

    await client.connect()
    internals.socket.emit({ type: 'data', terminalId: 't1', seq: 1, data: [1, 2, 3] })

    expect(matching).toHaveBeenCalledTimes(1)
    expect(Array.from(matching.mock.calls[0][0] as Uint8Array)).toEqual([1, 2, 3])
    expect(unrelated).not.toHaveBeenCalled()

    offMatching()
    offUnrelated()
    client.dispose()
  })

  it('negotiates and decodes binary terminal output without JSON byte arrays', async () => {
    vi.useFakeTimers()
    const client = new WebTerminalClient(
      'ws://test/terminal/ws',
      FakeWebSocket as unknown as typeof WebSocket
    )
    const internals = client as unknown as ClientInternals
    const matching = vi.fn()
    const unrelated = vi.fn()
    client.onDataForTerminal('t1', matching)
    client.onDataForTerminal('t2', unrelated)

    await client.connect()
    expect(internals.socket.protocols).toBe(WEB_TERMINAL_BINARY_PROTOCOL)
    expect(internals.socket.binaryType).toBe('arraybuffer')

    internals.socket.emitBinary(
      encodeBinaryFrame(WEB_TERMINAL_BINARY_KIND.LIVE, 't1', 4_294_967_299, [0, 0xff, 0x41])
    )

    expect(matching).toHaveBeenCalledTimes(1)
    expect(Array.from(matching.mock.calls[0][0] as Uint8Array)).toEqual([0, 0xff, 0x41])
    expect(unrelated).not.toHaveBeenCalled()
    expect(internals.trackers.get('t1')?.lastSeq).toBe(4_294_967_299)

    client.dispose()
  })

  it('ignores malformed binary terminal output frames', async () => {
    vi.useFakeTimers()
    const client = new WebTerminalClient(
      'ws://test/terminal/ws',
      FakeWebSocket as unknown as typeof WebSocket
    )
    const internals = client as unknown as ClientInternals
    const received = vi.fn()
    client.onData(received)

    await client.connect()
    internals.socket.emitBinary(new Uint8Array([1, 2, 3]).buffer)

    expect(received).not.toHaveBeenCalled()
    client.dispose()
  })

  it('preserves UTF-8 CJK, emoji, and combining bytes in binary replay frames', async () => {
    vi.useFakeTimers()
    const client = new WebTerminalClient(
      'ws://test/terminal/ws',
      FakeWebSocket as unknown as typeof WebSocket
    )
    const internals = client as unknown as ClientInternals
    const received = vi.fn()
    client.onDataForTerminal('unicode', received)
    const encoded = new TextEncoder().encode('中文 👩🏽‍💻 e\u0301')

    await client.connect()
    internals.socket.emitBinary(
      encodeBinaryFrame(WEB_TERMINAL_BINARY_KIND.REPLAY, 'unicode', 7, Array.from(encoded))
    )

    expect(Array.from(received.mock.calls[0][0] as Uint8Array)).toEqual(Array.from(encoded))
    client.dispose()
  })

  it('resolves a request with the matching reply data (round-trip)', async () => {
    vi.useFakeTimers()
    const client = new WebTerminalClient(
      'ws://test/terminal/ws',
      FakeWebSocket as unknown as typeof WebSocket
    )
    const internals = client as unknown as ClientInternals
    await client.connect()
    const sock = internals.socket

    // Stub send so it records the frame WITHOUT auto-replying — we drive the
    // reply manually to assert data round-trips.
    const sendStub = vi.spyOn(sock, 'send').mockImplementation((data: string) => {
      sock.sent.push(data)
    })

    const resultPromise = client.request<{ branch: string }>('get_git_branch', {
      terminalId: 't1'
    })
    // Flush past `await this.connect()` inside request() so the (stubbed) send runs.
    await vi.advanceTimersByTimeAsync(0)

    const sent = findSentRequest(sock, 'get_git_branch')
    expect(sent).toBeDefined()
    sock.emit({ id: sent!.id, success: true, data: { branch: 'main' } })
    sendStub.mockRestore()

    const result = await resultPromise
    expect(result.success).toBe(true)
    if (result.success) {
      expect(result.data).toEqual({ branch: 'main' })
    }

    client.dispose()
  })

  describe('CAP-3 claim lifecycle (issuance, adoption, rejection)', () => {
    function makeClient(): {
      client: WebTerminalClient
      internals: ClientInternals
    } {
      const client = new WebTerminalClient(
        'ws://test/terminal/ws',
        FakeWebSocket as unknown as typeof WebSocket
      )
      return { client, internals: client as unknown as ClientInternals }
    }

    afterEach(() => {
      attachReply = 'ok'
      vi.useRealTimers()
    })

    it('spawn reply carries the issued claim (round-trip shape)', async () => {
      vi.useFakeTimers()
      const { client } = makeClient()
      const result = await client.request<{ id: string; claim: string }>('spawn', {
        projectId: 'p1'
      })
      expect(result.success).toBe(true)
      if (result.success) {
        expect(result.data.id).toBe('pty-spawn-1')
        expect(result.data.claim).toBe('issued-claim-64-hex')
      }
      client.dispose()
    })

    it('resumes with the exact scoped payload, adopts the rotated grant, and delivers replay without spawning', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      const received: number[] = []
      const off = client.onData((terminalId, bytes) => {
        expect(terminalId).toBe('t1')
        received.push(...bytes)
      })

      const result = await client.resume({
        conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
        terminalId: 't1',
        lastSeq: 7
      })

      expect(result).toEqual({ success: true, data: resumeGrantData })
      const resumeRequest = findSentRequest(internals.socket, 'resume')
      expect(resumeRequest?.payload).toEqual({
        conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
        terminalId: 't1',
        lastSeq: 7
      })
      expect(findSentRequest(internals.socket, 'spawn')).toBeUndefined()
      expect(findSentRequest(internals.socket, 'attach')).toBeUndefined()
      expect(new TextDecoder().decode(Uint8Array.from(received))).toBe('replay-ok')
      expect(internals.trackers.get('t1')).toMatchObject({
        claim: 'resume-claim-rotated',
        lastSeq: 12,
        refCount: 0,
        streamAttached: true,
        disconnected: false,
        exited: false
      })

      off()
      client.dispose()
    })

    it('collapses a denied resume to generic UNAUTHORIZED and drops the stale tracker grant', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      await client.connect()
      await client.attach('t1', 'old-claim')
      resumeReply = 'unauthorized'

      const result = await client.resume({
        conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
        terminalId: 't1',
        lastSeq: 5
      })

      expect(result).toEqual({ success: false, error: 'Unauthorized', code: 'UNAUTHORIZED' })
      expect(internals.trackers.get('t1')).toMatchObject({
        claim: undefined,
        refCount: 0,
        streamAttached: false,
        disconnected: true,
        exited: false
      })
      expect(findSentRequest(internals.socket, 'resume')?.payload).toEqual({
        conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
        terminalId: 't1',
        lastSeq: 5
      })
      expect(findSentRequest(internals.socket, 'spawn')).toBeUndefined()

      client.dispose()
    })

    it('rejects a stolen predecessor without erasing the newer resume grant', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      await client.resume({
        conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
        terminalId: 't1',
        lastSeq: 0
      })
      attachReply = 'unauthorized'

      const rejected = await client.attachWithCursor('t1', 'stolen-pre-resume-claim', 12)

      expect(rejected).toEqual({ success: false, error: 'Unauthorized', code: 'UNAUTHORIZED' })
      expect(internals.trackers.get('t1')).toMatchObject({
        claim: 'resume-claim-rotated',
        lastSeq: 12,
        refCount: 0,
        streamAttached: true,
        disconnected: false
      })
      expect(findSentRequest(internals.socket, 'spawn')).toBeUndefined()

      client.dispose()
    })

    it('reconnects a mounted resumed terminal from its replay cursor without spawning', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      await client.resume({
        conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
        terminalId: 't1',
        lastSeq: 0
      })
      const firstSocket = internals.socket

      const mounted = await client.attach('t1')
      expect(mounted.success).toBe(true)
      expect(internals.trackers.get('t1')?.refCount).toBe(1)
      expect(findSentRequest(firstSocket, 'attach')).toBeUndefined()

      firstSocket.close()
      await vi.advanceTimersByTimeAsync(600)
      await Promise.resolve()

      expect(internals.socket).not.toBe(firstSocket)
      expect(findSentRequest(internals.socket, 'attach')?.payload).toEqual({
        terminalId: 't1',
        claim: 'resume-claim-rotated',
        lastSeq: 12
      })
      expect(findSentRequest(internals.socket, 'spawn')).toBeUndefined()

      if (internals.reconnectTimer) {
        clearTimeout(internals.reconnectTimer)
        internals.reconnectTimer = null
      }
      client.dispose()
    })

    it('fails closed on a mismatched resume grant without adopting it', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      resumeReply = 'invalid'

      const result = await client.resume({
        conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
        terminalId: 't1',
        lastSeq: 0
      })

      expect(result.success).toBe(false)
      if (!result.success) expect(result.code).toBe('NETWORK_ERROR')
      expect(internals.trackers.get('t1')).toMatchObject({
        claim: undefined,
        refCount: 0,
        streamAttached: false,
        disconnected: true
      })
      expect(findSentRequest(internals.socket, 'spawn')).toBeUndefined()

      client.dispose()
    })

    it('attaches with claim + lastSeq and adopts both only on server-confirmed success', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      await client.connect()
      const sock = internals.socket

      const result = await client.attach('t1', 'lease-abc')
      expect(result.success).toBe(true)
      const tracker = internals.trackers.get('t1')
      expect(tracker?.claim).toBe('lease-abc')
      expect(tracker?.refCount).toBe(1)
      expect(tracker?.disconnected).toBe(false)

      const attachReq = findSentRequest(sock, 'attach')
      expect(attachReq?.payload).toEqual({ terminalId: 't1', claim: 'lease-abc', lastSeq: 0 })
      client.dispose()
    })

    it('rejects an id-only attach locally (no round trip) and marks disconnected', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      await client.connect()
      const sock = internals.socket

      const result = await client.attach('t2')
      expect(result.success).toBe(false)
      if (!result.success) expect(result.code).toBe('UNAUTHORIZED')
      expect(internals.trackers.get('t2')?.disconnected).toBe(true)
      // No attach frame was presented to the server.
      expect(findSentRequest(sock, 'attach')).toBeUndefined()
      client.dispose()
    })

    it('rejection drops the adopted claim and never re-presents it on reconnect', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      await client.connect()
      attachReply = 'unauthorized'

      const result = await client.attach('t1', 'stolen-or-rotated-claim')
      expect(result.success).toBe(false)
      if (!result.success) expect(result.code).toBe('UNAUTHORIZED')
      expect(internals.trackers.get('t1')?.claim).toBeUndefined()
      expect(internals.trackers.get('t1')?.disconnected).toBe(true)
      expect(internals.trackers.get('t1')?.refCount).toBe(0)

      // Reconnect: the rejected credential must NOT be re-presented — a
      // disconnected terminal does not drive reconnect scheduling at all.
      attachReply = 'ok'
      internals.socket.close()
      await vi.advanceTimersByTimeAsync(600)
      await Promise.resolve()
      expect(internals.socket).toBeNull()
      expect(internals.reconnectTimer).toBeNull()

      client.dispose()
    })

    it('preserves outstanding refCounts across concurrent slow-path attaches', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      await client.connect()

      // Two renderers attach concurrently (both enter the slow path while
      // refCount is still 0). Success must INCREMENT, never reset to 1.
      const [a, b] = await Promise.all([
        client.attach('t1', 'lease-abc'),
        client.attach('t1', 'lease-abc')
      ])
      expect(a.success).toBe(true)
      expect(b.success).toBe(true)
      expect(internals.trackers.get('t1')?.refCount).toBe(2)

      // Fast path increments too.
      const c = await client.attach('t1')
      expect(c.success).toBe(true)
      expect(internals.trackers.get('t1')?.refCount).toBe(3)
      client.dispose()
    })

    it('close-view detaches output while retaining the claim/cursor for explicit reopen', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      await client.connect()
      await client.attach('t1', 'lease-abc')
      internals.socket.emit({ type: 'data', terminalId: 't1', seq: 9, data: [65] })

      const closed = await client.closeView('t1')
      expect(closed.success).toBe(true)

      expect(internals.trackers.get('t1')).toMatchObject({
        claim: 'lease-abc',
        lastSeq: 9,
        refCount: 0,
        disconnected: false
      })
      expect(findSentRequest(internals.socket, 'close_view')?.payload).toEqual({ terminalId: 't1' })

      internals.socket.close()
      await vi.advanceTimersByTimeAsync(600)
      expect(internals.socket).toBeNull()

      const reopened = await client.attach('t1')
      expect(reopened.success).toBe(true)
      expect(internals.trackers.get('t1')?.claim).toBe('lease-abc')
      client.dispose()
    })

    it('terminate is a distinct explicit request; kill is compatibility-only', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      await client.connect()
      await client.attach('t1', 'lease-abc')

      const result = await client.request<void>('terminate', { terminalId: 't1' })
      expect(result.success).toBe(true)
      expect(findSentRequest(internals.socket, 'terminate')?.payload).toEqual({ terminalId: 't1' })
      expect(findSentRequest(internals.socket, 'kill')).toBeUndefined()
      client.removeTracker('t1')
      expect(internals.trackers.has('t1')).toBe(false)
      client.dispose()
    })

    it('preserves cleanup failure detail and retains only the existing id for retry', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      await client.connect()
      await client.attach('t1', 'lease-abc')
      terminateReply = 'cleanup-failure'

      const result = await client.request<void>('terminate', { terminalId: 't1' })

      expect(result).toEqual({
        success: false,
        code: 'TERMINATE_FAILED',
        error: JSON.stringify({
          terminalId: 't1',
          primaryCode: 'TERMINATE_FAILED',
          cleanupStage: 'flusher_join'
        })
      })
      expect(internals.trackers.get('t1')).toMatchObject({
        claim: undefined,
        refCount: 0,
        streamAttached: false,
        disconnected: true,
        cleanupOnly: true,
        exited: false
      })
      expect(findSentRequest(internals.socket, 'spawn')).toBeUndefined()
      expect(findSentRequest(internals.socket, 'attach')?.payload.terminalId).toBe('t1')
      client.dispose()
    })

    it('retries the retained cleanup id once per action and clears only cleanup-only tracking on success', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      await client.connect()
      internals.trackers.set('unrelated-live-terminal', {
        lastSeq: 9,
        exited: false,
        refCount: 0,
        streamAttached: false,
        claim: 'unrelated-memory-claim',
        disconnected: false,
        cleanupOnly: false
      })
      terminateReply = 'cleanup-failure'

      const failed = await client.request<void>('terminate', {
        terminalId: 'terminal-cleanup-retry'
      })
      expect(failed).toEqual({
        success: false,
        code: 'TERMINATE_FAILED',
        error: JSON.stringify({
          terminalId: 'terminal-cleanup-retry',
          primaryCode: 'TERMINATE_FAILED',
          cleanupStage: 'flusher_join'
        })
      })
      expect(internals.trackers.get('terminal-cleanup-retry')).toMatchObject({
        cleanupOnly: true,
        claim: undefined,
        disconnected: true
      })

      terminateReply = 'ok'
      const succeeded = await client.request<void>('terminate', {
        terminalId: 'terminal-cleanup-retry'
      })
      if (succeeded.success) client.removeTracker('terminal-cleanup-retry')

      expect(succeeded).toEqual({ success: true, data: undefined })
      expect(internals.trackers.has('terminal-cleanup-retry')).toBe(false)
      expect(internals.trackers.get('unrelated-live-terminal')).toMatchObject({
        claim: 'unrelated-memory-claim',
        cleanupOnly: false
      })
      const sentTypes = internals.socket.sent.map(
        (raw) => JSON.parse(raw) as { type: string; payload: { terminalId?: string } }
      )
      expect(
        sentTypes.filter(
          (frame) =>
            frame.type === 'terminate' && frame.payload.terminalId === 'terminal-cleanup-retry'
        )
      ).toHaveLength(2)
      expect(sentTypes.some((frame) => frame.type === 'spawn')).toBe(false)
      expect(sentTypes.some((frame) => frame.type === 'attach')).toBe(false)

      client.dispose()
    })

    it('retains compound rollback terminal identity without attach, reconnect, or respawn', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      spawnReply = 'compound-failure'

      const result = await client.request('spawn', {
        conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
        cwdSource: 'workspace',
        cols: 80,
        rows: 24
      })

      expect(result).toEqual({
        success: false,
        code: 'TERMINAL_RESOURCE_ROLLBACK_FAILED',
        error: JSON.stringify({
          terminalId: 'terminal-recoverable-1',
          primaryCode: 'CONVERSATION_DURABILITY_FAILED',
          cleanupStage: 'kill'
        })
      })
      expect(internals.trackers.get('terminal-recoverable-1')).toMatchObject({
        claim: undefined,
        refCount: 0,
        streamAttached: false,
        disconnected: true,
        cleanupOnly: true,
        exited: false
      })
      const sentTypes = internals.socket.sent.map(
        (raw) => (JSON.parse(raw) as { type: string }).type
      )
      expect(sentTypes).toEqual(['authenticate', 'spawn'])
      client.dispose()
    })

    it('reconnect re-attaches terminals with a stored claim only', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      await client.connect()

      // t1 holds a lease; t3 does not (e.g. a cross-client record without a
      // credential).
      await client.attach('t1', 'lease-abc')
      internals.trackers.set('t3', {
        lastSeq: 0,
        exited: false,
        refCount: 1,
        streamAttached: false,
        disconnected: false,
        cleanupOnly: false
      })

      internals.socket.close()
      await vi.advanceTimersByTimeAsync(600)
      await Promise.resolve()

      // Exactly one attach frame — for the credentialed terminal only.
      const attachFrames = internals.socket.sent
        .map((raw) => JSON.parse(raw) as { type: string; payload: { terminalId: string } })
        .filter((f) => f.type === 'attach')
      expect(attachFrames).toHaveLength(1)
      expect(attachFrames[0].payload.terminalId).toBe('t1')
      // The claim-less terminal is marked disconnected.
      expect(internals.trackers.get('t3')?.disconnected).toBe(true)

      if (internals.reconnectTimer) {
        clearTimeout(internals.reconnectTimer)
        internals.reconnectTimer = null
      }
      client.dispose()
    })

    it('rotate adopts the fresh credential and forces a re-verified attach', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      await client.connect()
      await client.attach('t1', 'lease-old')
      expect(internals.trackers.get('t1')?.refCount).toBe(1)

      const rotated = await client.request<{ claim: string }>('rotate_claim', {
        terminalId: 't1',
        claim: 'lease-old'
      })
      expect(rotated.success).toBe(true)
      if (rotated.success) expect(rotated.data.claim).toBe('rotated-claim-64-hex')

      // Facade-level teardown semantics (severClaim): fresh credential held,
      // outstanding refs require a fresh verified attach.
      client.severClaim('t1', rotated.success ? rotated.data.claim : undefined)
      const tracker = internals.trackers.get('t1')
      expect(tracker?.claim).toBe('rotated-claim-64-hex')
      expect(tracker?.refCount).toBe(0)
      expect(tracker?.disconnected).toBe(false)

      // Re-attach with the rotated credential succeeds.
      const reattach = await client.attach('t1', 'rotated-claim-64-hex')
      expect(reattach.success).toBe(true)
      expect(internals.trackers.get('t1')?.refCount).toBe(1)
      client.dispose()
    })

    it('revoke drops the credential and marks the terminal disconnected', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      await client.connect()
      await client.attach('t1', 'lease-old')

      const revoked = await client.request<void>('revoke_claim', {
        terminalId: 't1',
        claim: 'lease-old'
      })
      expect(revoked.success).toBe(true)

      client.severClaim('t1')
      const tracker = internals.trackers.get('t1')
      expect(tracker?.claim).toBeUndefined()
      expect(tracker?.refCount).toBe(0)
      expect(tracker?.disconnected).toBe(true)

      // The revoked terminal no longer drives reconnect scheduling.
      internals.socket.close()
      await vi.advanceTimersByTimeAsync(600)
      expect(internals.reconnectTimer).toBeNull()
      client.dispose()
    })

    it('re-attach with a supplied claim while refs are outstanding increments refCount (never resets)', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      await client.connect()

      // Renderer A attaches with the lease...
      const a = await client.attach('t1', 'lease-abc')
      expect(a.success).toBe(true)
      expect(internals.trackers.get('t1')?.refCount).toBe(1)

      // ...then renderer B re-attaches presenting the same claim while A's
      // ref is outstanding. Success must INCREMENT — a `refCount = 1` reset
      // would discard renderer A's reference and tear its stream down.
      const b = await client.attach('t1', 'lease-abc')
      expect(b.success).toBe(true)
      expect(internals.trackers.get('t1')?.refCount).toBe(2)
      expect(internals.trackers.get('t1')?.claim).toBe('lease-abc')
      expect(internals.trackers.get('t1')?.disconnected).toBe(false)
      client.dispose()
    })

    it('handoff attach with supplied claim + cursor adopts both for reconnect', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      await client.connect()
      const sock = internals.socket

      // Cross-client handoff: the facade attach (attachWithCursor) presents
      // the supplied claim + cursor to the server.
      const result = await client.attachWithCursor('t1', 'handoff-claim', 87)
      expect(result.success).toBe(true)
      expect(findSentRequest(sock, 'attach')?.payload).toEqual({
        terminalId: 't1',
        claim: 'handoff-claim',
        lastSeq: 87
      })

      // Server confirmed → the credential is adopted, terminal attachable.
      const tracker = internals.trackers.get('t1')
      expect(tracker?.claim).toBe('handoff-claim')
      expect(tracker?.refCount).toBe(1)
      expect(tracker?.disconnected).toBe(false)

      // Seq-tagged output delivery (bounded replay / live) advances the cursor.
      sock.emit({ type: 'data', terminalId: 't1', seq: 90, data: [104, 105] })
      expect(internals.trackers.get('t1')?.lastSeq).toBe(90)

      // A reconnect re-presents the adopted claim + cursor.
      sock.close()
      await vi.advanceTimersByTimeAsync(600)
      await Promise.resolve()
      expect(internals.socket).not.toBe(sock)
      expect(findSentRequest(internals.socket, 'attach')?.payload).toEqual({
        terminalId: 't1',
        claim: 'handoff-claim',
        lastSeq: 90
      })

      if (internals.reconnectTimer) {
        clearTimeout(internals.reconnectTimer)
        internals.reconnectTimer = null
      }
      client.dispose()
    })

    it('rejected handoff attach (claim + cursor) drops the adopted claim and never re-presents it', async () => {
      vi.useFakeTimers()
      const { client, internals } = makeClient()
      await client.connect()
      attachReply = 'unauthorized'

      // Facade attach (attachWithCursor) with supplied claim + cursor — the
      // server rejects with the single generic UNAUTHORIZED error.
      const result = await client.attachWithCursor('t1', 'stolen-or-rotated-claim', 87)
      expect(result.success).toBe(false)
      if (!result.success) expect(result.code).toBe('UNAUTHORIZED')

      // No adopted credential survives the rejection.
      const tracker = internals.trackers.get('t1')
      expect(tracker?.claim).toBeUndefined()
      expect(tracker?.disconnected).toBe(true)
      expect(tracker?.refCount).toBe(0)

      // Reconnect: the rejected credential must never be re-presented — a
      // disconnected terminal does not drive reconnect scheduling at all.
      attachReply = 'ok'
      internals.socket.close()
      await vi.advanceTimersByTimeAsync(600)
      await Promise.resolve()
      expect(internals.socket).toBeNull()
      expect(internals.reconnectTimer).toBeNull()

      client.dispose()
    })
  })

  it('rejects a request awaiting connect to NETWORK_ERROR when forceReconnect fires mid-handshake', async () => {
    vi.useFakeTimers()
    const client = new WebTerminalClient(
      'ws://test/terminal/ws',
      FakeWebSocket as unknown as typeof WebSocket
    )
    const internals = client as unknown as ClientInternals

    // Kick off a request — connect() opens a CONNECTING socket; the request
    // awaits the in-flight connect promise (15s timeout not yet armed).
    const reqPromise = client.request('spawn', { rows: 24, cols: 80 })
    // A real browser does NOT fire `onopen` for a socket closed mid-handshake;
    // detach the FakeWebSocket's queued `onopen` to model that (otherwise the
    // double's microtask would unconditionally resolve connect and mask the
    // hang the fix prevents).
    internals.socket.onopen = null
    // Synchronously force-reconnect BEFORE any socket event fires.
    ;(client as unknown as { forceReconnect: (reason: string) => void }).forceReconnect(
      'afk return'
    )

    // Flush: the in-flight connect promise rejects → request() catches →
    // NETWORK_ERROR (does NOT hang until the 15s timeout).
    await vi.advanceTimersByTimeAsync(0)
    const result = await reqPromise

    expect(result.success).toBe(false)
    if (!result.success) {
      expect(result.code).toBe('NETWORK_ERROR')
    }

    if (internals.reconnectTimer) {
      clearTimeout(internals.reconnectTimer)
      internals.reconnectTimer = null
    }
    client.dispose()
  })

  it('lists live host PTYs and watches without a claim', async () => {
    const client = new WebTerminalClient(
      'ws://test/terminal/ws',
      FakeWebSocket as unknown as typeof WebSocket
    )
    const internals = client as unknown as ClientInternals

    const listed = await client.list({ projectId: 'proj-1' })
    expect(listed.success).toBe(true)
    if (listed.success) {
      expect(listed.data.terminals).toHaveLength(1)
      expect(listed.data.terminals[0]).toMatchObject({
        id: 'pty-desktop-1',
        title: 'termul',
        projectId: 'proj-1'
      })
    }
    expect(findSentRequest(internals.socket, 'list')?.payload).toEqual({ projectId: 'proj-1' })

    const watched = await client.watch('pty-desktop-1', 0)
    expect(watched.success).toBe(true)
    expect(findSentRequest(internals.socket, 'watch')?.payload).toEqual({
      terminalId: 'pty-desktop-1',
      lastSeq: 0
    })
    expect(findSentRequest(internals.socket, 'attach')).toBeUndefined()
    expect(internals.trackers.get('pty-desktop-1')?.disconnected).toBe(false)

    client.dispose()
  })
})
