import { afterEach, describe, expect, it, vi } from 'vitest'
import { logFrontendError } from '@/lib/log-api'
import {
  createRealTerminalHarness,
  type RealTerminalHarness
} from './__tests__/real-terminal-harness'
import { ensureTerminalUnicode11 } from './terminal-unicode'

vi.mock('@/lib/log-api', () => ({
  logFrontendError: vi.fn()
}))

/**
 * DOD-3 carrier. Asserts against a REAL `@xterm/xterm` Terminal — never a mock —
 * which needs no DOM, no `open()` and no renderer, because UnicodeService is a
 * core service.
 */
describe('ensureTerminalUnicode11', () => {
  let harness: RealTerminalHarness | null = null

  afterEach(() => {
    harness?.dispose()
    harness = null
    vi.clearAllMocks()
  })

  it('switches a v6 terminal to Unicode v11', () => {
    harness = createRealTerminalHarness({ allowProposedApi: true })
    expect(harness.terminal.unicode.activeVersion).toBe('6')
    expect(harness.terminal.unicode.versions).toEqual(['6'])

    ensureTerminalUnicode11(harness.terminal)

    // loadAddon alone only registers the version; the explicit assignment is
    // what actually switches it.
    expect(harness.terminal.unicode.versions).toEqual(['6', '11'])
    expect(harness.terminal.unicode.activeVersion).toBe('11')
  })

  it('is idempotent, so an every-mount call site cannot leak a second addon', () => {
    harness = createRealTerminalHarness({ allowProposedApi: true })

    ensureTerminalUnicode11(harness.terminal)
    ensureTerminalUnicode11(harness.terminal)

    expect(harness.terminal.unicode.versions).toEqual(['6', '11'])
    expect(harness.terminal.unicode.activeVersion).toBe('11')
  })

  it('activates on a terminal constructed without allowProposedApi', () => {
    // R-05: cached terminals may predate the constructor default, so the
    // function re-asserts the option at runtime instead of trusting it.
    harness = createRealTerminalHarness()

    ensureTerminalUnicode11(harness.terminal)

    expect(harness.terminal.unicode.activeVersion).toBe('11')
  })

  it('does not report a failure on the happy path', () => {
    harness = createRealTerminalHarness({ allowProposedApi: true })

    ensureTerminalUnicode11(harness.terminal)

    expect(logFrontendError).not.toHaveBeenCalled()
  })
})
