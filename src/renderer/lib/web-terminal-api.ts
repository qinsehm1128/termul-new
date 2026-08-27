import type {
  GitStatus,
  IpcResult,
  PrimaryTerminalDataHandle,
  RotatedClaim,
  SpawnedTerminal,
  TerminalApi,
  TerminalAttachResult,
  TerminalCwdChangedCallback,
  TerminalDataCallback,
  TerminalDisplayMode,
  TerminalDisplayModeChangedEvent,
  TerminalDisplayModeOptions,
  TerminalDisplayModeState,
  TerminalExitCallback,
  TerminalExitCodeChangedCallback,
  TerminalGitBranchChangedCallback,
  TerminalGitStatusChangedCallback,
  TerminalResumeGrant,
  TerminalResumeRequest,
  TerminalScopedDataCallback,
  TerminalSpawnedEvent,
  TerminalSpawnOptions
} from '@shared/types/ipc.types'
import {
  decodeWebTerminalBinaryFrame,
  readTerminalResourceFailure,
  WEB_TERMINAL_BINARY_PROTOCOL,
  type WebTerminalEventPayload,
  type WebTerminalFrame,
  type WebTerminalListResult,
  type WebTerminalReply,
  type WebTerminalRequestType
} from '@shared/types/web-terminal-protocol.types'
import { getRemoteAccessCredential } from '@/lib/acp-transport'
import { logFrontendError } from '@/lib/log-api'

const REQUEST_TIMEOUT_MS = 15_000
const RECONNECT_BASE_MS = 500
const RECONNECT_MAX_MS = 8_000
const RECONNECT_MAX_ATTEMPTS = 10
/** How long the page must stay hidden before a return triggers a proactive
 * reconnect. Mobile browsers suspend JS in backgrounded tabs, so the server's
 * keepalive tears the terminal WS down at its Pong-timeout — but the client
 * only learns this when `onclose` is finally delivered on resume (late, or
 * never on a half-open link). Mirrors `WsAcpTransport`: 30s sits between the
 * server's Ping interval and its Pong-timeout. Tunable. */
const VISIBILITY_STALE_THRESHOLD_MS = 30_000

export function resolveTerminalWsUrl(
  locationLike: { protocol: string; host: string } = window.location
): string {
  const protocol = locationLike.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${protocol}//${locationLike.host}/terminal/ws`
}

type Pending = {
  resolve: (reply: WebTerminalReply<unknown>) => void
  timer: ReturnType<typeof setTimeout>
}

/** Tracks per-terminal cursor and attachment state. */
interface TerminalTracker {
  /** Last received output sequence number (0 = no output yet). */
  lastSeq: number
  /** Whether the terminal has exited (stop reconnecting). */
  exited: boolean
  /** Active renderer reference count (detach when it reaches 0). */
  refCount: number
  /** Whether this websocket currently has an authorized live output stream. */
  streamAttached: boolean
  /**
   * CAP-3 lease credential for this terminal (in-memory only — never
   * persisted). Adopted ONLY on server-confirmed success (spawn reply or a
   * verified attach/rotate); dropped on any server rejection (the host returns
   * one generic UNAUTHORIZED for unknown terminal and bad/revoked credential
   * alike) — and never re-presented afterwards.
   */
  claim?: string
  /**
   * Terminal is unattachable from this client (no/invalid credential). Disconnected
   * terminals are skipped by the reconnect re-attach loop and do not drive
   * reconnect scheduling.
   */
  disconnected: boolean
  /** Cleanup failed after ownership was established; retain only the stable id for retry. */
  cleanupOnly: boolean
}

export class WebTerminalClient {
  private socket: WebSocket | null = null
  private connecting: Promise<void> | null = null
  /** Reject fn for the in-flight `connect()` promise (executor pattern), so
   * `forceReconnect` can settle it when tearing down a CONNECTING socket —
   * otherwise an awaiting `request()` hangs until its 15s timeout (which only
   * arms AFTER connect resolves). Mirrors WsAcpTransport's teardown approach. */
  private connectingReject: ((error: Error) => void) | null = null
  private disposed = false
  private nextId = 0
  private reconnectAttempt = 0
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  /** When the page became hidden (epoch-ms), or null while visible. Drives the
   * visibility-triggered proactive reconnect on mobile idle/background resume. */
  private lastHiddenAt: number | null = null
  /** Bound DOM-listener refs so `dispose()` can detach them. */
  private visibilityHandler: (() => void) | null = null
  private focusHandler: (() => void) | null = null
  private readonly pending = new Map<string, Pending>()
  private readonly trackers = new Map<string, TerminalTracker>()
  // Mirrors the Tauri transport: one live writer per PTY held in a Map so a
  // second concurrent writer is structurally impossible, plus read-only
  // sidecars kept in a separate container so the roles cannot be confused.
  private readonly primaryDataHandlers = new Map<string, TerminalScopedDataCallback>()
  private readonly dataSidecars = new Set<TerminalDataCallback>()
  private readonly exitCallbacks = new Set<TerminalExitCallback>()
  private readonly cwdCallbacks = new Set<TerminalCwdChangedCallback>()
  private readonly branchCallbacks = new Set<TerminalGitBranchChangedCallback>()
  private readonly statusCallbacks = new Set<TerminalGitStatusChangedCallback>()
  private readonly exitCodeCallbacks = new Set<TerminalExitCodeChangedCallback>()
  private readonly spawnedCallbacks = new Set<(event: TerminalSpawnedEvent) => void>()
  private readonly displayModeCallbacks = new Set<
    (event: TerminalDisplayModeChangedEvent) => void
  >()
  private invalidBinaryFrameLogged = false

  constructor(
    private readonly url = resolveTerminalWsUrl(),
    private readonly WebSocketImpl: typeof WebSocket = WebSocket
  ) {}

  async request<T>(
    type: WebTerminalRequestType,
    payload: Record<string, unknown>
  ): Promise<IpcResult<T>> {
    try {
      await this.connect()
    } catch (error) {
      return failure('NETWORK_ERROR', error)
    }
    const socket = this.socket
    if (!socket || socket.readyState !== this.WebSocketImpl.OPEN) {
      return failure('NETWORK_ERROR', 'Terminal websocket is not open')
    }
    const id = `terminal-${++this.nextId}`
    return new Promise<IpcResult<T>>((resolve) => {
      const timer = setTimeout(() => {
        this.pending.delete(id)
        resolve(failure('NETWORK_ERROR', `Terminal request ${type} timed out`))
      }, REQUEST_TIMEOUT_MS)
      this.pending.set(id, {
        timer,
        resolve: (reply) => {
          const result: IpcResult<T> = reply.success
            ? { success: true, data: reply.data as T }
            : { success: false, error: reply.error, code: reply.code }
          this.retainTerminalResourceFailure(result)
          resolve(result)
        }
      })
      socket.send(JSON.stringify({ id, type, payload }))
    })
  }

  connect(): Promise<void> {
    if (this.disposed) return Promise.reject(new Error('Terminal client disposed'))
    this.attachVisibilityListeners()
    if (this.socket?.readyState === this.WebSocketImpl.OPEN) return Promise.resolve()
    if (this.connecting) return this.connecting
    this.connecting = new Promise<void>((resolve, reject) => {
      const socket = new this.WebSocketImpl(this.url, WEB_TERMINAL_BINARY_PROTOCOL)
      socket.binaryType = 'arraybuffer'
      this.socket = socket
      this.connectingReject = reject
      socket.onopen = () => {
        const authId = `terminal-auth-${++this.nextId}`
        const timer = setTimeout(() => {
          this.pending.delete(authId)
          this.connecting = null
          this.connectingReject = null
          reject(new Error('Terminal authenticate timed out'))
        }, REQUEST_TIMEOUT_MS)
        this.pending.set(authId, {
          timer,
          resolve: (reply) => {
            this.reconnectAttempt = 0
            this.connecting = null
            this.connectingReject = null
            if (!reply.success) {
              reject(new Error(reply.error || 'Terminal authenticate failed'))
              return
            }
            this.reattachTrackedTerminals()
            resolve()
          }
        })
        socket.send(
          JSON.stringify({
            id: authId,
            type: 'authenticate',
            payload: { token: getRemoteAccessCredential() }
          })
        )
      }
      socket.onmessage = (event) => this.handleIncomingFrame(event.data)
      socket.onerror = () => {
        this.connecting = null
        this.connectingReject = null
        reject(new Error('Terminal websocket connection failed'))
      }
      socket.onclose = () => {
        this.connecting = null
        this.connectingReject = null
        this.socket = null
        for (const tracker of this.trackers.values()) tracker.streamAttached = false
        this.rejectPending()
        this.scheduleReconnect()
      }
    })
    return this.connecting
  }

  private reattachTrackedTerminals(): void {
    // CAP-3: re-attach ONLY terminals with a stored lease credential,
    // using their lastSeq cursor. Terminals without a claim cannot be
    // re-attached — mark them disconnected (no credential is ever
    // presented id-only, and a rejected credential is never re-presented).
    for (const [terminalId, tracker] of this.trackers) {
      if (tracker.exited || tracker.refCount <= 0 || tracker.cleanupOnly) continue
      if (!tracker.claim) {
        tracker.disconnected = true
        continue
      }
      // CAP-3: capture the credential this re-attach is presenting. A
      // rotate (`severClaim`) that completes while this request is in
      // flight installs a FRESH claim; the in-flight attach then resolves
      // with the generic UNAUTHORIZED for the OLD claim. Clearing
      // unconditionally would discard the fresh claim and strand the
      // terminal (valid lease held but unattachable). Only clear when the
      // tracker still holds the SAME credential this attach presented.
      const presentedClaim = tracker.claim
      void this.request('attach', {
        terminalId,
        claim: tracker.claim,
        lastSeq: tracker.lastSeq
      }).then((r) => {
        if (r.success) {
          tracker.disconnected = false
          tracker.streamAttached = true
          return
        }
        if (r.code !== 'NETWORK_ERROR' && tracker.claim === presentedClaim) {
          // Server rejection (single generic UNAUTHORIZED — the host never
          // distinguishes terminal-gone from credential-gone): the lease is
          // invalid/rotated/revoked or the terminal no longer exists. Drop
          // the credential and stop re-presenting it — but ONLY when a
          // newer claim has not superseded it in the meantime.
          tracker.claim = undefined
          tracker.disconnected = true
          tracker.streamAttached = false
        }
        // NETWORK_ERROR keeps the claim for the next reconnect attempt.
      })
    }
  }

  /**
   * CAP-3 verified attach (always a server round trip). The credential is the
   * gate:
   * - with no credential available, fail locally and mark the terminal
   *   disconnected — an id-only attach is never presented;
   * - the claim and cursor are adopted ONLY on server-confirmed success;
   * - on server rejection the adopted claim is dropped and the terminal is
   *   marked disconnected (never re-present a rejected credential);
   * - refCount is only ever incremented on success — a concurrent slow-path
   *   attach must not reset an outstanding refCount.
   */
  private async performAttach(
    terminalId: string,
    claim: string | undefined,
    lastSeq: number | undefined,
    countRendererRef = true
  ): Promise<IpcResult<TerminalAttachResult>> {
    const tracker = this.getOrCreate(terminalId)
    const credential = claim ?? tracker.claim
    if (!credential) {
      tracker.disconnected = true
      return { success: false, error: 'Unauthorized', code: 'UNAUTHORIZED' }
    }
    // Snapshot the claim held at request time. A rotate or resume that
    // completes while this request is in flight installs a fresh credential;
    // the in-flight attach can then reject the predecessor. Clear state only
    // when the rejected credential is still active (or was never adopted), so
    // an explicit stale credential cannot erase a newer grant.
    const claimAtRequest = tracker.claim
    const result = await this.request<TerminalAttachResult>('attach', {
      terminalId,
      claim: credential,
      lastSeq: lastSeq ?? tracker.lastSeq
    })
    if (result.success) {
      // Increment — never `= 1`: in-flight attaches must not discard refs.
      if (countRendererRef) tracker.refCount += 1
      tracker.claim = credential
      tracker.lastSeq = Math.max(tracker.lastSeq, result.data.latestSeq)
      tracker.disconnected = false
      tracker.streamAttached = true
      tracker.cleanupOnly = false
    } else if (result.code !== 'NETWORK_ERROR') {
      // Server rejection (generic UNAUTHORIZED): drop the adopted claim and
      // stop re-presenting it on reconnect only when the rejected credential
      // is still the active one. An explicit stale credential must not erase a
      // newer resume grant already installed in this tracker.
      const rejectedActiveClaim = tracker.claim === credential
      const rejectedUnadoptedClaim = tracker.claim === undefined && claimAtRequest === undefined
      if (rejectedActiveClaim || rejectedUnadoptedClaim) {
        tracker.claim = undefined
        tracker.disconnected = true
        tracker.streamAttached = false
      }
    }
    return result
  }

  /**
   * Attach using the stored cursor (spawn / renderer-ref flow). Fast path:
   * an already-attached terminal just increments the ref count.
   */
  async attach(terminalId: string, claim?: string): Promise<IpcResult<void>> {
    const tracker = this.getOrCreate(terminalId)
    if (claim === undefined && tracker.streamAttached) {
      // Already authorized — add the renderer reference without another replay.
      tracker.refCount++
      return { success: true, data: undefined }
    }
    const result = await this.performAttach(terminalId, claim, undefined)
    return result.success ? { success: true, data: undefined } : result
  }

  /** Prime an output stream without manufacturing a renderer reference. */
  async primeAttachment(terminalId: string, claim: string): Promise<IpcResult<void>> {
    const result = await this.performAttach(terminalId, claim, undefined, false)
    return result.success ? { success: true, data: undefined } : result
  }

  /** Attach with an explicit cursor (cross-client handoff / desktop parity). */
  async attachWithCursor(
    terminalId: string,
    claim: string,
    lastSeq: number
  ): Promise<IpcResult<TerminalAttachResult>> {
    return this.performAttach(terminalId, claim, lastSeq)
  }

  /** Enumerate live host PTYs for a conversation or project (companion viewer). */
  list(scope: {
    conversationId?: string
    projectId?: string
  }): Promise<IpcResult<WebTerminalListResult>> {
    return this.request('list', scope)
  }

  /**
   * Subscribe to a desktop-owned PTY without a CAP-3 claim. Does not rotate
   * the desktop holder. Replay + live `data` match `attach`.
   */
  async watch(terminalId: string, lastSeq = 0): Promise<IpcResult<TerminalAttachResult>> {
    const result = await this.request<TerminalAttachResult>('watch', { terminalId, lastSeq })
    if (result.success) {
      const tracker = this.getOrCreate(terminalId)
      tracker.refCount += 1
      tracker.disconnected = false
      tracker.lastSeq = result.data.latestSeq
    }
    return result
  }

  /**
   * Authenticated cold resume. The host rotates the claim and installs replay
   * plus a live forwarder; adoption is atomic and memory-only on success.
   */
  async resume(request: TerminalResumeRequest): Promise<IpcResult<TerminalResumeGrant>> {
    const tracker = this.getOrCreate(request.terminalId)
    const result = await this.request<TerminalResumeGrant>('resume', { ...request })
    if (!result.success) {
      if (result.code !== 'NETWORK_ERROR') {
        tracker.claim = undefined
        tracker.refCount = 0
        tracker.disconnected = true
        tracker.streamAttached = false
      }
      return result
    }

    if (result.data.terminal.id !== request.terminalId || !result.data.claim) {
      tracker.claim = undefined
      tracker.refCount = 0
      tracker.disconnected = true
      tracker.streamAttached = false
      return failure('NETWORK_ERROR', 'Invalid terminal resume response')
    }

    tracker.claim = result.data.claim
    tracker.lastSeq = Math.max(tracker.lastSeq, result.data.terminal.latestSeq)
    tracker.exited = false
    tracker.disconnected = false
    tracker.streamAttached = true
    tracker.cleanupOnly = false
    return result
  }

  /**
   * Adopt a server-issued credential (spawn issuance / successful rotation).
   * Issuance is server-confirmed by definition, so adoption is immediate.
   */
  adoptClaim(terminalId: string, claim?: string): void {
    const tracker = this.getOrCreate(terminalId)
    tracker.claim = claim
    tracker.disconnected = claim === undefined
    tracker.cleanupOnly = false
    if (!claim) tracker.streamAttached = false
  }

  /**
   * Drop the credential and mark the terminal disconnected (revocation or
   * rotate/revoke teardown). The server has severed this connection's
   * attachment + authorization, so outstanding renderer refs can no longer be
   * counted as attached: the next attach must re-verify with a credential.
   */
  severClaim(terminalId: string, newClaim?: string): void {
    const tracker = this.trackers.get(terminalId)
    if (!tracker) return
    tracker.refCount = 0
    tracker.claim = newClaim
    tracker.disconnected = !newClaim
    tracker.streamAttached = false
  }

  /** Detach from a terminal's output stream when ref count reaches 0. */
  detach(terminalId: string): void {
    const tracker = this.trackers.get(terminalId)
    if (!tracker) return
    tracker.refCount = Math.max(0, tracker.refCount - 1)
    if (tracker.refCount <= 0) {
      tracker.streamAttached = false
      void this.request('detach', { terminalId }).catch(() => {})
      if (tracker.exited) this.trackers.delete(terminalId)
    }
  }

  /** Close every renderer attachment while retaining the claim/cursor tracker. */
  async closeView(terminalId: string): Promise<IpcResult<void>> {
    const tracker = this.trackers.get(terminalId)
    if (!tracker) return { success: true, data: undefined }
    const result = await this.request<void>('close_view', { terminalId })
    if (result.success) {
      tracker.refCount = 0
      tracker.streamAttached = false
    }
    return result
  }

  /** Remove a terminal from tracking (used only after explicit terminate/exit). */
  removeTracker(terminalId: string): void {
    void this.request('detach', { terminalId }).catch(() => {})
    this.trackers.delete(terminalId)
  }

  private getOrCreate(terminalId: string): TerminalTracker {
    let tracker = this.trackers.get(terminalId)
    if (!tracker) {
      tracker = {
        lastSeq: 0,
        exited: false,
        refCount: 0,
        streamAttached: false,
        disconnected: false,
        cleanupOnly: false
      }
      this.trackers.set(terminalId, tracker)
    }
    return tracker
  }

  /**
   * A failed cleanup revokes normal I/O authority but does not erase ownership.
   * Retain only the recoverable id so an explicit terminate retry can target the
   * quarantined resource; it must never drive reconnect, attach, or respawn.
   */
  private retainTerminalResourceFailure(result: IpcResult<unknown>): void {
    const failure = readTerminalResourceFailure(result)
    if (!failure) return

    const tracker = this.getOrCreate(failure.terminalId)
    tracker.claim = undefined
    tracker.refCount = 0
    tracker.streamAttached = false
    tracker.disconnected = true
    tracker.cleanupOnly = true
  }

  private markExited(terminalId: string): void {
    const tracker = this.trackers.get(terminalId)
    if (tracker) tracker.exited = true
  }

  onData(callback: TerminalDataCallback): () => void {
    this.dataSidecars.add(callback)
    return () => this.dataSidecars.delete(callback)
  }

  registerPrimaryTerminalData(callback: TerminalScopedDataCallback): PrimaryTerminalDataHandle {
    let boundTerminalId: string | null = null
    const release = (): void => {
      if (boundTerminalId === null) return
      // Identity check: a later handle may already own this id, and dropping
      // its registration here would silently stop that terminal's output.
      if (this.primaryDataHandlers.get(boundTerminalId) === callback) {
        this.primaryDataHandlers.delete(boundTerminalId)
      }
      boundTerminalId = null
    }
    return {
      bind: (terminalId: string): void => {
        if (boundTerminalId === terminalId) return
        release()
        boundTerminalId = terminalId
        this.primaryDataHandlers.set(terminalId, callback)
      },
      dispose: release
    }
  }

  onDataForTerminal(terminalId: string, callback: TerminalScopedDataCallback): () => void {
    const handle = this.registerPrimaryTerminalData(callback)
    handle.bind(terminalId)
    return handle.dispose
  }

  onExit(callback: TerminalExitCallback): () => void {
    this.exitCallbacks.add(callback)
    return () => this.exitCallbacks.delete(callback)
  }
  onSpawned(callback: (event: TerminalSpawnedEvent) => void): () => void {
    this.spawnedCallbacks.add(callback)
    return () => this.spawnedCallbacks.delete(callback)
  }
  onDisplayModeChanged(callback: (event: TerminalDisplayModeChangedEvent) => void): () => void {
    this.displayModeCallbacks.add(callback)
    return () => this.displayModeCallbacks.delete(callback)
  }
  onCwd(callback: TerminalCwdChangedCallback): () => void {
    this.cwdCallbacks.add(callback)
    return () => this.cwdCallbacks.delete(callback)
  }
  onBranch(callback: TerminalGitBranchChangedCallback): () => void {
    this.branchCallbacks.add(callback)
    return () => this.branchCallbacks.delete(callback)
  }
  onStatus(callback: TerminalGitStatusChangedCallback): () => void {
    this.statusCallbacks.add(callback)
    return () => this.statusCallbacks.delete(callback)
  }
  onExitCode(callback: TerminalExitCodeChangedCallback): () => void {
    this.exitCodeCallbacks.add(callback)
    return () => this.exitCodeCallbacks.delete(callback)
  }

  dispose(): void {
    this.disposed = true
    this.detachVisibilityListeners()
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer)
    this.rejectPending()
    this.socket?.close()
    this.dataSidecars.clear()
    this.primaryDataHandlers.clear()
  }

  private handleIncomingFrame(data: unknown): void {
    if (typeof data === 'string') {
      this.handleJsonFrame(data)
      return
    }
    if (data instanceof ArrayBuffer) {
      this.handleBinaryFrame(data)
      return
    }
    if (typeof Blob !== 'undefined' && data instanceof Blob) {
      void data
        .arrayBuffer()
        .then((buffer) => {
          if (!this.disposed) this.handleBinaryFrame(buffer)
        })
        .catch(() => {
          this.logInvalidBinaryFrame('Could not read terminal WebSocket Blob payload')
        })
    }
  }

  private dispatchData(terminalId: string, bytes: Uint8Array): void {
    // Paint first so a slow sidecar cannot delay what the user sees.
    this.primaryDataHandlers.get(terminalId)?.(bytes)
    for (const callback of this.dataSidecars) callback(terminalId, bytes)
  }

  private handleBinaryFrame(buffer: ArrayBuffer): void {
    const frame = decodeWebTerminalBinaryFrame(buffer)
    if (!frame) {
      this.logInvalidBinaryFrame('Rejected malformed terminal WebSocket binary frame')
      return
    }
    const tracker = this.getOrCreate(frame.terminalId)
    tracker.lastSeq = Math.max(tracker.lastSeq, frame.seq)
    this.dispatchData(frame.terminalId, frame.data)
  }

  private logInvalidBinaryFrame(message: string): void {
    if (this.invalidBinaryFrameLogged) return
    this.invalidBinaryFrameLogged = true
    void logFrontendError({
      level: 'warn',
      source: 'WebTerminalClient.binaryFrame',
      message
    })
  }

  private handleJsonFrame(text: string): void {
    let frame: WebTerminalFrame
    try {
      frame = JSON.parse(text) as WebTerminalFrame
    } catch {
      return
    }
    if ('id' in frame) {
      const pending = this.pending.get(frame.id)
      if (!pending) return
      clearTimeout(pending.timer)
      this.pending.delete(frame.id)
      pending.resolve(frame as WebTerminalReply<unknown>)
      return
    }
    if (frame.type === 'data') {
      const tracker = this.trackers.get(frame.terminalId)
      if (tracker && frame.seq !== undefined) {
        tracker.lastSeq = frame.seq
      }
      const bytes = Uint8Array.from(frame.data)
      this.dispatchData(frame.terminalId, bytes)
      return
    }
    if (frame.type === 'replay') {
      // Sequenced replay: write each chunk in order, update cursor.
      const tracker = this.getOrCreate(frame.terminalId)
      for (const chunk of frame.chunks) {
        const bytes = Uint8Array.from(chunk.data)
        this.dispatchData(frame.terminalId, bytes)
        tracker.lastSeq = chunk.seq
      }
      tracker.lastSeq = Math.max(tracker.lastSeq, frame.latestSeq)
      // If a gap was reported, write a visible marker.
      if (frame.gap) {
        const marker = new Uint8Array([
          0x1b,
          0x5b,
          0x33,
          0x33,
          0x6d, // ESC[33m (yellow)
          ...new TextEncoder().encode('\r\n[output gap — some history was evicted]\r\n'),
          0x1b,
          0x5b,
          0x30,
          0x6d // ESC[0m (reset)
        ])
        this.dispatchData(frame.terminalId, marker)
      }
      return
    }
    if (frame.type === 'gap') {
      // Server reported a broadcast lag — output may have been lost.
      const marker = new Uint8Array([
        0x1b,
        0x5b,
        0x33,
        0x33,
        0x6d,
        ...new TextEncoder().encode('\r\n[output lag — some bytes were dropped]\r\n'),
        0x1b,
        0x5b,
        0x30,
        0x6d
      ])
      this.dispatchData(frame.terminalId, marker)
      return
    }
    if (frame.type === 'event') this.handleEvent(frame.payload)
  }

  private handleEvent(event: WebTerminalEventPayload): void {
    switch (event.type) {
      case 'exit':
        this.markExited(event.terminal_id)
        for (const callback of this.exitCallbacks)
          callback(event.terminal_id, event.exit_code ?? -1, event.signal ?? undefined)
        break
      case 'cwd_changed':
        for (const callback of this.cwdCallbacks) callback(event.terminal_id, event.cwd)
        break
      case 'git_branch_changed':
        for (const callback of this.branchCallbacks) callback(event.terminal_id, event.branch)
        break
      case 'git_status_changed':
        for (const callback of this.statusCallbacks) callback(event.terminal_id, event.status)
        break
      case 'exit_code_changed':
        for (const callback of this.exitCodeCallbacks) callback(event.terminal_id, event.exit_code)
        break
      case 'spawned':
        for (const callback of this.spawnedCallbacks) {
          callback({
            terminalId: event.terminal_id,
            projectId: event.project_id,
            conversationId: event.conversation_id,
            cwd: event.cwd,
            cols: event.cols,
            rows: event.rows,
            shell: event.shell
          })
        }
        break
      case 'display_mode_changed':
        for (const callback of this.displayModeCallbacks) {
          callback({
            terminalId: event.terminal_id,
            mode: event.mode,
            cols: event.cols,
            rows: event.rows
          })
        }
        break
    }
  }

  private rejectPending(reason?: string): void {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer)
      pending.resolve({
        id: 'closed',
        success: false,
        error: reason ?? 'Terminal websocket disconnected',
        code: 'NETWORK_ERROR'
      })
    }
    this.pending.clear()
  }

  private scheduleReconnect(): void {
    if (this.disposed || this.reconnectTimer) return
    // Stop reconnecting if no terminal is both live AND holds a lease
    // credential — exited/disconnected terminals are never re-presented.
    const activeCount = Array.from(this.trackers.values()).filter(
      (t) => !t.exited && !t.disconnected && !t.cleanupOnly && t.refCount > 0
    ).length
    if (activeCount === 0) return
    if (this.reconnectAttempt >= RECONNECT_MAX_ATTEMPTS) return
    const delay = Math.min(RECONNECT_BASE_MS * 2 ** this.reconnectAttempt++, RECONNECT_MAX_MS)
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      void this.connect().catch(() => this.scheduleReconnect())
    }, delay)
  }

  /**
   * Attach `visibilitychange` + `focus` listeners (web only) so a return from
   * a backgrounded mobile tab proactively reconnects instead of waiting for an
   * `onclose` the suspended browser delivers late or never. Mirrors
   * `WsAcpTransport`: same threshold, coalescing, and `forceReconnect`
   * semantics. Idempotent; detached in `dispose`.
   */
  private attachVisibilityListeners(): void {
    if (this.visibilityHandler || typeof document === 'undefined') return
    const onVisibility = (): void => {
      if (document.visibilityState === 'hidden') {
        this.lastHiddenAt = Date.now()
        return
      }
      // visible — a backgrounded tab returning to the foreground.
      this.maybeReconnectOnReturn()
    }
    const onFocus = (): void => {
      // Fallback for platforms where `visibilitychange` is unreliable; only
      // acts when a hide was previously recorded so normal use is a no-op.
      this.maybeReconnectOnReturn()
    }
    this.visibilityHandler = onVisibility
    this.focusHandler = onFocus
    document.addEventListener('visibilitychange', onVisibility)
    // `focus` is a window-level event that does NOT bubble — attach to `window`.
    window.addEventListener('focus', onFocus)
  }

  /** Detach the visibility/focus listeners (called from `dispose`). */
  private detachVisibilityListeners(): void {
    if (this.visibilityHandler && typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', this.visibilityHandler)
      this.visibilityHandler = null
    }
    if (this.focusHandler && typeof window !== 'undefined') {
      window.removeEventListener('focus', this.focusHandler)
      this.focusHandler = null
    }
  }

  /**
   * On a return-to-foreground, if the page was hidden past the staleness
   * threshold OR the socket is not OPEN, force a reconnect so a half-open
   * socket killed server-side during AFK is recovered. After reopen, `connect`
   * re-attaches non-exited trackers via their stored `lastSeq`, replaying
   * missed output. Consumes `lastHiddenAt` so a `focus` following a
   * `visibilitychange` does not double-trigger.
   */
  private maybeReconnectOnReturn(): void {
    if (this.disposed) return
    const hiddenAt = this.lastHiddenAt
    this.lastHiddenAt = null
    if (hiddenAt == null) return // never recorded a hide — nothing to recover
    const hiddenFor = Date.now() - hiddenAt
    const socketDown = this.socket?.readyState !== this.WebSocketImpl.OPEN
    if (hiddenFor > VISIBILITY_STALE_THRESHOLD_MS || socketDown) {
      this.forceReconnect(
        socketDown ? 'socket closed while page was hidden' : 'visibility return after idle'
      )
    }
  }

  /**
   * Force a clean reconnect, bypassing the `connect()` fast path that trusts
   * `readyState === OPEN`. Tears down the suspect socket (detaching its
   * handlers so its eventual close does not double-fire `scheduleReconnect`),
   * resets `reconnectAttempt` so AFK never strands the terminal at the backoff
   * ceiling, clears any pending reconnect timer, then reuses `scheduleReconnect`
   * so the existing backoff + `onopen` re-attach machinery runs unchanged.
   */
  private forceReconnect(reason: string): void {
    if (this.disposed) return
    const old = this.socket
    // Capture the in-flight connect promise's reject BEFORE nulling so it can
    // be settled after the socket teardown. Without this, a `request()`
    // awaiting `connect()` (socket CONNECTING) hangs — its 15s timeout only
    // arms AFTER connect resolves, and forceReconnect nulls `connecting`
    // without rejecting the in-flight promise.
    const inflightReject = this.connectingReject
    this.socket = null
    this.connecting = null
    this.connectingReject = null
    this.rejectPending(reason)
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
    this.reconnectAttempt = 0
    if (old) {
      // Detach ALL handlers (incl. onopen) so a late CONNECTING→open on the
      // torn-down socket doesn't fire `onopen` against shared `this` state
      // (would clobber reconnectAttempt + null connecting).
      old.onopen = null
      old.onclose = null
      old.onerror = null
      old.onmessage = null
      try {
        old.close()
      } catch {
        // ignore — already closed
      }
    }
    // Settle the in-flight connect promise so awaiters throw → request()
    // catches → returns NETWORK_ERROR (mirrors WsAcpTransport).
    inflightReject?.(new Error(reason))
    this.scheduleReconnect()
  }
}

function failure(code: string, error: unknown): IpcResult<never> {
  return {
    success: false,
    code,
    error: error instanceof Error ? error.message : String(error)
  }
}

const client = new WebTerminalClient()

export function createWebTerminalApi(): TerminalApi {
  return {
    async spawn(options: TerminalSpawnOptions = {}): Promise<IpcResult<SpawnedTerminal>> {
      const result = await client.request<SpawnedTerminal>(
        'spawn',
        options as Record<string, unknown>
      )
      if (result.success) {
        if (result.data.claim) {
          // Adopt the issued credential, then prime the output stream without
          // manufacturing a renderer reference. addRendererRef accounts for
          // the actual mounted view after the component is ready.
          client.adoptClaim(result.data.id, result.data.claim)
          const attachResult = await client.primeAttachment(result.data.id, result.data.claim)
          if (!attachResult.success) {
            // A failed view attach is non-destructive. The PTY and claim remain
            // eligible for a later explicit reopen/reconnect.
            return { success: false, error: attachResult.error, code: attachResult.code }
          }
        } else {
          // Defensive: a claim-less spawn success cannot attach (no credential
          // to present). Return the spawn result WITHOUT killing the PTY — the
          // tracker stays claim-less and reconnect marks it disconnected. This
          // path is unreachable against a host that issues claims; it exists so
          // a malformed reply can never destroy a freshly spawned terminal.
          client.adoptClaim(result.data.id)
        }
      }
      return result
    },
    resume: (request) => client.resume(request),
    attach: (terminalId, claim, lastSeq) => client.attachWithCursor(terminalId, claim, lastSeq),
    watch: (terminalId, lastSeq) => client.watch(terminalId, lastSeq),
    onSpawned: (callback) => client.onSpawned(callback),
    onDisplayModeChanged: (callback) => client.onDisplayModeChanged(callback),
    async rotateClaim(terminalId: string, claim: string): Promise<IpcResult<RotatedClaim>> {
      const result = await client.request<RotatedClaim>('rotate_claim', { terminalId, claim })
      if (result.success) {
        // Teardown (amendment R1): the server detached this connection's
        // attachment and authorization. Adopt the fresh credential and force
        // a re-verified attach for any outstanding refs.
        client.severClaim(terminalId, result.data.claim)
      }
      return result
    },
    async revokeClaim(terminalId: string, claim: string): Promise<IpcResult<void>> {
      const result = await client.request<void>('revoke_claim', { terminalId, claim })
      if (result.success) {
        // Teardown (amendment R1): output stream + write/resize access gone.
        client.severClaim(terminalId)
      }
      return result
    },
    write: (terminalId, data) => client.request('write', { terminalId, data }),
    resize: (terminalId, cols, rows) => client.request('resize', { terminalId, cols, rows }),
    setDisplayMode: (
      terminalId,
      mode: TerminalDisplayMode,
      options: TerminalDisplayModeOptions = {}
    ) =>
      client.request<TerminalDisplayModeState>('set_display_mode', {
        terminalId,
        mode,
        cols: options.cols,
        rows: options.rows,
        force: options.force
      }),
    closeView: (terminalId) => client.closeView(terminalId),
    async terminate(terminalId): Promise<IpcResult<void>> {
      const result = await client.request<void>('terminate', { terminalId })
      if (result.success) client.removeTracker(terminalId)
      return result
    },
    /** @deprecated compatibility alias for terminate. */
    async kill(terminalId): Promise<IpcResult<void>> {
      const result = await client.request<void>('kill', { terminalId })
      if (result.success) client.removeTracker(terminalId)
      return result
    },
    onData: (callback) => client.onData(callback),
    onDataForTerminal: (terminalId, callback) => client.onDataForTerminal(terminalId, callback),
    registerPrimaryTerminalData: (callback) => client.registerPrimaryTerminalData(callback),
    onExit: (callback) => client.onExit(callback),
    onCwdChanged: (callback) => client.onCwd(callback),
    getCwd: (terminalId) => client.request('get_cwd', { terminalId }),
    onGitBranchChanged: (callback) => client.onBranch(callback),
    getGitBranch: (terminalId) => client.request('get_git_branch', { terminalId }),
    onGitStatusChanged: (callback) => client.onStatus(callback),
    getGitStatus: (terminalId) =>
      client.request<GitStatus | null>('get_git_status', { terminalId }),
    onExitCodeChanged: (callback) => client.onExitCode(callback),
    getExitCode: (terminalId) => client.request('get_exit_code', { terminalId }),
    updateOrphanDetection: (enabled, timeout) =>
      client.request('update_orphan_detection', { enabled, timeout })
  }
}

export const webTerminalInternals = {
  async addRendererRef(terminalId: string, rendererId: string): Promise<IpcResult<void>> {
    const attached = await client.attach(terminalId)
    if (!attached.success) return attached
    const result = await client.request<void>('add_renderer_ref', { terminalId, rendererId })
    if (!result.success) client.detach(terminalId)
    return result
  },
  async removeRendererRef(terminalId: string, rendererId: string): Promise<IpcResult<void>> {
    // Remove the backend ref while this connection is still authorized. Only
    // then detach the output/authorization and decrement the local ref count.
    const result = await client.request<void>('remove_renderer_ref', { terminalId, rendererId })
    client.detach(terminalId)
    return result
  },
  setProtected: (terminalId: string, protectedState: boolean) =>
    client.request<void>('set_protected', { terminalId, protected: protectedState }),
  list: (scope: { conversationId?: string; projectId?: string }) => client.list(scope),
  watch: (terminalId: string, lastSeq?: number) => client.watch(terminalId, lastSeq)
}
