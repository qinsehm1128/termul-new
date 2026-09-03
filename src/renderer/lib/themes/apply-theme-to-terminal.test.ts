/**
 * T-G1C — texture refresh on a theme change.
 *
 * Ruling R-G1C: the refresh is already performed upstream, and FORBID-01
 * forbids adding a `clearTextureAtlas` call site here. The chain this repo
 * relies on, none of which is asserted below because none of it is ours:
 *
 *   `terminal.options.theme = …`
 *     → `@xterm/xterm/src/common/services/OptionsService.ts:135` fires only on
 *       `rawOptions[key] !== value`, i.e. reference inequality;
 *     → `@xterm/xterm/src/browser/services/ThemeService.ts:73`
 *       `onSpecificOptionChange('theme')` → `_setTheme` → `onChangeColors`;
 *     → `@xterm/addon-webgl/src/WebglRenderer.ts:102` subscribes, and
 *       `:176-183` `_handleColorChange()` runs `_refreshCharAtlas()` plus a
 *       full `_clearModel(true)`.
 *
 * So the only thing this repo owns is the seam: assign a *fresh* object (a
 * re-assigned reference is a silent no-op at OptionsService.ts:135, and the
 * whole chain never starts) and compensate the DOM/canvas renderers with one
 * `refresh`. Baseline spec KDC-7afe22f64a0148d7: jsdom has no canvas backend,
 * so anything past this seam cannot be asserted here without faking evidence.
 */
import type { Terminal } from '@xterm/xterm'
import { describe, expect, it, vi } from 'vitest'
import { applyThemeToTerminal } from './apply-theme-to-terminal'

/** `clearTextureAtlas` is present purely so its absence can be asserted. */
function fakeTerminal(theme: object = {}): Terminal & {
  clearTextureAtlas: ReturnType<typeof vi.fn>
} {
  return {
    options: { theme },
    rows: 24,
    refresh: vi.fn(),
    clearTextureAtlas: vi.fn()
  } as unknown as Terminal & { clearTextureAtlas: ReturnType<typeof vi.fn> }
}

describe('applyThemeToTerminal', () => {
  it('sets options.theme and calls refresh', () => {
    const terminal = fakeTerminal()

    const theme = { background: '#111111', foreground: '#eeeeee' }
    applyThemeToTerminal(terminal, theme)

    expect(terminal.options.theme).toEqual(theme)
    expect(terminal.refresh).toHaveBeenCalledWith(0, 23)
  })

  it('assigns a fresh object, so the option change is never a silent no-op', () => {
    const terminal = fakeTerminal()
    const theme = { background: '#111111', foreground: '#eeeeee' }

    applyThemeToTerminal(terminal, theme)

    // Equal by value, distinct by reference: OptionsService compares with
    // `!==`, so handing it the caller's own object would fire nothing the
    // second time the same theme object came round.
    expect(terminal.options.theme).toEqual(theme)
    expect(terminal.options.theme).not.toBe(theme)
  })

  it('re-applying the same theme value still swaps the reference', () => {
    const terminal = fakeTerminal()
    const theme = { background: '#111111' }

    applyThemeToTerminal(terminal, theme)
    const first = terminal.options.theme
    applyThemeToTerminal(terminal, theme)

    expect(terminal.options.theme).not.toBe(first)
  })

  it('refreshes every row exactly once', () => {
    const terminal = fakeTerminal()

    applyThemeToTerminal(terminal, { background: '#111111' })

    expect(terminal.refresh).toHaveBeenCalledTimes(1)
    expect(terminal.refresh).toHaveBeenCalledWith(0, 23)
  })

  it('clamps the refresh range on a zero-row terminal', () => {
    const terminal = fakeTerminal()
    Object.defineProperty(terminal, 'rows', { value: 0 })

    applyThemeToTerminal(terminal, { background: '#111111' })

    expect(terminal.refresh).toHaveBeenCalledWith(0, 0)
  })

  // FORBID-01. The WebGL addon refreshes its own atlas off `onChangeColors`;
  // a call from here would additionally clear an atlas shared with sibling
  // terminals, leaving them with stale UVs (terminal-webgl-repair.ts:58).
  it('never clears the texture atlas', () => {
    const terminal = fakeTerminal()

    applyThemeToTerminal(terminal, { background: '#111111' })

    expect(terminal.clearTextureAtlas).not.toHaveBeenCalled()
  })
})
