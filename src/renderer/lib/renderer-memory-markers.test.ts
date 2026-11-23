import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

function xterm(cols: number, normal: number, alternate: number) {
  return { cols, buffer: { normal: { length: normal }, alternate: { length: alternate } } }
}

const mocks = vi.hoisted(() => ({
  mac: true,
  tauri: true,
  log: vi.fn(async (_payload: { source: string; message: string; level: string }) => undefined),
  getState: vi.fn(() => ({
    terminals: [
      { rendererAttachmentCount: 2, transcript: 'secret', pendingScrollback: ['private'] },
      { isHidden: true, detachedOutput: 'private' }
    ]
  }))
}))

vi.mock('./platform', () => ({
  get isMac() {
    return mocks.mac
  }
}))
vi.mock('./tauri-runtime', () => ({ isTauriContext: () => mocks.tauri }))
vi.mock('./log-api', () => ({ logFrontendError: mocks.log }))
vi.mock('@/stores/terminal-store', () => ({
  useTerminalStore: { getState: mocks.getState, subscribe: vi.fn() }
}))
vi.mock('@/components/terminal/terminal-cache', () => ({
  forEachCachedTerminal: (visit: (terminal: ReturnType<typeof xterm>) => void) => {
    visit(xterm(150, 40, 0))
    return 1
  }
}))
vi.mock('@/utils/terminal-registry', () => ({
  getRegistrySize: () => 2,
  forEachTerminal: (visit: (terminal: ReturnType<typeof xterm>) => void) => {
    visit(xterm(80, 100, 10))
    visit(xterm(100, 20, 0))
  }
}))
vi.mock('./webgl-renderer-budget', () => ({ countWebglSlots: () => 2 }))

beforeEach(() => {
  vi.resetModules()
  vi.useFakeTimers()
  vi.clearAllMocks()
  mocks.mac = true
  mocks.tauri = true
})

afterEach(() => {
  vi.useRealTimers()
})

async function advanceSample(): Promise<void> {
  await vi.advanceTimersByTimeAsync(60_000)
  await vi.dynamicImportSettled()
}

describe('renderer memory markers', () => {
  it('logs only count and length aggregates on a single timer per desktop renderer', async () => {
    const { installRendererMemoryMarkers } = await import('./renderer-memory-markers')
    installRendererMemoryMarkers('boot1')
    installRendererMemoryMarkers('boot2')
    expect(mocks.log).not.toHaveBeenCalled()
    await advanceSample()
    expect(mocks.log).toHaveBeenCalledExactlyOnceWith({
      level: 'warn',
      source: 'renderer-memory',
      message:
        'event=sample nonce=boot1 terminals=2 attached=1 mounted_xterms=2 mounted_cells=10800 ' +
        'cached_xterms=1 cached_cells=6000 webgl_slots=2 ' +
        'transcript_chars=6 detached_chars=7 pending_scrollback_lines=1 hidden_terminals=1'
    })
    await advanceSample()
    expect(mocks.log).toHaveBeenCalledTimes(2)
  })

  it('uses the real renderer boot nonce to correlate samples with a reload', async () => {
    const { installRendererLifecycleMarkers } = await import('./renderer-lifecycle-markers')
    installRendererLifecycleMarkers()
    const boot = mocks.log.mock.calls[0]?.[0]
    expect(boot?.source).toBe('renderer-lifecycle')
    const [nonce] = boot?.message.match(/nonce=[a-z0-9]+/) ?? []
    expect(nonce).toBeDefined()
    await advanceSample()
    expect(mocks.log.mock.calls[1]?.[0]).toMatchObject({
      source: 'renderer-memory',
      message: expect.stringContaining(`${nonce} `)
    })
  })

  it.each([
    { mac: true, tauri: false },
    { mac: false, tauri: true }
  ])('does not sample outside native macOS ($mac, $tauri)', async ({ mac, tauri }) => {
    mocks.mac = mac
    mocks.tauri = tauri
    const { installRendererMemoryMarkers } = await import('./renderer-memory-markers')
    installRendererMemoryMarkers('boot1')
    await advanceSample()
    expect(mocks.getState).not.toHaveBeenCalled()
    expect(mocks.log).not.toHaveBeenCalled()
  })
})
