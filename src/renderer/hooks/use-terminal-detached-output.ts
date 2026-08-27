import { useEffect } from 'react'
import { peekCachedTerminal, setCacheEvictionHandler } from '@/components/terminal/terminal-cache'
import { terminalApi } from '@/lib/api'
import { logFrontendError } from '@/lib/log-api'
import {
  MAX_TRANSCRIPT_CHARS,
  rendererOwnsDetachedContinuity,
  useTerminalStore
} from '@/stores/terminal-store'
import type { Terminal } from '@/types/project'
import {
  buildScrollbackRestorePayload,
  extractScrollbackFromTerminal
} from '@/utils/terminal-registry'

const IS_DEV = import.meta.env.DEV

/**
 * Tracks transcript buffer sizes and logs warnings when they grow large.
 * Helps diagnose memory growth issues during development.
 */
function logTranscriptStats(ptyId: string, dataLen: number, totalTranscriptLen: number): void {
  if (!IS_DEV) return

  // Log at every crossed 100KB increment to avoid missing milestones
  const kb = Math.floor(totalTranscriptLen / 1024)
  const prevKb = Math.floor((totalTranscriptLen - dataLen) / 1024)
  if (kb > 0 && Math.floor(kb / 100) !== Math.floor(prevKb / 100)) {
    console.debug(
      `[MemTrack] transcript pty=${ptyId.slice(0, 12)} size=${kb}KB ` +
        `(${((totalTranscriptLen / MAX_TRANSCRIPT_CHARS) * 100).toFixed(1)}% of cap)`
    )
  }
}

/**
 * Captures PTY output only when a renderer-side replay buffer is actually needed.
 * This is exclusively for the detached-terminal case — when no ConnectedTerminal
 * renderer is mounted (e.g. project switch, pane removal). In that situation the
 * data is needed to reconstruct terminal continuity when the user returns.
 *
 * When a renderer IS attached (even if the app window is hidden/minimized), the
 * transcript should NOT capture because:
 *   - xterm.js already has the pre-hide buffer in its own internal state
 *   - replaying the transcript on restore is synchronous and blocks the main thread
 *   - the renderer resumes receiving live data immediately on restore
 */
export function useTerminalDetachedOutput(): void {
  useEffect(() => {
    // Buffer for PTY data that arrives before the store has the terminal record
    // (e.g. between spawn and setTerminalPtyId populating the ptyIdIndex).
    const pendingDetachedBuffer = new Map<string, string[]>()
    // PTY read boundaries are unrelated to UTF-8 character boundaries, so a
    // multi-byte sequence split across two chunks degrades to U+FFFD on both
    // sides unless one decoder carries the partial sequence across events.
    const decoders = new Map<string, TextDecoder>()
    const getDecoder = (ptyId: string): TextDecoder => {
      let decoder = decoders.get(ptyId)
      if (!decoder) {
        // Not `{ fatal: true }`: a fatal decoder throws on genuinely invalid
        // bytes, turning a cosmetic problem into a throw inside an event callback.
        decoder = new TextDecoder('utf-8')
        decoders.set(ptyId, decoder)
      }
      return decoder
    }

    // Evicting a sink would otherwise drop everything written into it since it
    // was cached — the exact span the sink exists to keep. Serialise the buffer
    // back into the transcript on the way out: the instance is gone, so the
    // transcript becomes the only carrier again, and the cold-restore path
    // already knows how to replay it. Post-eviction chunks append after this,
    // in order, because `peekCachedTerminal` now misses for that PTY.
    setCacheEvictionHandler((ptyId, session) => {
      const store = useTerminalStore.getState()
      // `getTerminalModes` is deliberately not consulted: unmount unregisters the
      // terminal and stops its mode tracker, so by eviction the live tracker is
      // always gone. `pendingModes` is the surviving snapshot.
      const payload = buildScrollbackRestorePayload(
        extractScrollbackFromTerminal(session.terminal),
        store.findTerminalByPtyId(ptyId)?.pendingModes
      )
      if (payload) {
        store.appendTranscript(ptyId, payload)
      }
    })

    /**
     * Route detached output to whichever sink can hold it without a later splice.
     *
     * Preferred sink is the cached xterm itself. It is detached from the DOM but
     * otherwise fully alive — `write()` is scheduled on timers and microtasks,
     * and only the paint depends on the element — so the bytes land in exactly
     * the structure they would have if the user had been watching, and ageing
     * out is xterm's own scrollback ring. Nothing accumulates that has to be
     * spliced onto a live screen later, which is where the detached interval
     * was being lost outright: a splice that looked unsafe was thrown away
     * whole, and the user came back to the frame they had left.
     *
     * The transcript stays the sink in the two cases where the cached instance
     * cannot be the authority: there is no cached instance (cold restore, or
     * evicted from the LRU), or the host will replay this same interval on
     * reattach and writing both would duplicate whole blocks of output.
     */
    const routeDetachedOutput = (ptyId: string, terminal: Terminal, dataStr: string): void => {
      const store = useTerminalStore.getState()
      const cached = rendererOwnsDetachedContinuity(terminal)
        ? peekCachedTerminal(ptyId)
        : undefined

      if (cached) {
        try {
          // Whatever the transcript holds predates this instance becoming the
          // sink, so it has to go in first or the two halves land out of order.
          if (terminal.transcript) {
            const pending = store.consumeTranscript(ptyId)
            if (pending) cached.terminal.write(pending)
          }
          cached.terminal.write(dataStr)
          return
        } catch (error) {
          // A disposed instance is the realistic failure. Falling through to the
          // transcript keeps the bytes; anything already written above is older
          // than what the transcript now carries, so the order still holds.
          void logFrontendError({
            level: 'warn',
            source: 'use-terminal-detached-output',
            message: `cached sink write failed ptyId=${ptyId}: ${
              error instanceof Error ? error.message : String(error)
            }`
          })
        }
      }

      store.appendTranscript(ptyId, dataStr)

      if (IS_DEV && terminal.transcript !== undefined) {
        logTranscriptStats(ptyId, dataStr.length, terminal.transcript.length + dataStr.length)
      }
    }

    const unsubscribe = terminalApi.onData((ptyId: string, data: Uint8Array) => {
      if (!data || data.length === 0) {
        return
      }

      // Decode EVERY event, including ones dropped further down. A retained
      // streaming decoder is a stateful cursor over the byte stream, so moving
      // this behind the rendererAttachmentCount gate below would desynchronise
      // it and corrupt the NEXT captured chunk.
      const dataStr = getDecoder(ptyId).decode(data, { stream: true })

      const buffered = pendingDetachedBuffer.get(ptyId)
      if (buffered) {
        pendingDetachedBuffer.delete(ptyId)
        const store = useTerminalStore.getState()
        const terminal = store.findTerminalByPtyId(ptyId)
        if (terminal && (terminal.rendererAttachmentCount ?? 0) === 0) {
          routeDetachedOutput(ptyId, terminal, buffered.join('') + dataStr)
          return
        }
        // Terminal exists but has renderer attached — drop buffered data
      }

      const store = useTerminalStore.getState()
      const terminal = store.findTerminalByPtyId(ptyId)
      if (!terminal) {
        // Store record not yet available — buffer data until it is
        if (IS_DEV) {
          console.debug(
            `[DetachedOutput] Buffering data for unknown PTY pty=${ptyId.slice(0, 12)} len=${dataStr.length}`
          )
        }
        const existing = pendingDetachedBuffer.get(ptyId)
        if (existing) {
          existing.push(dataStr)
        } else {
          pendingDetachedBuffer.set(ptyId, [dataStr])
        }
        return
      }

      const rendererAttachmentCount = terminal.rendererAttachmentCount ?? 0

      // Only capture when truly detached — no renderer mounted.
      // Do NOT capture when the app is hidden but a renderer is attached.
      if (rendererAttachmentCount > 0) {
        return
      }

      routeDetachedOutput(ptyId, terminal, dataStr)
    })

    // Release a closed PTY's decoder immediately rather than at app unmount.
    const unsubscribeExit = terminalApi.onExit((ptyId: string) => {
      decoders.delete(ptyId)
      pendingDetachedBuffer.delete(ptyId)
    })

    return () => {
      unsubscribe()
      unsubscribeExit()
      setCacheEvictionHandler(null)
      pendingDetachedBuffer.clear()
      decoders.clear()
    }
  }, [])
}
