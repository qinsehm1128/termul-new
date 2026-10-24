import { Terminal } from '@xterm/xterm'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import {
  buildRehydrateSequences,
  clearRegistry,
  getTerminalModes,
  startTrackingModes
} from './terminal-registry'

/**
 * These tests intentionally use the real xterm parser and public modes API.
 * They do not open a DOM terminal and do not mock parser callbacks.
 */
describe('real xterm mouse protocol tracking', () => {
  const terminals: Terminal[] = []

  const createTerminal = (): Terminal => {
    const terminal = new Terminal({ allowProposedApi: true })
    terminals.push(terminal)
    return terminal
  }

  const write = (terminal: Terminal, data: string): Promise<void> =>
    new Promise((resolve) => terminal.write(data, resolve))

  beforeEach(() => {
    clearRegistry()
  })

  afterEach(() => {
    for (const terminal of terminals.splice(0)) terminal.dispose()
    clearRegistry()
  })

  it.each([
    ['vt200', '\x1b[?1000h', 'vt200', 'x10'],
    ['drag', '\x1b[?1002h', 'drag', 'drag'],
    ['any', '\x1b[?1003h', 'any', 'any']
  ] as const)('captures %s from the real parser and rehydrates it', async (_name, sequence, expected, dtoExpected) => {
    const source = createTerminal()
    startTrackingModes('real-mouse', source)
    await write(source, sequence)

    expect(source.modes.mouseTrackingMode).toBe(expected)
    expect(getTerminalModes('real-mouse')?.mouseTracking).toBe(dtoExpected)

    for (let attempt = 0; attempt < 3; attempt += 1) {
      const restored = createTerminal()
      await write(restored, buildRehydrateSequences(getTerminalModes('real-mouse')))
      expect(restored.modes.mouseTrackingMode).toBe(expected)
    }
  })

  it.each([
    ['vt200', '\x1b[?1000h', 'vt200', 'x10'],
    ['drag', '\x1b[?1002h', 'drag', 'drag'],
    ['any', '\x1b[?1003h', 'any', 'any']
  ] as const)('last mouse SET wins (%s)', async (_name, sequence, expected, dtoExpected) => {
    const terminal = createTerminal()
    startTrackingModes('last-set', terminal)
    await write(terminal, '\x1b[?1003h')
    await write(terminal, sequence)

    expect(terminal.modes.mouseTrackingMode).toBe(expected)
    expect(getTerminalModes('last-set')?.mouseTracking).toBe(dtoExpected)
  })

  it.each([
    ['vt200', '\x1b[?1000h', 'vt200', 'x10'],
    ['drag', '\x1b[?1002h', 'drag', 'drag'],
    ['any', '\x1b[?1003h', 'any', 'any']
  ] as const)('captures active %s before reset matrix', async (_name, sequence, expected, dtoExpected) => {
    const terminal = createTerminal()
    startTrackingModes(`matrix-${_name}`, terminal)
    await write(terminal, sequence)
    expect(terminal.modes.mouseTrackingMode).toBe(expected)
    expect(getTerminalModes(`matrix-${_name}`)?.mouseTracking).toBe(dtoExpected)
  })

  it.each([
    ['vt200', '\x1b[?1000h', 'vt200'],
    ['drag', '\x1b[?1002h', 'drag'],
    ['any', '\x1b[?1003h', 'any']
  ] as const)('clears active %s for every mouse reset', async (_name, sequence, expected) => {
    for (const resetMode of [9, 1000, 1002, 1003] as const) {
      const terminal = createTerminal()
      const id = `reset-${_name}-${resetMode}`
      startTrackingModes(id, terminal)
      await write(terminal, sequence)
      expect(terminal.modes.mouseTrackingMode).toBe(expected)
      await write(terminal, `\x1b[?${resetMode}l`)

      expect(terminal.modes.mouseTrackingMode).toBe('none')
      expect(getTerminalModes(id)?.mouseTracking).toBe(null)

      const restored = createTerminal()
      await write(restored, buildRehydrateSequences(getTerminalModes(id)))
      expect(restored.modes.mouseTrackingMode).toBe('none')
    }
  })

  it('keeps a fresh terminal snapshot at none and does not rehydrate drag', async () => {
    const source = createTerminal()
    startTrackingModes('fresh-none', source)

    expect(source.modes.mouseTrackingMode).toBe('none')
    expect(getTerminalModes('fresh-none')?.mouseTracking).toBe(null)

    const restored = createTerminal()
    await write(restored, buildRehydrateSequences(getTerminalModes('fresh-none')))
    expect(restored.modes.mouseTrackingMode).toBe('none')
  })

  it('clears tracked modes on RIS and lets xterm reset the real terminal', async () => {
    const terminal = createTerminal()
    startTrackingModes('ris', terminal)
    await write(terminal, '\x1b[?1002h\x1b[?1006h')
    await write(terminal, '\x1bc')

    expect(terminal.modes.mouseTrackingMode).toBe('none')
    expect(getTerminalModes('ris')).toEqual({
      alternateScreen: false,
      bracketedPaste: false,
      applicationCursor: false,
      mouseTracking: null,
      sgrMouseMode: false,
      sgrMousePixelsMode: false
    })
  })

  it('tracks SGR encoding independently while preserving drag protocol', async () => {
    const terminal = createTerminal()
    startTrackingModes('sgr-real', terminal)
    await write(terminal, '\x1b[?1002h\x1b[?1006h')

    expect(terminal.modes.mouseTrackingMode).toBe('drag')
    expect(getTerminalModes('sgr-real')).toEqual(
      expect.objectContaining({ mouseTracking: 'drag', sgrMouseMode: true })
    )

    await write(terminal, '\x1b[?1016h')
    expect(getTerminalModes('sgr-real')).toEqual(
      expect.objectContaining({
        mouseTracking: 'drag',
        sgrMouseMode: false,
        sgrMousePixelsMode: true
      })
    )

    await write(terminal, '\x1b[?1016l')
    expect(getTerminalModes('sgr-real')).toEqual(
      expect.objectContaining({
        mouseTracking: 'drag',
        sgrMouseMode: false,
        sgrMousePixelsMode: false
      })
    )
  })

  it('leaves mouse selection options false by default', () => {
    const terminal = createTerminal()
    expect(terminal.options.macOptionClickForcesSelection).toBe(false)
    expect(terminal.options.mouseEventsRequireAlt).toBe(false)
  })
})
