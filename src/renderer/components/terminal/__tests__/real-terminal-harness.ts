import { type ITerminalOptions, Terminal } from '@xterm/xterm'

/**
 * Test-only observability primitives for the two signals this bug touches.
 *
 * `Terminal.prototype.open` is deliberately NEVER called here, and no
 * `try/catch` around it may be added later as a workaround. Under jsdom
 * `HTMLCanvasElement.prototype.getContext('2d')` returns null and
 * `OffscreenCanvas` is undefined (the `canvas` npm package is not a
 * devDependency), so xterm 6.1's renderer construction throws
 * `value must not be falsy` from inside `open()` — the same limitation
 * `terminal-performance.bench.test.ts` guards with `describe.skipIf(!hasCanvas)`.
 *
 * The trap, measured in run-e1acbf005b69/work/probe-evidence.md Probe 2: it
 * throws AFTER the DOM subtree has been built, so
 * `terminal.element.querySelector('.xterm-screen')` is still FOUND on the
 * failure path. Such an element belongs to a half-initialised terminal with no
 * renderer, and must never be used as a fixture. That is why the will-change
 * fixture below is hand-built instead of recovered from a terminal.
 *
 * A constructed-but-unopened Terminal is enough for the unicode axis:
 * `UnicodeService` is a core service and needs no DOM and no renderer.
 */

export interface RealTerminalHarness {
  terminal: Terminal
  readActiveUnicodeVersion(): string
  dispose(): void
}

/**
 * Construct a real `@xterm/xterm` Terminal without opening it.
 *
 * Reading `terminal.unicode` requires `allowProposedApi: true`; without it the
 * getter throws `You must set the allowProposedApi option to true to use
 * proposed API`, which `readActiveUnicodeVersion` deliberately propagates.
 */
export function createRealTerminalHarness(options: ITerminalOptions = {}): RealTerminalHarness {
  const terminal = new Terminal({ rows: 24, cols: 80, ...options })
  return {
    terminal,
    readActiveUnicodeVersion: (): string => terminal.unicode.activeVersion,
    dispose: (): void => terminal.dispose()
  }
}

export interface XtermScreenFixture {
  host: HTMLElement
  screen: HTMLElement
  readInlineWillChange(): string
  readComputedWillChange(): string
  dispose(): void
}

/**
 * A hand-built, document-attached `.terminal.xterm > .xterm-screen` shape.
 *
 * Attaching to `document.body` is what makes `getComputedStyle` observe the CSS
 * cascade, so a compositor promotion reintroduced from a stylesheet is caught
 * too — an inline-style assertion alone cannot see that.
 */
export function createXtermScreenFixture(): XtermScreenFixture {
  const host = document.createElement('div')
  host.className = 'terminal xterm'
  const screen = document.createElement('div')
  screen.className = 'xterm-screen'
  host.appendChild(screen)
  document.body.appendChild(host)

  return {
    host,
    screen,
    readInlineWillChange: (): string => screen.style.willChange,
    readComputedWillChange: (): string => window.getComputedStyle(screen).willChange,
    dispose: (): void => host.remove()
  }
}
