/**
 * WebGL texture-atlas corruption repro (xterm.js #5847 family).
 *
 * Loaded into a real WKWebView by `runner.swift`; `check.ts` drives it. The
 * query string picks the scenario; the page reports the captured terminal's
 * WebGL canvas as a PNG data URL, compared pixel-for-pixel against `reference`.
 *
 *   mode=stress|reference   render every chunk as it arrives, or write all of
 *                           them at once and render once (a clean atlas)
 *   scenario=scroll|tui|hidden
 *     scroll   prose in per-word fg+bg colours, then scroll back and forth
 *     tui      agent-CLI footer rewritten every frame in per-character colours
 *     hidden   terminal 2 runs a TUI, is hidden while terminal 1 churns the
 *              shared atlas, then is shown and restored — the reported bug
 *   screen=normal|alt       run the TUI on the alternate screen
 *   hide=visibility|display how terminal 2 is hidden (Se uses visibility)
 *   terms=1|2  capture=1|2  a second terminal sharing the atlas; which to capture
 *   hook=none|app           wire Se's terminal-webgl-repair as ConnectedTerminal does
 *   policy=current|legacy   legacy keeps the alternate-screen model after merges
 *   repair=none|model|textures|resize|atlas|sibling-atlas   applied at the end
 *   profile=app|vioh        Se's terminal options, or the upstream reporter's
 */
import { WebglAddon } from '@xterm/addon-webgl'
import { type ITerminalOptions, Terminal } from '@xterm/xterm'
import {
  clearWebglRenderModel,
  createWebglModelRebuildDetector,
  createWebglScrollRepair
} from '../../src/renderer/components/terminal/terminal-webgl-repair'

type Repair = 'none' | 'model' | 'textures' | 'resize' | 'atlas' | 'sibling-atlas'

const COLS = 120
const ROWS = 40

const params = new URLSearchParams(location.search)
const mode = params.get('mode') ?? 'stress'
const scenario = params.get('scenario') ?? 'scroll'
const repair = (params.get('repair') ?? 'none') as Repair
const profile = params.get('profile') ?? 'app'
const terms = Number(params.get('terms') ?? '1')
const frames = Number(params.get('frames') ?? '600')
// Which terminal to repair and capture: 1 renders first each frame, 2 second.
const capture = params.get('capture') ?? '1'
// hook=app wires Se's own repair exactly as ConnectedTerminal does.
const hook = params.get('hook') ?? 'none'
// screen=alt runs the scenario on the alternate screen, where full-screen agent
// TUIs live (and where Se's repair skips the model rebuild).
const screen = params.get('screen') ?? 'normal'
// scenario=hidden: terminal 2 runs its TUI, is hidden (hide=visibility, as Se's
// inactive tabs are, or hide=display) while terminal 1 keeps churning the shared
// atlas, then is shown again and restored the way ConnectedTerminal restores.
const hide = params.get('hide') ?? 'visibility'
// policy=legacy is the rebuild policy before the fix: the alternate screen kept
// its model even after an atlas merge. Kept as the scenario's positive control.
const policy = params.get('policy') ?? 'current'

// `app` mirrors DEFAULT_TERMINAL_OPTIONS in terminal-config.ts; `vioh` is the
// upstream reporter's configuration, which surfaces the bug sooner.
const PROFILES: Record<string, ITerminalOptions> = {
  app: {
    fontFamily: 'Menlo, monospace',
    fontSize: 14,
    lineHeight: 1,
    letterSpacing: 0,
    allowTransparency: false,
    rescaleOverlappingGlyphs: true,
    drawBoldTextInBrightColors: true,
    theme: { background: '#0a0b12', foreground: '#c8ccd8' }
  },
  vioh: {
    fontFamily: 'Menlo, monospace',
    fontSize: 11,
    lineHeight: 1.3,
    letterSpacing: 0.4,
    minimumContrastRatio: 1.15,
    allowTransparency: true,
    theme: { background: '#0a0b12', foreground: '#c8ccd8' }
  }
}

// Prose plus CJK, as in the agent CLI output where users hit the bug: the
// failure shows as a misspelt or swapped glyph.
const PHRASES = [
  'The quick brown fox jumps over the lazy dog while the tramway passes the old bureaux near the river',
  '需求侧增加开发周期 产品在需求中选择开发周期 非必填 开发任务自动跟随 减少逐任务修改',
  'Verifying current symlink and cross-references before the release build moves the reference files',
  '测试提前看到需求 标记需要测试后就进入测试池 不再等全部开发任务完成才可见',
  'Lumières aussitôt le matin, les bureaux ouvrent et le tramway emmène les voyageurs vers la gare'
]

function post(message: Record<string, unknown>): void {
  const handler = (
    window as unknown as {
      webkit?: { messageHandlers?: { done?: { postMessage: (m: unknown) => void } } }
    }
  ).webkit?.messageHandlers?.done
  if (handler) handler.postMessage(message)
  else console.log(message)
}

const frame = (): Promise<void> => new Promise((resolve) => requestAnimationFrame(() => resolve()))
async function settle(count = 3): Promise<void> {
  for (let i = 0; i < count; i++) await frame()
}

function lcg(seed: number): () => number {
  let state = seed
  return () => {
    state = (state * 1103515245 + 12345) & 0x7fffffff
    return state
  }
}

function proseLines(count: number, seed: number): string[] {
  const next = lcg(seed)
  const lines: string[] = []
  for (let i = 0; i < count; i++) {
    const words = PHRASES[i % PHRASES.length].split(' ')
    const styled = words.map((word) => {
      const c = next()
      const f = next()
      const bold = next() % 3 === 0 ? '\x1b[1m' : ''
      return (
        `${bold}\x1b[38;2;${(c >> 16) & 255};${(c >> 8) & 255};${c & 255}m` +
        `\x1b[48;2;${(f >> 17) & 63};${(f >> 9) & 63};${(f >> 1) & 63}m${word}\x1b[22m`
      )
    })
    lines.push(`${String(i).padStart(4, '0')} ${styled.join(' ')}\x1b[0m\r\n`)
  }
  return lines
}

/** One frame of an agent-CLI footer: every character gets its own colour. */
function tuiFrame(f: number, salt: number): string {
  const gradient = (text: string, phase: number): string =>
    [...text]
      .map((ch, i) => {
        const t = (f * 7 + i * 13 + phase + salt) % 360
        const r = 128 + Math.round(127 * Math.sin((t * Math.PI) / 180))
        const g = 128 + Math.round(127 * Math.sin(((t + 120) * Math.PI) / 180))
        const b = 128 + Math.round(127 * Math.sin(((t + 240) * Math.PI) / 180))
        return `\x1b[38;2;${r};${g};${b}m${ch}`
      })
      .join('')
  const spinner = '⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏'[f % 10]
  const secs = `${Math.floor(f / 60)}m ${f % 60}s`
  const tokens = `${(f * 0.37).toFixed(1)}k`
  const bar = Array.from({ length: 30 }, (_, i) => {
    const level = (f * 3 + i * 9 + salt) % 256
    return `\x1b[48;2;${level};${255 - level};${(level * 7) % 256}m${i < (f % 30) ? '█' : '░'}`
  }).join('')
  const rows = [
    `${spinner} ${gradient('Gusting… 正在核对当前符号链接与交叉引用', 0)} \x1b[0m(${secs} · ↓ ${tokens} tokens)`,
    `  ${gradient('Verifying current symlink and cross-references · 需求汇总', 90)}`,
    `${gradient('────────────────────────────────────────────────────────────', 45)}`,
    `${gradient('> 测试提前看到需求 标记需要测试后就进入测试池', 180)}`,
    `\x1b[0mGPT ${gradient('think high', 30)} ↑${f}k ↓${f % 997} ${bar}\x1b[0m ${(f % 100).toString().padStart(2, '0')}%`,
    `${gradient(`qin-cpa · ~/ns-req-go · main · frame ${f}`, 270)}`
  ]
  const top = ROWS - rows.length + 1
  return rows.map((row, i) => `\x1b[${top + i};1H\x1b[2K${row}\x1b[0m`).join('')
}

function chunksFor(salt: number): string[] {
  if (scenario === 'tui' || scenario === 'hidden') {
    const enter = screen === 'alt' ? '\x1b[?1049h\x1b[H' : ''
    const chunks = [enter + proseLines(ROWS - 1, 7 + salt).join('')]
    for (let f = 0; f < frames; f++) chunks.push(tuiFrame(f, salt))
    return chunks
  }
  const lines = proseLines(1500, 1 + salt)
  const chunks: string[] = []
  for (let i = 0; i < lines.length; i += ROWS) chunks.push(lines.slice(i, i + ROWS).join(''))
  return chunks
}

type RendererHandle = {
  _charAtlas?: { pages?: { canvas: HTMLCanvasElement }[] }
  _glyphRenderer?: { value?: { invalidateAtlasTextures?: () => void } }
}
type CoreHandle = {
  _renderService?: { clear?: () => void; _renderer?: { value?: RendererHandle } }
}
const coreOf = (term: Terminal): CoreHandle => (term as unknown as { _core: CoreHandle })._core

async function applyRepair(term: Terminal, addon: WebglAddon): Promise<void> {
  const core = coreOf(term)
  switch (repair) {
    case 'none':
      return
    case 'model':
      // What terminal-webgl-repair.ts does today.
      core._renderService?.clear?.()
      term.refresh(0, term.rows - 1)
      return
    case 'textures':
      core._renderService?._renderer?.value?._glyphRenderer?.value?.invalidateAtlasTextures?.()
      core._renderService?.clear?.()
      term.refresh(0, term.rows - 1)
      return
    case 'resize':
      // What collapsing and reopening a side panel does.
      term.resize(COLS - 1, ROWS)
      await settle()
      term.resize(COLS, ROWS)
      return
    case 'atlas':
      addon.clearTextureAtlas()
      term.refresh(0, term.rows - 1)
      return
  }
}

type Mounted = {
  term: Terminal
  addon: WebglAddon
  write: (data: string) => Promise<void>
  restore: () => void
}

/** ConnectedTerminal's wiring: atlas events, onWriteParsed, in-place redraw detector, scroll. */
function wireAppRepair(
  term: Terminal,
  addon: WebglAddon
): { write: (data: string) => Promise<void>; restore: () => void } {
  const detector = createWebglModelRebuildDetector()
  let skipNextParsed = false
  const repairer = createWebglScrollRepair({
    getTerminal: () => term,
    // Same policy as ConnectedTerminal's rebuildSurface.
    rebuildSurface: (t, { atlasPagesMoved }) => {
      const keepModel = policy === 'legacy' || !atlasPagesMoved
      if (keepModel && term.buffer.active.type === 'alternate') return
      clearWebglRenderModel(t, true)
    }
  })
  addon.onAddTextureAtlasCanvas(() => repairer.markAtlasDirty())
  addon.onRemoveTextureAtlasCanvas(() => repairer.noteAtlasMerged())
  term.onWriteParsed(() => {
    if (skipNextParsed) {
      skipNextParsed = false
      return
    }
    repairer.onWrite(false)
  })
  term.onScroll(() => repairer.onScroll())
  // restoreVisibleTerminalSurface: what a tab becoming visible runs.
  const restore = (): void => repairer.repairNow()
  const write = (data: string): Promise<void> =>
    new Promise((resolve) => {
      const mayRebuild = detector.scan(data)
      term.write(data, () => {
        if (mayRebuild || detector.flush()) {
          skipNextParsed = true
          repairer.onWrite(true)
        }
        resolve()
      })
    })
  return { write, restore }
}

function mount(id: string): Mounted {
  const el = document.getElementById(id) as HTMLElement
  const term = new Terminal({ ...PROFILES[profile], cols: COLS, rows: ROWS, scrollback: 5000 })
  term.open(el)
  const addon = new WebglAddon({ preserveDrawingBuffer: true })
  const plainWrite = (data: string): Promise<void> =>
    new Promise((resolve) => term.write(data, resolve))
  const wired = hook === 'app' ? wireAppRepair(term, addon) : null
  term.loadAddon(addon)
  return {
    term,
    addon,
    write: wired?.write ?? plainWrite,
    restore: wired?.restore ?? (() => term.refresh(0, term.rows - 1))
  }
}

const write = (target: Mounted, data: string): Promise<void> => target.write(data)

const errors: string[] = []
window.addEventListener('error', (event) => errors.push(String(event.message)))
const consoleError = console.error.bind(console)
console.error = (...args: unknown[]) => {
  errors.push(args.map(String).join(' ').slice(0, 200))
  consoleError(...args)
}

async function main(): Promise<void> {
  const main = mount('terminal')
  const side = terms > 1 ? mount('terminal-2') : null
  const stats = { added: 0, merged: 0 }
  main.addon.onAddTextureAtlasCanvas(() => stats.added++)
  main.addon.onRemoveTextureAtlasCanvas(() => stats.merged++)
  await settle()

  const chunks = chunksFor(0)
  const sideChunks = side ? chunksFor(1000) : []
  if (mode === 'stress' && scenario === 'hidden' && side) {
    const host2 = document.getElementById('terminal-2') as HTMLElement
    const third = Math.floor(chunks.length / 3)
    // Phase 1: both visible, terminal 2's vertices fill with glyphs on many pages.
    for (let i = 0; i < third; i++) {
      await write(main, chunks[i])
      await write(side, sideChunks[i])
      await frame()
    }
    // Phase 2: terminal 2 hidden like an inactive tab; terminal 1 keeps churning.
    if (hide === 'display') host2.style.display = 'none'
    else host2.style.visibility = 'hidden'
    for (let i = third; i < chunks.length; i++) {
      await write(main, chunks[i])
      await write(side, sideChunks[i])
      await frame()
    }
    // Phase 3: shown again and restored.
    host2.style.display = ''
    host2.style.visibility = ''
    await frame()
    side.restore()
  } else if (mode === 'stress') {
    for (let i = 0; i < chunks.length; i++) {
      await write(main, chunks[i])
      if (side && sideChunks[i]) await write(side, sideChunks[i])
      await frame()
    }
    if (scenario === 'scroll') {
      main.term.scrollToTop()
      await frame()
      for (let i = 0; i < 40; i++) {
        main.term.scrollLines(13)
        await frame()
        main.term.scrollLines(-5)
        await frame()
      }
    }
  } else {
    await write(main, chunks.join(''))
    if (side) await write(side, sideChunks.join(''))
  }

  if (scenario === 'scroll') main.term.scrollToLine(600)
  else main.term.scrollToBottom()
  const target = capture === '2' && side ? side : main
  await settle(5)
  if (repair === 'sibling-atlas') {
    // Positive control (xterm.js #6014): clearing the shared atlas from the
    // other terminal must corrupt the captured one on this addon version.
    const other = target === main ? side : main
    other?.addon.clearTextureAtlas()
    other?.term.refresh(0, ROWS - 1)
    await settle(5)
    target.term.refresh(0, ROWS - 1)
  } else {
    await applyRepair(target.term, target.addon)
  }
  await settle(5)

  const host = document.getElementById(target === main ? 'terminal' : 'terminal-2') as HTMLElement
  // The first canvas is the 2D link layer; the WebGL surface has no class.
  const canvas = host.querySelector('canvas:not(.xterm-link-layer)') as HTMLCanvasElement
  const buffer = target.term.buffer.active
  const viewport: string[] = []
  for (let y = 0; y < target.term.rows; y++) {
    viewport.push(buffer.getLine(buffer.viewportY + y)?.translateToString(true) ?? '')
  }
  const pages = coreOf(main.term)._renderService?._renderer?.value?._charAtlas?.pages ?? []
  post({
    png: canvas.toDataURL('image/png'),
    width: canvas.width,
    height: canvas.height,
    viewportY: buffer.viewportY,
    text: viewport.join('\n'),
    dpr: window.devicePixelRatio,
    atlas: `added=${stats.added} merged=${stats.merged} pageCount=${pages.length} pages=${pages.map((p) => p.canvas.width).join(',')} errors=${errors.length} firstError=${errors[0] ?? ''}`
  })
}

main().catch((error: unknown) => post({ error: String(error) }))
