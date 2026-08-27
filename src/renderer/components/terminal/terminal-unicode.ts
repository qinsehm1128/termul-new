import { Unicode11Addon } from '@xterm/addon-unicode11'
import type { Terminal } from '@xterm/xterm'
import { logFrontendError } from '@/lib/log-api'

/**
 * The single place in the renderer allowed to touch xterm's proposed API (R-04).
 *
 * ACCEPTANCE BOUNDARY (R-06): `@xterm/addon-unicode11` supplies Unicode v11
 * WIDTH TABLES ONLY. ZWJ sequences and grapheme clusters are NOT covered, and
 * adding a grapheme provider is explicitly out of scope for this change. A
 * residual symptom involving emoji or ZWJ is a separate decision, not a reason
 * to widen this module.
 */

export const TERMINAL_UNICODE_VERSION = '11'

/**
 * Activate Unicode v11 width tables on a terminal.
 *
 * The three statements are ordered and each is load-bearing:
 *  1. `allowProposedApi` must be true or the `unicode` getter throws. Cached
 *     terminals were constructed before this option existed on some code paths,
 *     so it is re-asserted at runtime rather than trusted from construction.
 *  2. `loadAddon` registers v11, taking `unicode.versions` from `['6']` to
 *     `['6', '11']` — but it does NOT switch the active version, because
 *     xterm's UnicodeService only adopts a version when none is seated and
 *     CoreTerminal already seated v6 during construction.
 *  3. The explicit assignment is therefore required, and must come strictly
 *     after `loadAddon`: the setter throws `unknown Unicode version "11"` for
 *     a version that is not registered yet.
 *
 * Idempotent by construction, so the every-mount call site cannot re-register
 * the addon or leak a second one across project switches.
 */
export function ensureTerminalUnicode11(terminal: Terminal): void {
  try {
    terminal.options.allowProposedApi = true
    if (!terminal.unicode.versions.includes(TERMINAL_UNICODE_VERSION)) {
      terminal.loadAddon(new Unicode11Addon())
    }
    if (terminal.unicode.activeVersion !== TERMINAL_UNICODE_VERSION) {
      terminal.unicode.activeVersion = TERMINAL_UNICODE_VERSION
    }
  } catch (error) {
    void logFrontendError({
      level: 'warn',
      source: 'ConnectedTerminal:terminal-unicode',
      message: `unicode v${TERMINAL_UNICODE_VERSION} activation failed: ${
        error instanceof Error ? error.message : String(error)
      }`,
      stack: error instanceof Error ? error.stack : undefined
    })
  }
}
