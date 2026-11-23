import type { Terminal } from '@xterm/xterm'
import { logFrontendError } from './log-api'
import { isMac } from './platform'
import { isTauriContext } from './tauri-runtime'

const SAMPLE_INTERVAL_MS = 60_000
let installed = false

function logicalCells(terminal: Terminal): number {
  // Columns times rows is a comparison metric, not an allocation size. Do not
  // scan individual lines or read their text, especially during high pressure.
  return terminal.cols * (terminal.buffer.normal.length + terminal.buffer.alternate.length)
}

/**
 * App counters complement the native footprint samples. These are counts and
 * lengths, not estimates of heap size. Never serialize terminal or page data.
 */
export function installRendererMemoryMarkers(nonce: string): void {
  if (installed || !isTauriContext() || !isMac) return
  installed = true

  // Delay imports until initial rendering has settled. No browser client sends
  // these local machine diagnostics to its remote host.
  const start = async (): Promise<void> => {
    const [terminalStore, terminalCache, webglBudget, registry] = await Promise.all([
      import('@/stores/terminal-store'),
      import('@/components/terminal/terminal-cache'),
      import('./webgl-renderer-budget'),
      import('@/utils/terminal-registry')
    ])
    const sample = async (): Promise<void> => {
      const { terminals } = terminalStore.useTerminalStore.getState()
      let attached = 0
      let hidden = 0
      let transcriptChars = 0
      let detachedChars = 0
      let pendingScrollbackLines = 0
      for (const terminal of terminals) {
        if ((terminal.rendererAttachmentCount ?? 0) > 0) attached += 1
        if (terminal.isHidden || terminal.isAppHidden) hidden += 1
        transcriptChars += terminal.transcript?.length ?? 0
        detachedChars += terminal.detachedOutput?.length ?? 0
        pendingScrollbackLines += terminal.pendingScrollback?.length ?? 0
      }
      let mountedCells = 0
      let cachedCells = 0
      registry.forEachTerminal((terminal) => {
        mountedCells += logicalCells(terminal)
      })
      const cachedXterms = terminalCache.forEachCachedTerminal((terminal) => {
        cachedCells += logicalCells(terminal)
      })
      await logFrontendError({
        level: 'warn',
        source: 'renderer-memory',
        message:
          `event=sample nonce=${nonce} terminals=${terminals.length} attached=${attached} ` +
          `mounted_xterms=${registry.getRegistrySize()} mounted_cells=${mountedCells} ` +
          `cached_xterms=${cachedXterms} cached_cells=${cachedCells} webgl_slots=${webglBudget.countWebglSlots()} ` +
          `transcript_chars=${transcriptChars} detached_chars=${detachedChars} ` +
          `pending_scrollback_lines=${pendingScrollbackLines} hidden_terminals=${hidden}`
      })
    }
    const tick = (): void => {
      // Never queue another log request behind a blocked native transport.
      void sample().then(schedule, () => {
        void logFrontendError({
          level: 'warn',
          source: 'renderer-memory',
          message: `event=sample_failed nonce=${nonce}`
        }).then(schedule)
      })
    }
    const schedule = (): void => {
      setTimeout(tick, SAMPLE_INTERVAL_MS)
    }
    tick()
  }
  setTimeout(() => {
    void start().catch(() =>
      logFrontendError({
        level: 'warn',
        source: 'renderer-memory',
        message: `event=init_failed nonce=${nonce}`
      })
    )
  }, SAMPLE_INTERVAL_MS)
}
