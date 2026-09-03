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
import { Channel, type InvokeArgs, invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { logFrontendError } from './log-api'
import { cleanupTauriListener, isTauriContext } from './tauri-runtime'

/**
 * Attach/watch replay cursor that means "live only". Used when the spawn
 * Channel has already been painting this PTY — replaying the output log
 * would dump history a second time into xterm.
 *
 * Safe as JSON + Rust `u64`; PTY seqs never approach 2^53 in a session.
 */
const LIVE_HANDOFF_LAST_SEQ = Number.MAX_SAFE_INTEGER

/**
 * IPC Event names matching Rust commands in src-tauri/src/commands.rs
 * Using kebab-case as defined in tech spec
 */
const IPC_EVENTS = {
  TERMINAL_DATA: 'terminal-data',
  TERMINAL_EXIT: 'terminal-exit',
  TERMINAL_CWD_CHANGED: 'terminal-cwd-changed',
  TERMINAL_GIT_BRANCH_CHANGED: 'terminal-git-branch-changed',
  TERMINAL_GIT_STATUS_CHANGED: 'terminal-git-status-changed',
  TERMINAL_EXIT_CODE_CHANGED: 'terminal-exit-code-changed',
  TERMINAL_SPAWNED: 'terminal-spawned',
  TERMINAL_DISPLAY_MODE_CHANGED: 'terminal-display-mode-changed'
} as const

type EventPayloadMap = {
  [IPC_EVENTS.TERMINAL_DATA]: { id: string; data: string }
  [IPC_EVENTS.TERMINAL_EXIT]: { id: string; exitCode: number | null; signal: number | null }
  [IPC_EVENTS.TERMINAL_CWD_CHANGED]: { terminalId: string; cwd: string }
  [IPC_EVENTS.TERMINAL_GIT_BRANCH_CHANGED]: { terminalId: string; branch: string | null }
  [IPC_EVENTS.TERMINAL_GIT_STATUS_CHANGED]: { terminalId: string; status: GitStatus | null }
  [IPC_EVENTS.TERMINAL_EXIT_CODE_CHANGED]: { terminalId: string; exitCode: number }
  [IPC_EVENTS.TERMINAL_SPAWNED]: TerminalSpawnedEvent
  [IPC_EVENTS.TERMINAL_DISPLAY_MODE_CHANGED]: TerminalDisplayModeChangedEvent
}

type SharedListenerEntry<T> = {
  callbacks: Map<number, (payload: T) => void>
  nextCallbackId: number
  unlisten?: Promise<UnlistenFn>
}

/**
 * IPC Command names matching Rust commands in src-tauri/src/commands/terminal.rs
 */
const IPC_COMMANDS = {
  SPAWN: 'terminal_spawn',
  RESUME: 'terminal_resume',
  ATTACH: 'terminal_attach',
  WATCH: 'terminal_watch',
  ROTATE_CLAIM: 'terminal_rotate_claim',
  REVOKE_CLAIM: 'terminal_revoke_claim',
  WRITE: 'terminal_write',
  RESIZE: 'terminal_resize',
  SET_DISPLAY_MODE: 'terminal_set_display_mode',
  CLOSE_VIEW: 'terminal_close_view',
  TERMINATE: 'terminal_terminate',
  KILL: 'terminal_kill',
  GET_CWD: 'terminal_get_cwd',
  GET_GIT_BRANCH: 'terminal_get_git_branch',
  GET_GIT_STATUS: 'terminal_get_git_status',
  GET_EXIT_CODE: 'terminal_get_exit_code',
  UPDATE_ORPHAN_DETECTION: 'terminal_update_orphan_detection',
  ADD_RENDERER_REF: 'terminal_add_renderer_ref',
  REMOVE_RENDERER_REF: 'terminal_remove_renderer_ref',
  SET_PROTECTED: 'terminal_set_protected'
} as const

/**
 * Invoke Tauri IPC commands that already return IpcResult<T> from Rust.
 * The Rust commands in commands.rs wrap their results in IpcResult::success/error,
 * so we must NOT wrap them again here.
 */
async function invokeIpc<T>(command: string, args?: InvokeArgs): Promise<IpcResult<T>> {
  try {
    return await invoke<IpcResult<T>>(command, args)
  } catch (error) {
    return {
      success: false,
      error: error instanceof Error ? error.message : String(error),
      code: 'INVOKE_ERROR'
    }
  }
}

/**
 * Spawn tracking variables to detect and prevent spawn loops
 */
const IS_DEV = import.meta.env.DEV

function devLog(...args: unknown[]): void {
  if (IS_DEV) {
    console.log(...args)
  }
}

const sharedEventListeners = new Map<keyof EventPayloadMap, SharedListenerEntry<unknown>>()

function subscribeSharedEvent<K extends keyof EventPayloadMap>(
  eventName: K,
  callback: (payload: EventPayloadMap[K]) => void,
  debugLabel: string
): () => void {
  if (!isTauriContext()) {
    if (IS_DEV) {
      devLog(`[TauriTerminalAPI] Skipping ${debugLabel} listener outside Tauri runtime`)
    }
    return () => {}
  }

  let entry = sharedEventListeners.get(eventName) as
    | SharedListenerEntry<EventPayloadMap[K]>
    | undefined

  if (!entry) {
    if (IS_DEV) {
      devLog(`[TauriTerminalAPI] Creating shared ${debugLabel} native listener`)
    }

    entry = {
      callbacks: new Map<number, (payload: EventPayloadMap[K]) => void>(),
      nextCallbackId: 0
    }

    sharedEventListeners.set(eventName, entry as SharedListenerEntry<unknown>)

    try {
      entry.unlisten = listen<EventPayloadMap[K]>(eventName, ({ payload }) => {
        const currentEntry = sharedEventListeners.get(eventName) as
          | SharedListenerEntry<EventPayloadMap[K]>
          | undefined

        if (!currentEntry) {
          return
        }

        for (const [subscriberId, subscriber] of currentEntry.callbacks.entries()) {
          try {
            subscriber(payload)
          } catch (error) {
            console.error(`[TauriTerminalAPI] Listener callback failed`, {
              eventName,
              debugLabel,
              subscriberId,
              subscriberCount: currentEntry.callbacks.size,
              error
            })
          }
        }
      }).catch((error) => {
        console.error(`[TauriTerminalAPI] Failed to register ${debugLabel} listener:`, error)
        if (sharedEventListeners.get(eventName) === (entry as SharedListenerEntry<unknown>)) {
          sharedEventListeners.delete(eventName)
        }
        return () => {}
      })
    } catch (error) {
      console.error(`[TauriTerminalAPI] Failed to register ${debugLabel} listener:`, error)
      sharedEventListeners.delete(eventName)
      return () => {}
    }
  }

  const subscriberId = entry.nextCallbackId++
  entry.callbacks.set(subscriberId, callback)

  if (IS_DEV) {
    devLog(
      `[TauriTerminalAPI] Shared ${debugLabel} subscriber added (count=${entry.callbacks.size})`
    )
  }

  return () => {
    const currentEntry = sharedEventListeners.get(eventName) as
      | SharedListenerEntry<EventPayloadMap[K]>
      | undefined

    if (!currentEntry) {
      return
    }

    currentEntry.callbacks.delete(subscriberId)

    if (IS_DEV) {
      devLog(
        `[TauriTerminalAPI] Shared ${debugLabel} subscriber removed (count=${currentEntry.callbacks.size})`
      )
    }

    if (currentEntry.callbacks.size > 0) {
      return
    }

    if (IS_DEV) {
      devLog(`[TauriTerminalAPI] Disposing shared ${debugLabel} native listener`)
    }

    sharedEventListeners.delete(eventName)
    cleanupTauriListener(currentEntry.unlisten)
  }
}

let SPAWN_CALL_COUNTER = 0
const SPAWN_CALLS: Array<{
  id: string
  timestamp: number
  shell?: string
  cwd?: string
  stack: string
}> = []

/**
 * Capture stack trace for debugging spawn calls
 */
function captureStackTrace(): string {
  const stack = new Error().stack?.split('\n').slice(3).join('\n') || ''
  return stack
}

/**
 * Create a TerminalApi implementation using Tauri IPC
 *
 * This adapter maps all TerminalApi methods to Tauri invoke() calls and event listeners.
 * It maintains the same interface as the Electron preload script for easy migration.
 */
export function createTauriTerminalApi(): TerminalApi {
  // A PTY has exactly one live writer — the renderer painting it into xterm —
  // held in a Map so a second concurrent writer cannot exist by construction.
  // Sidecars observe every PTY read-only (transcript capture, diagnostics) and
  // are deliberately a separate container so the two roles cannot be confused.
  const primaryDataHandlers = new Map<string, TerminalScopedDataCallback>()
  const dataSidecars = new Set<TerminalDataCallback>()
  // Spawn's flusher Channel stays open for the PTY lifetime. Attach/watch
  // install a second Channel on the same broadcast. Both dispatch here, so
  // exactly one of them must be the live source or every chunk is written
  // twice. An entry here means this renderer owns a spawn Channel for that
  // terminal; `muted` records an explicit handoff of the live role:
  //   present + unmuted -> spawn is authoritative, live Channels stay quiet
  //   present + muted   -> spawn stood down, the live Channel took over
  //   absent            -> this renderer never spawned it, live Channel owns it
  // Tauri Channels have no unsubscribe, so Rust keeps sending on both regardless
  // and the loser has to drop its chunks on arrival.
  const spawnOutputGates = new Map<string, { muted: boolean }>()

  const dispatchTerminalData = (terminalId: string, bytes: Uint8Array): void => {
    // Paint first so a slow sidecar cannot delay what the user sees.
    const primary = primaryDataHandlers.get(terminalId)
    if (primary) {
      try {
        primary(bytes)
      } catch (error) {
        console.error('[BinaryChannel] Error in primary terminal data handler:', error)
      }
    }

    for (const callback of dataSidecars) {
      try {
        callback(terminalId, bytes)
      } catch (error) {
        console.error('[BinaryChannel] Error in terminal data sidecar:', error)
      }
    }
  }

  const registerPrimaryTerminalData = (
    callback: TerminalScopedDataCallback
  ): PrimaryTerminalDataHandle => {
    let boundTerminalId: string | null = null
    const release = (): void => {
      if (boundTerminalId === null) return
      // Identity check: a later handle may already own this id, and dropping
      // its registration here would silently stop that terminal's output.
      if (primaryDataHandlers.get(boundTerminalId) === callback) {
        primaryDataHandlers.delete(boundTerminalId)
      }
      boundTerminalId = null
    }
    return {
      bind(terminalId: string): void {
        if (boundTerminalId === terminalId) return
        release()
        boundTerminalId = terminalId
        primaryDataHandlers.set(terminalId, callback)
      },
      dispose: release
    }
  }

  // Silent while healthy. Both of these mean a second live writer was about to
  // exist, which is the failure that duplicated every keystroke echo and every
  // command block on screen — worth a log line the first time it happens per
  // terminal, and worth nothing at all when it does not.
  const reportedLiveSuppression = new Set<string>()
  const reportDataPathAnomaly = (event: string, terminalId: string, detail: string): void => {
    void logFrontendError({
      level: 'warn',
      source: 'terminal-data-path',
      message: `${event} terminalId=${terminalId} ${detail}`
    })
  }

  const createTerminalDataChannel = (terminalId: string): Channel<ArrayBuffer> => {
    const onData = new Channel<ArrayBuffer>()
    onData.onmessage = (buf: ArrayBuffer) => {
      // Ownership is resolved per chunk, not at channel creation: attach/watch
      // can run before `spawn` resolves and registers its gate, so a creation
      // time check would still race. An unmuted gate means this renderer's own
      // spawn Channel is the authoritative live source for the PTY lifetime, so
      // this second Channel must stay quiet or every chunk is written twice.
      const gate = spawnOutputGates.get(terminalId)
      if (gate && !gate.muted) {
        if (!reportedLiveSuppression.has(terminalId)) {
          reportedLiveSuppression.add(terminalId)
          reportDataPathAnomaly(
            'suppressed-redundant-live-channel',
            terminalId,
            'spawn Channel owns the live role; this attach/watch Channel would have duplicated every chunk'
          )
        }
        return
      }
      dispatchTerminalData(terminalId, new Uint8Array(buf))
    }
    return onData
  }

  const muteSpawnOutput = (terminalId: string): boolean => {
    const gate = spawnOutputGates.get(terminalId)
    if (!gate || gate.muted) return false
    gate.muted = true
    return true
  }

  const unmuteSpawnOutput = (terminalId: string): void => {
    const gate = spawnOutputGates.get(terminalId)
    if (gate) gate.muted = false
  }

  const dropSpawnOutput = (terminalId: string): void => {
    spawnOutputGates.delete(terminalId)
  }

  const beginLiveHandoff = (
    terminalId: string,
    lastSeq: number,
    reason: 'attach' | 'watch' | 'resume'
  ): { lastSeq: number; handedOff: boolean } => {
    if (!spawnOutputGates.has(terminalId)) {
      return { lastSeq, handedOff: false }
    }
    if (muteSpawnOutput(terminalId)) {
      void logFrontendError({
        level: 'warn',
        source: 'tauri-terminal-api.output-handoff',
        message: `muted spawn channel terminalId=${terminalId} reason=${reason}`
      })
    }
    return { lastSeq: LIVE_HANDOFF_LAST_SEQ, handedOff: true }
  }

  /**
   * Report replay coverage from *this renderer's* point of view, not the host's.
   *
   * The host answers `gap` for the cursor it was given, but on this transport a
   * successful attach/watch does not imply its bytes ever reached xterm:
   *
   *  - `handedOff` means we deliberately sent `LIVE_HANDOFF_LAST_SEQ`, so the
   *    host replayed nothing at all — yet it still answers `gap: false`, because
   *    no retained chunk sits above `u64::MAX`.
   *  - An unmuted spawn gate means `createTerminalDataChannel` drops every chunk
   *    on this Channel, replay included.
   *
   * In both cases the caller must fall back to its own transcript, so coverage
   * is downgraded to a gap. Erring this way costs a duplicated block at worst;
   * erring the other way silently loses everything written while detached.
   */
  const withDeliveredCoverage = (
    result: IpcResult<TerminalAttachResult>,
    terminalId: string,
    handedOff: boolean
  ): IpcResult<TerminalAttachResult> => {
    if (!result.success) return result
    const gate = spawnOutputGates.get(terminalId)
    const replayReachedRenderer = !handedOff && !(gate && !gate.muted)
    if (replayReachedRenderer) return result
    return { ...result, data: { ...result.data, gap: true } }
  }

  const attachTerminal = async (
    terminalId: string,
    claim: string,
    lastSeq: number
  ): Promise<IpcResult<TerminalAttachResult>> => {
    if (!claim) {
      return { success: false, error: 'Unauthorized', code: 'UNAUTHORIZED' }
    }

    const handoff = beginLiveHandoff(terminalId, lastSeq, 'attach')
    const onData = createTerminalDataChannel(terminalId)
    const result = await invokeIpc<TerminalAttachResult>(IPC_COMMANDS.ATTACH, {
      terminalId,
      claim,
      lastSeq: handoff.lastSeq,
      onData
    })
    if (!result.success) {
      onData.onmessage = () => {}
      if (handoff.handedOff) unmuteSpawnOutput(terminalId)
    }
    return withDeliveredCoverage(result, terminalId, handoff.handedOff)
  }

  const _registerListener = <T>(
    eventName: string,
    callback: (payload: T) => void,
    debugLabel: string
  ): (() => void) => {
    if (!isTauriContext()) {
      if (import.meta.env.DEV) {
        devLog(`[TauriTerminalAPI] Skipping ${debugLabel} listener outside Tauri runtime`)
      }
      return () => {}
    }

    let unlisten: Promise<UnlistenFn> | undefined

    try {
      unlisten = listen<T>(eventName, ({ payload }) => {
        callback(payload)
      })
    } catch (error) {
      console.error(`[TauriTerminalAPI] Failed to register ${debugLabel} listener:`, error)
      return () => {}
    }

    return () => {
      cleanupTauriListener(unlisten)
    }
  }

  return {
    /**
     * Spawn a new terminal PTY
     *
     * CAP-3: the successful result carries the issued `claim` credential —
     * spawn is the only issuance path. Callers must store the claim in the
     * terminal store (in-memory only; it is never persisted).
     */
    async spawn(options?: TerminalSpawnOptions): Promise<IpcResult<SpawnedTerminal>> {
      if (IS_DEV) {
        const spawnId = `spawn-${SPAWN_CALL_COUNTER++}-${Date.now().toString(36)}`
        const stack = captureStackTrace()

        const callInfo = {
          id: spawnId,
          timestamp: Date.now(),
          shell: options?.shell,
          cwd: options?.cwd,
          stack
        }

        SPAWN_CALLS.push(callInfo)

        devLog('═══════════════════════════════════════════════════════════════')
        devLog('[SPAWN CALL]', {
          id: spawnId,
          totalCalls: SPAWN_CALLS.length,
          options,
          stackTrace: stack,
          recentCalls: SPAWN_CALLS.slice(-5).map((c) => ({
            id: c.id,
            time: new Date(c.timestamp).toISOString().split('T')[1].slice(0, 12),
            shell: c.shell,
            cwd: c.cwd
          }))
        })
        devLog('═══════════════════════════════════════════════════════════════')

        if (SPAWN_CALLS.length >= 5) {
          const last5 = SPAWN_CALLS.slice(-5)
          const timeSpan = last5[4].timestamp - last5[0].timestamp
          if (timeSpan < 2000) {
            console.warn('⚠️ Rapid spawns detected', {
              callCount: last5.length,
              timeSpan: `${timeSpan}ms`,
              calls: last5.map((c) => ({
                id: c.id,
                time: new Date(c.timestamp).toISOString().split('T')[1].slice(0, 12),
                stack: c.stack
              }))
            })
          }
        }

        if (SPAWN_CALLS.length >= 10) {
          console.debug(`Spawn tracking: ${SPAWN_CALLS.length} terminals spawned`)
        }
      }

      // Create a binary data channel for this terminal session.
      // PTY output arrives as ArrayBuffer with no JSON encoding overhead.
      const on_data = new Channel<ArrayBuffer>()

      // Capture terminal ID from the invoke response once available.
      // We pass the channel to Rust synchronously. The channel's onmessage
      // fires with ArrayBuffer chunks as they arrive. We dispatch to all
      // registered TerminalDataCallback instances, but we need the terminal ID
      // which we get from the spawn result.
      //
      // We handle this by buffering data in-flight before the spawn result
      // arrives (unlikely but possible with fast PTY output).
      let pendingBuffer: Uint8Array[] = []
      let capturedTerminalId: string | null = null

      on_data.onmessage = (buf: ArrayBuffer) => {
        const bytes = new Uint8Array(buf)

        if (capturedTerminalId) {
          if (spawnOutputGates.get(capturedTerminalId)?.muted) return
          dispatchTerminalData(capturedTerminalId, bytes)
        } else {
          // Data arrived before spawn result — buffer it
          pendingBuffer.push(bytes)
        }
      }

      const result = await invokeIpc<SpawnedTerminal>(IPC_COMMANDS.SPAWN, {
        options,
        onData: on_data
      })

      if (result.success && result.data) {
        capturedTerminalId = result.data.id
        spawnOutputGates.set(capturedTerminalId, { muted: false })

        // Flush any buffered data that arrived before we knew the terminal ID
        if (pendingBuffer.length > 0) {
          for (const bytes of pendingBuffer) {
            if (spawnOutputGates.get(capturedTerminalId)?.muted) break
            dispatchTerminalData(capturedTerminalId, bytes)
          }
          pendingBuffer = []
        }
      } else {
        // Spawn failed — clean up channel to prevent memory leaks
        pendingBuffer = []
        on_data.onmessage = () => {}
      }

      return result
    },

    /**
     * Resume a passive SessionWorkspace reference, then attach from the
     * returned replay watermark so this method resolves only after live output
     * continuity is installed. Neither operation spawns or terminates a PTY.
     */
    async resume(request: TerminalResumeRequest): Promise<IpcResult<TerminalResumeGrant>> {
      const handoff = beginLiveHandoff(request.terminalId, request.lastSeq, 'resume')
      const replayChannel = createTerminalDataChannel(request.terminalId)
      if (handoff.handedOff) {
        // Spawn already painted this history. Drop the resume replay so it
        // cannot reprint the prompt / last command on top of the live buffer.
        replayChannel.onmessage = () => {}
      }
      const resumed = await invokeIpc<TerminalResumeGrant>(IPC_COMMANDS.RESUME, {
        request,
        onData: replayChannel
      })
      if (!resumed.success) {
        replayChannel.onmessage = () => {}
        if (handoff.handedOff) unmuteSpawnOutput(request.terminalId)
        return {
          success: false,
          error: resumed.code === 'UNAUTHORIZED' ? 'Unauthorized' : 'Terminal resume failed',
          code: resumed.code
        }
      }
      if (resumed.data.terminal.id !== request.terminalId || !resumed.data.claim) {
        replayChannel.onmessage = () => {}
        if (handoff.handedOff) unmuteSpawnOutput(request.terminalId)
        return {
          success: false,
          error: 'Terminal resume failed',
          code: 'NETWORK_ERROR'
        }
      }

      const attached = await attachTerminal(
        resumed.data.terminal.id,
        resumed.data.claim,
        resumed.data.terminal.latestSeq
      )
      if (!attached.success) {
        replayChannel.onmessage = () => {}
        if (handoff.handedOff) unmuteSpawnOutput(request.terminalId)
        return {
          success: false,
          error: attached.code === 'UNAUTHORIZED' ? 'Unauthorized' : 'Terminal resume failed',
          code: attached.code
        }
      }

      return resumed
    },

    /**
     * CAP-3: attach to a terminal's output stream with terminalId + claim +
     * lastSeq. The host verifies the credential BEFORE any replay; every
     * verification failure resolves to the generic UNAUTHORIZED error with no
     * terminal metadata or output. Output streams through a dedicated raw
     * channel (parity with the spawn channel), and the response's `latestSeq`
     * is the desktop reattach cursor.
     */
    attach: attachTerminal,

    async watch(terminalId: string, lastSeq: number): Promise<IpcResult<TerminalAttachResult>> {
      const handoff = beginLiveHandoff(terminalId, lastSeq, 'watch')
      const onData = createTerminalDataChannel(terminalId)
      const result = await invokeIpc<TerminalAttachResult>(IPC_COMMANDS.WATCH, {
        terminalId,
        lastSeq: handoff.lastSeq,
        onData
      })
      if (!result.success) {
        onData.onmessage = () => {}
        if (handoff.handedOff) unmuteSpawnOutput(terminalId)
      }
      return withDeliveredCoverage(result, terminalId, handoff.handedOff)
    },

    onSpawned(callback: (event: TerminalSpawnedEvent) => void): () => void {
      return subscribeSharedEvent(IPC_EVENTS.TERMINAL_SPAWNED, callback, 'terminal-spawned')
    },

    /**
     * CAP-3: possession-based rotation — the returned credential replaces the
     * presented one atomically.
     */
    async rotateClaim(terminalId: string, claim: string): Promise<IpcResult<RotatedClaim>> {
      return invokeIpc<RotatedClaim>(IPC_COMMANDS.ROTATE_CLAIM, { terminalId, claim })
    },

    /**
     * CAP-3: revoke the credential; the PTY keeps running.
     */
    async revokeClaim(terminalId: string, claim: string): Promise<IpcResult<void>> {
      return invokeIpc<void>(IPC_COMMANDS.REVOKE_CLAIM, { terminalId, claim })
    },

    /**
     * Write data to terminal PTY
     */
    async write(terminalId: string, data: string): Promise<IpcResult<void>> {
      return invokeIpc<void>(IPC_COMMANDS.WRITE, { terminalId, data })
    },

    /**
     * Resize terminal PTY
     */
    async resize(terminalId: string, cols: number, rows: number): Promise<IpcResult<void>> {
      return invokeIpc<void>(IPC_COMMANDS.RESIZE, { terminalId, cols, rows })
    },

    async setDisplayMode(
      terminalId: string,
      mode: TerminalDisplayMode,
      options: TerminalDisplayModeOptions = {}
    ): Promise<IpcResult<TerminalDisplayModeState>> {
      return invokeIpc<TerminalDisplayModeState>(IPC_COMMANDS.SET_DISPLAY_MODE, {
        terminalId,
        mode,
        cols: options.cols,
        rows: options.rows
      })
    },

    onDisplayModeChanged(callback: (event: TerminalDisplayModeChangedEvent) => void): () => void {
      return subscribeSharedEvent(
        IPC_EVENTS.TERMINAL_DISPLAY_MODE_CHANGED,
        callback,
        'terminal-display-mode-changed'
      )
    },

    /** Close one renderer view; the PTY and claim survive. */
    async closeView(terminalId: string): Promise<IpcResult<void>> {
      return invokeIpc<void>(IPC_COMMANDS.CLOSE_VIEW, { terminalId })
    },

    /** Explicitly terminate the PTY resource. Structured cleanup failures pass through unchanged. */
    async terminate(terminalId: string): Promise<IpcResult<void>> {
      dropSpawnOutput(terminalId)
      const result = await invokeIpc<void>(IPC_COMMANDS.TERMINATE, { terminalId })
      return result
    },

    /** @deprecated compatibility alias for terminate. */
    async kill(terminalId: string): Promise<IpcResult<void>> {
      dropSpawnOutput(terminalId)
      const result = await invokeIpc<void>(IPC_COMMANDS.KILL, { terminalId })
      return result
    },

    /**
     * Subscribe to terminal data events (binary channel)
     * Each callback receives (terminalId, data as Uint8Array).
     *
     * Data arrives via per-terminal Tauri Channels created during spawn(),
     * then dispatched to registered callbacks.
     */
    onData(callback: TerminalDataCallback): () => void {
      dataSidecars.add(callback)
      return () => {
        dataSidecars.delete(callback)
      }
    },

    onDataForTerminal(terminalId: string, callback: TerminalScopedDataCallback): () => void {
      const handle = registerPrimaryTerminalData(callback)
      handle.bind(terminalId)
      return handle.dispose
    },

    registerPrimaryTerminalData,

    /**
     * Subscribe to terminal exit events
     * Returns cleanup function (UnlistenFn)
     */
    onExit(callback: TerminalExitCallback): () => void {
      return subscribeSharedEvent(
        IPC_EVENTS.TERMINAL_EXIT,
        (payload) => {
          if (import.meta.env.DEV) {
            devLog(`[TauriTerminalAPI] Terminal ${payload.id} exited with code ${payload.exitCode}`)
          }
          callback(payload.id, payload.exitCode ?? -1, payload.signal ?? undefined)
        },
        'terminal-exit'
      )
    },

    /**
     * Subscribe to CWD change events
     * Returns cleanup function (UnlistenFn)
     */
    onCwdChanged(callback: TerminalCwdChangedCallback): () => void {
      return subscribeSharedEvent(
        IPC_EVENTS.TERMINAL_CWD_CHANGED,
        (payload) => {
          callback(payload.terminalId, payload.cwd)
        },
        'terminal-cwd-changed'
      )
    },

    /**
     * Get current working directory for terminal
     */
    async getCwd(terminalId: string): Promise<IpcResult<string | null>> {
      return invokeIpc<string | null>(IPC_COMMANDS.GET_CWD, { terminalId })
    },

    /**
     * Subscribe to git branch change events
     * Returns cleanup function (UnlistenFn)
     */
    onGitBranchChanged(callback: TerminalGitBranchChangedCallback): () => void {
      return subscribeSharedEvent(
        IPC_EVENTS.TERMINAL_GIT_BRANCH_CHANGED,
        (payload) => {
          callback(payload.terminalId, payload.branch)
        },
        'terminal-git-branch-changed'
      )
    },

    /**
     * Get git branch for terminal
     */
    async getGitBranch(terminalId: string): Promise<IpcResult<string | null>> {
      return invokeIpc<string | null>(IPC_COMMANDS.GET_GIT_BRANCH, { terminalId })
    },

    /**
     * Subscribe to git status change events
     * Returns cleanup function (UnlistenFn)
     */
    onGitStatusChanged(callback: TerminalGitStatusChangedCallback): () => void {
      return subscribeSharedEvent(
        IPC_EVENTS.TERMINAL_GIT_STATUS_CHANGED,
        (payload) => {
          callback(payload.terminalId, payload.status)
        },
        'terminal-git-status-changed'
      )
    },

    /**
     * Get git status for terminal
     */
    async getGitStatus(terminalId: string): Promise<IpcResult<GitStatus | null>> {
      return invokeIpc<GitStatus | null>(IPC_COMMANDS.GET_GIT_STATUS, { terminalId })
    },

    /**
     * Subscribe to exit code change events
     * Returns cleanup function (UnlistenFn)
     */
    onExitCodeChanged(callback: TerminalExitCodeChangedCallback): () => void {
      return subscribeSharedEvent(
        IPC_EVENTS.TERMINAL_EXIT_CODE_CHANGED,
        (payload) => {
          callback(payload.terminalId, payload.exitCode)
        },
        'terminal-exit-code-changed'
      )
    },

    /**
     * Get exit code for terminal
     */
    async getExitCode(terminalId: string): Promise<IpcResult<number | null>> {
      return invokeIpc<number | null>(IPC_COMMANDS.GET_EXIT_CODE, { terminalId })
    },

    /**
     * Update orphan detection settings
     */
    async updateOrphanDetection(
      enabled: boolean,
      timeout: number | null
    ): Promise<IpcResult<void>> {
      // Rust expects argument `settings: OrphanDetectionSettings`
      const settings = {
        enabled,
        timeoutMinutes: timeout ? Math.floor(timeout / 60000) : null
      }
      return invokeIpc<void>(IPC_COMMANDS.UPDATE_ORPHAN_DETECTION, { settings })
    }
  }
}

/**
 * Internal method to add renderer ref (not part of TerminalApi interface)
 * Called when a terminal component mounts to register with the Rust backend
 */
export async function addRendererRef(ptyId: string, rendererId: string): Promise<IpcResult<void>> {
  // Rust expects argument `request: RendererRefRequest { terminal_id, renderer_id }`
  const request = { terminalId: ptyId, rendererId }
  return invokeIpc<void>(IPC_COMMANDS.ADD_RENDERER_REF, { request })
}

/**
 * Internal method to remove renderer ref (not part of TerminalApi interface)
 * Called when a terminal component unmounts to unregister from the Rust backend
 */
export async function removeRendererRef(
  ptyId: string,
  rendererId: string
): Promise<IpcResult<void>> {
  // Rust expects argument `request: RendererRefRequest { terminal_id, renderer_id }`
  const request = { terminalId: ptyId, rendererId }
  return invokeIpc<void>(IPC_COMMANDS.REMOVE_RENDERER_REF, { request })
}

/**
 * Internal method to set a terminal's orphan-reaping protection (not part of
 * the TerminalApi interface).
 *
 * Protection is enabled automatically at spawn. Call with `protected = false`
 * ONLY when a terminal is genuinely released — its project is closed or its tab
 * is closed. Do NOT call this on a project switch or component unmount: a
 * backgrounded project's terminals must stay protected so orphan detection does
 * not kill them mid-task (the "Terminal not found"/hang bug).
 */
export async function setTerminalProtected(
  ptyId: string,
  protectedState: boolean
): Promise<IpcResult<void>> {
  // Rust expects argument `request: SetTerminalProtectedRequest { terminal_id, protected }`
  const request = { terminalId: ptyId, protected: protectedState }
  return invokeIpc<void>(IPC_COMMANDS.SET_PROTECTED, { request })
}

/**
 * Expose spawn tracking for debugging
 * Access via: window.__SE_SPAWN_TRACKER__
 */
if (typeof window !== 'undefined' && IS_DEV) {
  const globalDebug = window as unknown as Record<string, unknown>
  globalDebug.__SE_SPAWN_TRACKER__ = {
    getCalls: () => [...SPAWN_CALLS],
    getCallCount: () => SPAWN_CALLS.length,
    clearCalls: () => {
      SPAWN_CALLS.length = 0
    },
    getLastNCalls: (n: number) => SPAWN_CALLS.slice(-n),
    printSummary: () => {
      console.table(
        SPAWN_CALLS.map((c) => ({
          id: c.id,
          time: new Date(c.timestamp).toISOString().split('T')[1].slice(0, 12),
          shell: c.shell || 'N/A',
          cwd: c.cwd || 'N/A',
          caller: c.stack.split(' <- ')[0] || 'unknown'
        }))
      )
      devLog(`Total spawn calls: ${SPAWN_CALLS.length}`)

      // Detect potential loops
      if (SPAWN_CALLS.length >= 5) {
        const last5 = SPAWN_CALLS.slice(-5)
        const timeSpan = last5[4].timestamp - last5[0].timestamp
        if (timeSpan < 2000) {
          console.error('🚨 POTENTIAL SPAWN LOOP DETECTED 🚨')
          console.error('5 spawns within', timeSpan, 'ms')
          console.table(last5)
        }
      }
    }
  }
}
