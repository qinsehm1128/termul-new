import type { TerminalInputModes } from '@shared/types/terminal-screen.types'
import { Terminal } from '@xterm/xterm'
import { afterEach, describe, expect, it } from 'vitest'
import {
  applyInputModesToRenderer,
  buildInputModeResyncSequence,
  hostMouseTrackingName
} from './terminal-input-modes'

/** Real xterm parser and public modes API; no DOM, no mocked callbacks. */
describe('input mode resync against a real xterm', () => {
  const terminals: Terminal[] = []
  const create = (): Terminal => {
    const terminal = new Terminal({ allowProposedApi: true })
    terminals.push(terminal)
    return terminal
  }
  const write = (terminal: Terminal, data: string): Promise<void> =>
    new Promise((resolve) => terminal.write(data, resolve))

  afterEach(() => {
    for (const terminal of terminals.splice(0)) terminal.dispose()
  })

  const crosstermCapture: TerminalInputModes = {
    mouseTracking: 1003,
    mouseEncoding: 1006,
    bracketedPaste: true,
    applicationCursor: false,
    focusEvents: true,
    applicationKeypad: false,
    alternateScreen: true
  }
  const plainShell: TerminalInputModes = {
    mouseTracking: null,
    mouseEncoding: null,
    bracketedPaste: true,
    applicationCursor: false,
    focusEvents: false,
    applicationKeypad: false,
    alternateScreen: false
  }

  it('re-takes mouse capture in a fresh renderer that missed the program startup', async () => {
    const terminal = create()
    expect(terminal.modes.mouseTrackingMode).toBe('none')
    await applyInputModesToRenderer(terminal, crosstermCapture)
    expect(terminal.modes.mouseTrackingMode).toBe('any')
    expect(terminal.modes.bracketedPasteMode).toBe(true)
    expect(terminal.modes.sendFocusMode).toBe(true)
  })

  it('releases a stale capture when the program already turned mouse reporting off', async () => {
    const terminal = create()
    await write(terminal, '\x1b[?1002h\x1b[?1006h\x1b[?1h\x1b=')
    expect(terminal.modes.mouseTrackingMode).toBe('drag')
    await applyInputModesToRenderer(terminal, plainShell)
    expect(terminal.modes.mouseTrackingMode).toBe('none')
    expect(terminal.modes.applicationCursorKeysMode).toBe(false)
    expect(terminal.modes.applicationKeypadMode).toBe(false)
  })

  it('switches protocol to the host one and never toggles the screen buffer', async () => {
    const terminal = create()
    await write(terminal, '\x1b[?1000h')
    expect(buildInputModeResyncSequence(crosstermCapture)).not.toMatch(/\?(1049|1047|47)[hl]/)
    await applyInputModesToRenderer(terminal, crosstermCapture)
    expect(terminal.modes.mouseTrackingMode).toBe('any')
    expect(terminal.buffer.active.type).toBe('normal')
  })

  it('is idempotent', async () => {
    const terminal = create()
    await applyInputModesToRenderer(terminal, crosstermCapture)
    await applyInputModesToRenderer(terminal, crosstermCapture)
    expect(terminal.modes.mouseTrackingMode).toBe('any')
  })

  it('names host protocols the way xterm does', () => {
    expect(hostMouseTrackingName(1000)).toBe('vt200')
    expect(hostMouseTrackingName(1002)).toBe('drag')
    expect(hostMouseTrackingName(1003)).toBe('any')
    expect(hostMouseTrackingName(null)).toBe('none')
  })
})
