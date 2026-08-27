/**
 * Tests for the web-mode persistence API (issue #613).
 *
 * Uses a fake WebSocket that speaks the minimal WS handshake
 * (`auth_required` → `authenticate`) and serves `store_read` / `store_write`
 * / `store_delete` from an in-memory map, mirroring the Rust `WebStore`.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createWebPersistenceApi } from '../web-persistence-api'

class FakeStoreSocket {
  static OPEN = 1
  static CONNECTING = 0
  static CLOSING = 2
  static CLOSED = 3

  readyState = FakeStoreSocket.CONNECTING
  onopen: ((ev: Event) => void) | null = null
  onmessage: ((ev: MessageEvent) => void) | null = null
  onerror: ((ev: Event) => void) | null = null
  onclose: ((ev: CloseEvent) => void) | null = null
  sent: string[] = []
  store = new Map<string, unknown>()
  /** When set, every non-auth request replies with this error. */
  failCode: string | null = null

  constructor(public url: string) {
    queueMicrotask(() => {
      this.readyState = FakeStoreSocket.OPEN
      this.onopen?.(new Event('open'))
      this.emit({ type: 'auth_required', payload: {} })
    })
  }

  emit(msg: unknown): void {
    this.onmessage?.({ data: JSON.stringify(msg) } as MessageEvent)
  }

  send(data: string): void {
    this.sent.push(data)
    const req = JSON.parse(data) as {
      id: string
      type: string
      payload: { key?: string; value?: unknown }
    }
    if (req.type === 'authenticate') {
      this.emit({ id: req.id, ok: true, payload: {} })
      return
    }
    if (this.failCode) {
      this.emit({ id: req.id, ok: false, err: { code: this.failCode, message: 'store failed' } })
      return
    }
    const key = req.payload.key as string
    if (req.type === 'store_read') {
      this.emit({ id: req.id, ok: true, payload: { value: this.store.get(key) ?? null } })
      return
    }
    if (req.type === 'store_write') {
      this.store.set(key, req.payload.value)
      this.emit({ id: req.id, ok: true, payload: {} })
      return
    }
    if (req.type === 'store_delete') {
      const existed = this.store.delete(key)
      this.emit({ id: req.id, ok: true, payload: { existed } })
      return
    }
    this.emit({ id: req.id, ok: false, err: { code: 'not_implemented', message: 'unknown' } })
  }

  closed = false

  close(): void {
    this.closed = true
    this.readyState = FakeStoreSocket.CLOSED
    this.onclose?.(new CloseEvent('close'))
  }
}

/**
 * A socket that never fires `onopen`, `onerror` or `onclose` — a TCP connect
 * left hanging by a firewalled host or a server that vanished mid-handshake.
 */
class SilentSocket {
  static OPEN = 1
  static CONNECTING = 0
  static CLOSING = 2
  static CLOSED = 3

  readyState = SilentSocket.CONNECTING
  onopen: ((ev: Event) => void) | null = null
  onmessage: ((ev: MessageEvent) => void) | null = null
  onerror: ((ev: Event) => void) | null = null
  onclose: ((ev: CloseEvent) => void) | null = null
  closed = false

  constructor(public url: string) {}

  send(): void {
    /* the peer never answers */
  }

  close(): void {
    this.closed = true
  }
}

/** Opens, but never answers the `authenticate` frame. */
class MuteAuthSocket extends SilentSocket {
  constructor(url: string) {
    super(url)
    queueMicrotask(() => {
      this.readyState = SilentSocket.OPEN
      this.onopen?.(new Event('open'))
    })
  }
}

const okVoid = { success: true, data: undefined }

describe('createWebPersistenceApi', () => {
  let sockets: FakeStoreSocket[]
  let api: ReturnType<typeof createWebPersistenceApi>

  beforeEach(() => {
    sockets = []
    class TrackerSocket extends FakeStoreSocket {
      constructor(url: string) {
        super(url)
        sockets.push(this)
      }
    }
    api = createWebPersistenceApi({ WebSocketImpl: TrackerSocket as unknown as typeof WebSocket })
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('gives up on a connect that never opens instead of waiting forever', async () => {
    // `request()` arms its own timeout only after `connect()` resolves, so a
    // stalled connect used to hang every store call with no error at all.
    vi.useFakeTimers()
    const api = createWebPersistenceApi({
      WebSocketImpl: SilentSocket as unknown as typeof WebSocket
    })

    const pending = api.write('k', 1)
    await vi.advanceTimersByTimeAsync(15_000)

    expect(await pending).toEqual({
      success: false,
      error: 'store connection timed out',
      code: 'WS_TIMEOUT'
    })
  })

  it('gives up when the peer opens but never completes the handshake', async () => {
    // `authenticate()` has no timeout of its own; the connect bound is what
    // covers it, and an unanswered handshake is the likelier failure in
    // practice than a socket that never opens.
    vi.useFakeTimers()
    const api = createWebPersistenceApi({
      WebSocketImpl: MuteAuthSocket as unknown as typeof WebSocket
    })

    const pending = api.write('k', 1)
    await vi.advanceTimersByTimeAsync(15_000)

    expect(await pending).toEqual({
      success: false,
      error: 'store connection timed out',
      code: 'WS_TIMEOUT'
    })
  })

  it('does not let the connect bound tear down a healthy idle socket', async () => {
    // The timer must be cleared once the handshake succeeds. Left armed, it
    // fires 15s later and closes a socket that is working fine — the failure
    // would look like the store silently dying after a quiet period.
    vi.useFakeTimers()
    expect(await api.write('k', 1)).toEqual(okVoid)

    await vi.advanceTimersByTimeAsync(60_000)

    expect(sockets[0].closed).toBe(false)
    expect(await api.write('k', 2)).toEqual(okVoid)
  })

  it('read returns KEY_NOT_FOUND for a missing key', async () => {
    const result = await api.read('settings')
    expect(result).toEqual({
      success: false,
      error: 'Key not found: settings',
      code: 'KEY_NOT_FOUND'
    })
  })

  it('read unwraps a versioned value written by write', async () => {
    await api.write('settings', { theme: 'dark' })
    const result = await api.read<{ theme: string }>('settings')
    expect(result).toEqual({ success: true, data: { theme: 'dark' } })
  })

  it('write persists a versioned payload to the server', async () => {
    const result = await api.write('settings', { theme: 'dark' })
    expect(result).toEqual(okVoid)
    const stored = sockets[0].store.get('settings') as { _version: number; data: unknown }
    expect(stored).toEqual({ _version: 1, data: { theme: 'dark' } })
  })

  it('delete removes a key', async () => {
    await api.write('k', 1)
    const result = await api.delete('k')
    expect(result).toEqual(okVoid)
    expect(sockets[0].store.has('k')).toBe(false)
  })

  it('writeDebounced coalesces rapid writes into a single store_write', async () => {
    vi.useFakeTimers()
    const p1 = api.writeDebounced('k', 1)
    const p2 = api.writeDebounced('k', 2)
    await vi.advanceTimersByTimeAsync(500)
    await expect(p1).resolves.toEqual(okVoid)
    await expect(p2).resolves.toEqual(okVoid)

    const writes = sockets[0].sent.filter((s) => JSON.parse(s).type === 'store_write')
    expect(writes).toHaveLength(1)
    const stored = JSON.parse(writes[0]).payload.value as { data: unknown }
    expect(stored.data).toBe(2)
  })

  it('flushPendingWrites flushes immediately', async () => {
    vi.useFakeTimers()
    const p = api.writeDebounced('k', { a: 1 })
    const result = await api.flushPendingWrites()
    expect(result).toEqual(okVoid)
    await expect(p).resolves.toEqual(okVoid)
    expect(sockets[0].store.get('k')).toEqual({ _version: 1, data: { a: 1 } })
  })

  it('maps a server error code into the IpcResult', async () => {
    // Warm the socket (connect is lazy) before arming the failure.
    await api.read('warmup')
    sockets[0].failCode = 'STORE_UNAVAILABLE'
    const result = await api.write('k', 1)
    expect(result).toEqual({
      success: false,
      error: 'store failed',
      code: 'STORE_UNAVAILABLE'
    })
  })
})
