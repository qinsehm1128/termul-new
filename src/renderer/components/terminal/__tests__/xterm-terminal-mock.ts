import { vi } from 'vitest'

/**
 * The single `@xterm/xterm` Terminal stub shared by every ConnectedTerminal
 * suite.
 *
 * Before this module the stub set was duplicated in ConnectedTerminal.test.tsx
 * and ConnectedTerminal.contextmenu.test.tsx and had already drifted (neither
 * carried a `unicode` member). One factory turns a missing stub member into a
 * single-point failure instead of a silent per-suite divergence.
 *
 * The factory is memoised per module instance so a `vi.mock('@xterm/xterm')`
 * factory and the suite body observe the same `instance` and `handles`.
 * A suite that calls `vi.resetModules()` gets a fresh, independent set.
 */

export interface XtermTerminalMockLinkProvider {
  provideLinks: (
    y: number,
    callback: (
      links: Array<{
        activate: (event: MouseEvent, text: string) => void | Promise<void>
        text: string
      }>
    ) => void
  ) => void
}

export interface XtermTerminalMockHandles {
  constructorSpy: ReturnType<typeof vi.fn>
  dataCallback: ((data: string) => void) | null
  resizeCallback: ((dims: { cols: number; rows: number }) => void) | null
  scrollCallback: (() => void) | null
  linkProviders: XtermTerminalMockLinkProvider[]
  /**
   * Chunks handed to `write` whose parse callback has not fired yet — the
   * stub's stand-in for xterm's `_writeBuffer` depth.
   */
  pendingWrites: number
  /**
   * `pendingWrites` sampled at each `refresh()` call. A non-zero sample means
   * the repaint ran while chunks were still unparsed, i.e. it was armed
   * straight after `write()` instead of from the parse callback.
   */
  refreshPendingWrites: number[]
}

export interface XtermTerminalMock {
  Terminal: new (options?: Record<string, unknown>) => Record<string, unknown>
  instance: ReturnType<typeof buildInstance>
  handles: XtermTerminalMockHandles
}

function buildInstance(handles: XtermTerminalMockHandles) {
  return {
    loadAddon: vi.fn(),
    registerLinkProvider: vi.fn((provider: XtermTerminalMockLinkProvider) => {
      handles.linkProviders.push(provider)
      return { dispose: vi.fn() }
    }),
    open: vi.fn(),
    onData: vi.fn<(_cb: (data: string) => void) => { dispose: () => void }>((cb) => {
      handles.dataCallback = cb
      return { dispose: vi.fn() }
    }),
    onResize: vi.fn<
      (_cb: (dims: { cols: number; rows: number }) => void) => { dispose: () => void }
    >((cb) => {
      handles.resizeCallback = cb
      return { dispose: vi.fn() }
    }),
    onSelectionChange: vi.fn(() => ({ dispose: vi.fn() })),
    onScroll: vi.fn<(_cb: () => void) => { dispose: () => void }>((cb) => {
      handles.scrollCallback = cb
      return { dispose: vi.fn() }
    }),
    attachCustomKeyEventHandler: vi.fn(),
    hasSelection: vi.fn(() => false),
    getSelection: vi.fn(() => ''),
    selectAll: vi.fn(),
    paste: vi.fn(),
    // Real xterm buffers the chunk and parses it on a later macrotask
    // (`WriteBuffer._scheduleInnerWrite` uses `setTimeout(..., 0)`), invoking
    // the callback only once the buffer holds the new content. Firing it
    // synchronously here would make "repaint from the parse callback" and
    // "repaint straight after write()" indistinguishable to every assertion in
    // the suite, which is how the timing invariant ended up untested.
    write: vi.fn((_data: unknown, callback?: () => void) => {
      handles.pendingWrites += 1
      setTimeout(() => {
        handles.pendingWrites -= 1
        callback?.()
      }, 0)
    }),
    clear: vi.fn(),
    focus: vi.fn(),
    refresh: vi.fn(() => {
      handles.refreshPendingWrites.push(handles.pendingWrites)
    }),
    scrollToLine: vi.fn(),
    dispose: vi.fn(),
    cols: 80,
    rows: 24,
    options: {} as Record<string, unknown>,
    // The real Terminal only exposes `unicode` behind allowProposedApi; the
    // stub always exposes it so a suite can observe the version handshake.
    unicode: { versions: ['6'], activeVersion: '6', register: vi.fn() },
    buffer: {
      active: {
        // Real xterm always reports one or the other. Omitting it made every
        // alternate-screen branch read `undefined` and silently take the
        // normal-buffer path, so the branch looked covered and was not.
        type: 'normal' as 'normal' | 'alternate',
        getLine: vi.fn((index: number) => ({
          translateToString: () => (index === 0 ? 'missing.ts src/renderer/App.tsx' : '')
        }))
      }
    },
    // The real Terminal exposes its RenderService here; the leftover-glyph
    // repair reaches through it to invalidate the renderer's cell model.
    _core: { _renderService: { clear: vi.fn() } },
    // Real DOM element so recovery's CSS visibility re-composite can be tested.
    element: (typeof document !== 'undefined' ? document.createElement('div') : undefined) as
      | HTMLDivElement
      | undefined,
    // Real element too: the keyCode-229 guard reads `.value` to tell "the
    // browser has not inserted this key yet" from "an IME already did".
    textarea: (typeof document !== 'undefined' ? document.createElement('textarea') : undefined) as
      | HTMLTextAreaElement
      | undefined
  }
}

let shared: XtermTerminalMock | null = null

export function createXtermTerminalMock(): XtermTerminalMock {
  if (shared) return shared

  const handles: XtermTerminalMockHandles = {
    constructorSpy: vi.fn(),
    dataCallback: null,
    resizeCallback: null,
    scrollCallback: null,
    linkProviders: [],
    pendingWrites: 0,
    refreshPendingWrites: []
  }
  const instance = buildInstance(handles)

  class MockTerminal {
    constructor(options?: Record<string, unknown>) {
      handles.constructorSpy(options)
    }
    loadAddon = instance.loadAddon
    registerLinkProvider = instance.registerLinkProvider
    open = instance.open
    onData = instance.onData
    onResize = instance.onResize
    onSelectionChange = instance.onSelectionChange
    onScroll = instance.onScroll
    attachCustomKeyEventHandler = instance.attachCustomKeyEventHandler
    hasSelection = instance.hasSelection
    getSelection = instance.getSelection
    selectAll = instance.selectAll
    paste = instance.paste
    write = instance.write
    clear = instance.clear
    focus = instance.focus
    refresh = instance.refresh
    scrollToLine = instance.scrollToLine
    dispose = instance.dispose
    cols = instance.cols
    rows = instance.rows
    options = instance.options
    unicode = instance.unicode
    buffer = instance.buffer
    _core = instance._core
    element = instance.element
    textarea = instance.textarea
  }

  shared = {
    Terminal: MockTerminal as unknown as XtermTerminalMock['Terminal'],
    instance,
    handles
  }
  return shared
}
