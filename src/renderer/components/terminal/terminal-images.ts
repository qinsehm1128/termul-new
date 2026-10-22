import { type IImageAddonOptions, ImageAddon } from '@xterm/addon-image'
import type { Terminal } from '@xterm/xterm'

/**
 * Decoded images a single terminal may keep, in MB. The addon's default is
 * 128 MB per terminal; with a dozen or more terminals open that alone can push
 * the WebView past WebKit's per-page memory limit, which kills the page. Past
 * the cap the oldest images turn into a placeholder.
 */
export const TERMINAL_IMAGE_STORAGE_LIMIT_MB = 32

export const TERMINAL_IMAGE_OPTIONS: IImageAddonOptions = {
  storageLimit: TERMINAL_IMAGE_STORAGE_LIMIT_MB,
  sixelSupport: true,
  iipSupport: true,
  // Off: the addon only takes Kitty images sent inline, and ratatui-image's
  // Kitty path draws through Unicode placeholders xterm.js is not known to
  // support. Answering the Kitty query would make such programs pick a path
  // that shows nothing, instead of Sixel or iTerm2 images that work.
  kittySupport: false
}

const addons = new WeakMap<Terminal, ImageAddon>()

/**
 * Show Sixel and iTerm2 inline images in `terminal`, or stop showing them.
 * Idempotent, so a terminal restored from the cache can be synced again.
 */
export function setTerminalImagesEnabled(terminal: Terminal, enabled: boolean): void {
  const current = addons.get(terminal)
  if (enabled === Boolean(current)) return
  if (current) {
    addons.delete(terminal)
    current.dispose()
    return
  }
  const addon = new ImageAddon(TERMINAL_IMAGE_OPTIONS)
  terminal.loadAddon(addon)
  addons.set(terminal, addon)
}
