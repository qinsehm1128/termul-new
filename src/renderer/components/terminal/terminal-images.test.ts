import { Terminal } from '@xterm/xterm'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { setTerminalImagesEnabled, TERMINAL_IMAGE_OPTIONS } from './terminal-images'

/** The DA1 reply `terminal` sends for a `CSI c` query. */
async function primaryDeviceAttributes(terminal: Terminal): Promise<string> {
  let reply = ''
  const listener = terminal.onData((data) => {
    reply += data
  })
  await new Promise<void>((resolve) => terminal.write('\x1b[c', resolve))
  listener.dispose()
  return reply
}

describe('setTerminalImagesEnabled', () => {
  let terminal: Terminal | undefined

  afterEach(() => {
    terminal?.dispose()
    terminal = undefined
  })

  it('makes the terminal answer DA1 with Sixel support, and stops on disable', async () => {
    terminal = new Terminal()
    expect(await primaryDeviceAttributes(terminal)).toBe('\x1b[?1;2c')

    setTerminalImagesEnabled(terminal, true)
    // The same literal the Terminal Core answers first (se-pty da_filter.rs).
    expect(await primaryDeviceAttributes(terminal)).toBe('\x1b[?62;4;9;22c')

    setTerminalImagesEnabled(terminal, false)
    expect(await primaryDeviceAttributes(terminal)).toBe('\x1b[?1;2c')
  })

  it('loads the addon once however often it is synced', async () => {
    terminal = new Terminal()
    const loadAddon = vi.spyOn(terminal, 'loadAddon')
    // A cached terminal is synced again on every remount.
    setTerminalImagesEnabled(terminal, true)
    setTerminalImagesEnabled(terminal, true)
    expect(loadAddon).toHaveBeenCalledTimes(1)
    expect(await primaryDeviceAttributes(terminal)).toBe('\x1b[?62;4;9;22c')
  })

  it('caps image memory per terminal and keeps Kitty off', () => {
    expect(TERMINAL_IMAGE_OPTIONS.storageLimit).toBe(32)
    expect(TERMINAL_IMAGE_OPTIONS.kittySupport).toBe(false)
    expect(TERMINAL_IMAGE_OPTIONS.sixelSupport).toBe(true)
    expect(TERMINAL_IMAGE_OPTIONS.iipSupport).toBe(true)
  })
})
