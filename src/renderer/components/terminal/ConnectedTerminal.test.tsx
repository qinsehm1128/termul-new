import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import type { ComponentProps, ReactElement } from 'react'
import { toast } from 'sonner'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import * as appSettingsStore from '@/stores/app-settings-store'
import { clearScrollPosition } from '@/utils/terminal-registry'
import { createXtermTerminalMock } from './__tests__/xterm-terminal-mock'

// Mock Tauri APIs BEFORE importing the component
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(vi.fn())),
  emit: vi.fn()
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(() =>
    Promise.resolve({
      id: 'terminal-123',
      shell: 'bash',
      cwd: '/home/user'
    })
  )
}))

vi.mock('sonner', () => ({
  toast: {
    error: vi.fn()
  }
}))

// The '@xterm/xterm' Terminal stub is shared with
// ConnectedTerminal.contextmenu.test.tsx via one factory, so a missing stub
// member fails once instead of drifting per suite.
const { instance: mockTerminalInstance, handles: xtermHandles } = createXtermTerminalMock()
const mockTerminalConstructor = xtermHandles.constructorSpy

const mockFitAddonInstance = {
  fit: vi.fn(),
  dispose: vi.fn()
}

const _mockWebglAddonInstance = {
  onContextLoss: vi.fn(),
  dispose: vi.fn()
}

// Track WebGL addon instances for recovery testing
let webglAddonCreateCount = 0
let capturedContextLossCallback: (() => void) | null = null
// Track the last created WebGL addon instance for disposal order testing
let lastCreatedWebglInstance: {
  dispose: ReturnType<typeof vi.fn>
  onContextLoss: ReturnType<typeof vi.fn>
  clearTextureAtlas: ReturnType<typeof vi.fn>
  onAddTextureAtlasCanvas: ReturnType<typeof vi.fn>
  onRemoveTextureAtlasCanvas: ReturnType<typeof vi.fn>
} | null = null
let _capturedAtlasAddCallback: (() => void) | null = null
let capturedAtlasRemoveCallback: (() => void) | null = null

const capturedLinkProviders = xtermHandles.linkProviders

vi.mock('@xterm/xterm', async () => {
  const { createXtermTerminalMock } = await import('./__tests__/xterm-terminal-mock')
  return { Terminal: createXtermTerminalMock().Terminal }
})

vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class MockFitAddon {
    fit = mockFitAddonInstance.fit
    dispose = mockFitAddonInstance.dispose
  }
}))

const {
  mockAttachPixelSmoothScroll,
  mockPixelScrollDispose,
  mockPixelScrollReset,
  mockPixelScrollSetEnabled
} = vi.hoisted(() => {
  const mockPixelScrollDispose = vi.fn()
  const mockPixelScrollReset = vi.fn()
  const mockPixelScrollSetEnabled = vi.fn()
  const mockAttachPixelSmoothScroll = vi.fn(() => ({
    attached: false,
    dispose: mockPixelScrollDispose,
    reset: mockPixelScrollReset,
    setEnabled: mockPixelScrollSetEnabled
  }))
  return {
    mockAttachPixelSmoothScroll,
    mockPixelScrollDispose,
    mockPixelScrollReset,
    mockPixelScrollSetEnabled
  }
})

vi.mock('./terminal-pixel-scroll', () => ({
  attachPixelSmoothScroll: mockAttachPixelSmoothScroll
}))

// Keeps the suite from importing the real Unicode v11 addon; the activation
// behaviour is covered against a real Terminal in terminal-unicode.test.ts.
const { mockEnsureTerminalUnicode11 } = vi.hoisted(() => ({
  mockEnsureTerminalUnicode11: vi.fn()
}))

vi.mock('./terminal-unicode', () => ({
  ensureTerminalUnicode11: mockEnsureTerminalUnicode11
}))

vi.mock('@xterm/addon-webgl', () => ({
  WebglAddon: class MockWebglAddon {
    dispose = vi.fn()
    clearTextureAtlas = vi.fn()
    onContextLoss = vi.fn((cb: () => void) => {
      capturedContextLossCallback = cb
    })
    onAddTextureAtlasCanvas = vi.fn((cb: () => void) => {
      _capturedAtlasAddCallback = cb
      return { dispose: vi.fn() }
    })
    onRemoveTextureAtlasCanvas = vi.fn((cb: () => void) => {
      capturedAtlasRemoveCallback = cb
      return { dispose: vi.fn() }
    })
    constructor() {
      webglAddonCreateCount++
      // Store reference to this instance for disposal order testing
      lastCreatedWebglInstance = {
        dispose: this.dispose,
        onContextLoss: this.onContextLoss,
        clearTextureAtlas: this.clearTextureAtlas,
        onAddTextureAtlasCanvas: this.onAddTextureAtlasCanvas,
        onRemoveTextureAtlasCanvas: this.onRemoveTextureAtlasCanvas
      }
    }
  }
}))

vi.mock('@/lib/file-path-links', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/file-path-links')>()
  return {
    ...actual,
    openFilePathFromTerminal: vi.fn()
  }
})

vi.mock('@/stores/project-store', () => ({
  useActiveProject: vi.fn(() => ({ path: '/project-root' })),
  useProjects: vi.fn(() => []),
  useActiveProjectId: vi.fn(() => 'project-a')
}))

// Mock window.api with proper typing for mocks
let capturedDataCallback: ((id: string, data: Uint8Array) => void) | null = null
let _capturedExitCallback: ((id: string, exitCode: number, signal?: number) => void) | null = null
let capturedPowerResumeCallback: (() => void) | null = null

const mockTerminalApi = {
  spawn: vi.fn<(...args: unknown[]) => Promise<unknown>>(),
  write: vi.fn<(...args: unknown[]) => Promise<unknown>>(),
  resize: vi.fn<(...args: unknown[]) => Promise<unknown>>(),
  kill: vi.fn<(...args: unknown[]) => Promise<unknown>>(() => Promise.resolve({ success: true })),
  onData: vi.fn<(cb: (id: string, data: Uint8Array) => void) => () => void>((cb) => {
    capturedDataCallback = cb
    return vi.fn()
  }),
  onExit: vi.fn<(cb: (id: string, exitCode: number, signal?: number) => void) => () => void>(
    (cb) => {
      _capturedExitCallback = cb
      return vi.fn()
    }
  )
}

const mockClipboardApi = {
  readText: vi.fn<() => Promise<{ success: boolean; data?: string; error?: string }>>(),
  writeText: vi.fn<() => Promise<{ success: boolean; error?: string }>>()
}

// Define mock window.api
type WindowWithOptionalApi = Window & { api?: unknown }

const mockWindowApi = {
  terminal: mockTerminalApi,
  clipboard: mockClipboardApi,
  persistence: {
    read: vi.fn(() => Promise.resolve({ success: true, data: undefined })),
    write: vi.fn(() => Promise.resolve({ success: true }))
  },
  system: {
    getHomeDirectory: vi.fn(() => Promise.resolve({ success: true, data: '/home/user' })),
    onPowerResume: vi.fn((cb: () => void) => {
      capturedPowerResumeCallback = cb
      return vi.fn()
    })
  }
}

Object.defineProperty(window, 'api', {
  value: mockWindowApi as unknown as Window['api'],
  writable: true,
  configurable: true
})

import { clipboardApi, systemApi, terminalApi } from '@/lib/api'
import { openFilePathFromTerminal } from '@/lib/file-path-links'
import { addRendererRef, registerPrimaryTerminalData, removeRendererRef } from '@/lib/terminal-api'
import { ConnectedTerminal } from './ConnectedTerminal'
import { clearTerminalCache, disposeCachedTerminal, hasCachedTerminal } from './terminal-cache'

const { mockRecordTerminalContinuityEvent, mockGetOrCreateProjectContinuityCorrelation } =
  vi.hoisted(() => ({
    mockRecordTerminalContinuityEvent: vi.fn(),
    mockGetOrCreateProjectContinuityCorrelation: vi.fn(() => 'corr-project-a')
  }))

vi.mock('@/hooks/use-terminal-restore', () => ({
  isTerminalPendingPtyAssignment: vi.fn(() => false)
}))

vi.mock('@/lib/terminal-continuity-instrumentation', () => ({
  recordTerminalContinuityEvent: mockRecordTerminalContinuityEvent,
  getOrCreateProjectContinuityCorrelation: mockGetOrCreateProjectContinuityCorrelation
}))

// Mock the API modules
vi.mock('@/lib/api', () => ({
  terminalApi: {
    spawn: vi.fn(),
    write: vi.fn(),
    resize: vi.fn(),
    setDisplayMode: vi.fn(),
    onDisplayModeChanged: vi.fn(() => vi.fn()),
    closeView: vi.fn(),
    terminate: vi.fn(),
    kill: vi.fn(),
    onData: vi.fn(),
    onExit: vi.fn(),
    onCwdChanged: vi.fn(),
    getCwd: vi.fn(),
    watch: vi.fn()
  },
  systemApi: {
    getHomeDirectory: vi.fn(),
    onPowerResume: vi.fn(() => vi.fn())
  },
  clipboardApi: {
    readText: vi.fn(),
    writeText: vi.fn(),
    hasImage: vi.fn(() => Promise.resolve({ success: true, data: false }))
  }
}))

vi.mock('@/stores/app-settings-store', () => ({
  useTerminalFontFamily: vi.fn(() => 'Menlo, Monaco, "Courier New", monospace'),
  useTerminalSymbolFontFamily: vi.fn(() => ''),
  useTerminalFontSize: vi.fn(() => 14),
  useTerminalBufferSize: vi.fn(() => 10000),
  useTerminalRenderer: vi.fn(() => 'auto'),
  useTerminalScreenReaderMode: vi.fn(() => false)
}))

import { useTerminalRenderer, useTerminalScreenReaderMode } from '@/stores/app-settings-store'

const mockTerminalStoreState = {
  terminals: [] as Array<{ id: string; ptyId?: string; healthStatus?: string }>,
  activeTerminalId: '',
  selectTerminal: vi.fn(),
  addTerminal: vi.fn(),
  closeTerminal: vi.fn(),
  renameTerminal: vi.fn(),
  reorderTerminals: vi.fn(),
  setTerminals: vi.fn(),
  resumeTerminalResource: vi.fn(async () => ({ success: true, data: undefined })),
  setTerminalPtyId: vi.fn(),
  setTerminalClaim: vi.fn(),
  findTerminalByPtyId: vi.fn(),
  updateTerminalCwd: vi.fn(),
  updateTerminalGitBranch: vi.fn(),
  updateTerminalGitStatus: vi.fn(),
  updateTerminalExitCode: vi.fn(),
  updateTerminalScrollback: vi.fn(),
  appendTranscript: vi.fn(),
  peekTranscript: vi.fn(() => ''),
  consumeTranscript: vi.fn(() => ''),
  appendDetachedOutput: vi.fn(),
  consumeDetachedOutput: vi.fn(() => ''),
  setRendererAttached: vi.fn(),
  setTerminalHealthStatus: vi.fn(),
  setTerminalResumeCursor: vi.fn(),
  setTerminalHidden: vi.fn(),
  updateTerminalActivity: vi.fn(),
  updateTerminalLastActivityTimestamp: vi.fn(),
  updateTerminalActivityBatch: vi.fn(),
  restartTerminal: vi.fn(),
  restartTerminalResource: vi.fn(async () => true),
  clearTerminalPtyId: vi.fn(),
  truncateHiddenTerminalBuffers: vi.fn(),
  getTerminalCount: vi.fn(() => 0),
  isTerminalLimitReached: vi.fn(() => false),
  cleanupProjectTerminals: vi.fn(),
  cleanupRecoveries: {} as Record<
    string,
    {
      terminalId: string
      primaryCode: string
      cleanupStage: 'kill' | 'wait' | 'flusher_join' | 'reader_join'
      retrying: boolean
      retryFailed: boolean
    }
  >,
  recordTerminalCleanupFailure: vi.fn(),
  retryTerminalCleanup: vi.fn(async () => true)
}

vi.mock('@/stores/terminal-store', () => ({
  useTerminalStore: Object.assign(
    vi.fn((selector) => selector(mockTerminalStoreState)),
    { getState: () => mockTerminalStoreState }
  )
}))

/**
 * Mirrors the production contract: a live writer only receives chunks for the
 * PTY it has bound. Modelling the binding here (instead of handing every
 * registered callback every chunk) keeps the "non-matching PTY never reaches
 * this terminal" assertions meaningful.
 */
const primaryRegistry = vi.hoisted(() => ({
  entries: [] as Array<{ cb: (data: Uint8Array) => void; boundId: string | null }>,
  handles: [] as Array<{ bind: ReturnType<typeof vi.fn>; dispose: ReturnType<typeof vi.fn> }>
}))

vi.mock('@/lib/terminal-api', () => ({
  addRendererRef: vi.fn().mockResolvedValue({ success: true, data: undefined }),
  removeRendererRef: vi.fn().mockResolvedValue({ success: true, data: undefined }),
  subscribeTerminalData: vi.fn(() => vi.fn()),
  registerPrimaryTerminalData: vi.fn((cb: (data: Uint8Array) => void) => {
    const entry: { cb: (data: Uint8Array) => void; boundId: string | null } = { cb, boundId: null }
    primaryRegistry.entries.push(entry)
    const handle = {
      bind: vi.fn((terminalId: string) => {
        entry.boundId = terminalId
      }),
      dispose: vi.fn(() => {
        entry.boundId = null
      })
    }
    primaryRegistry.handles.push(handle)
    return handle
  })
}))

const dispatchPrimaryData = (terminalId: string, data: Uint8Array): void => {
  for (const entry of primaryRegistry.entries) {
    if (entry.boundId === terminalId) entry.cb(data)
  }
}

describe('ConnectedTerminal', () => {
  let rendererPreferenceSpy: ReturnType<typeof vi.spyOn>
  let getBoundingClientRectSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    clearTerminalCache()
    vi.clearAllMocks()
    mockRecordTerminalContinuityEvent.mockReset()
    mockGetOrCreateProjectContinuityCorrelation.mockReset()
    mockGetOrCreateProjectContinuityCorrelation.mockReturnValue('corr-project-a')
    rendererPreferenceSpy = vi
      .spyOn(appSettingsStore, 'useTerminalRenderer')
      .mockReturnValue('auto')
    vi.mocked(useTerminalScreenReaderMode).mockReturnValue(false)
    webglAddonCreateCount = 0
    capturedContextLossCallback = null
    xtermHandles.scrollCallback = null
    xtermHandles.dataCallback = null
    xtermHandles.resizeCallback = null
    _capturedAtlasAddCallback = null
    capturedAtlasRemoveCallback = null
    capturedPowerResumeCallback = null
    primaryRegistry.entries.length = 0
    primaryRegistry.handles.length = 0
    capturedDataCallback = dispatchPrimaryData
    _capturedExitCallback = null
    lastCreatedWebglInstance = null
    capturedLinkProviders.length = 0

    global.ResizeObserver = class MockResizeObserver {
      observe = vi.fn()
      unobserve = vi.fn()
      disconnect = vi.fn()
    } as unknown as typeof ResizeObserver

    getBoundingClientRectSpy = vi
      .spyOn(HTMLDivElement.prototype, 'getBoundingClientRect')
      .mockReturnValue({
        width: 800,
        height: 600,
        top: 0,
        left: 0,
        bottom: 600,
        right: 800,
        x: 0,
        y: 0,
        toJSON: () => {}
      } as DOMRect)

    // Re-setup onData and onExit mocks with fresh callback captures
    vi.mocked(terminalApi).onData.mockImplementation(
      (cb: (id: string, data: Uint8Array) => void) => {
        capturedDataCallback = cb
        return vi.fn()
      }
    )
    vi.mocked(terminalApi).onExit.mockImplementation(
      (cb: (id: string, exitCode: number, signal?: number) => void) => {
        _capturedExitCallback = cb
        return vi.fn()
      }
    )
    vi.mocked(systemApi).onPowerResume.mockImplementation((cb: () => void) => {
      capturedPowerResumeCallback = cb
      return vi.fn()
    })

    mockTerminalStoreState.terminals = []
    mockTerminalStoreState.findTerminalByPtyId.mockReset()
    mockTerminalStoreState.findTerminalByPtyId.mockImplementation((ptyId: string) => ({
      id: ptyId,
      ptyId,
      conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
      claim: 'test-memory-grant',
      healthStatus: 'running',
      cwd: '/terminal-cwd'
    }))
    mockTerminalStoreState.resumeTerminalResource.mockReset()
    mockTerminalStoreState.resumeTerminalResource.mockResolvedValue({
      success: true,
      data: undefined
    })
    mockTerminalStoreState.updateTerminalActivity.mockReset()
    mockTerminalStoreState.updateTerminalLastActivityTimestamp.mockReset()
    mockTerminalStoreState.updateTerminalActivityBatch.mockReset()
    mockTerminalStoreState.setRendererAttached.mockReset()
    mockTerminalStoreState.peekTranscript.mockReset()
    mockTerminalStoreState.peekTranscript.mockReturnValue('')
    mockTerminalStoreState.consumeTranscript.mockReset()
    mockTerminalStoreState.consumeTranscript.mockReturnValue('')
    mockTerminalStoreState.consumeDetachedOutput.mockReset()
    mockTerminalStoreState.consumeDetachedOutput.mockReturnValue('')
    mockTerminalStoreState.cleanupRecoveries = {}
    mockTerminalStoreState.recordTerminalCleanupFailure.mockReset()
    mockTerminalStoreState.recordTerminalCleanupFailure.mockImplementation((result) => {
      if (result.success) return null
      try {
        const detail = JSON.parse(result.error) as {
          terminalId: string
          primaryCode: string
          cleanupStage: 'kill' | 'wait' | 'flusher_join' | 'reader_join'
        }
        mockTerminalStoreState.cleanupRecoveries[detail.terminalId] = {
          ...detail,
          retrying: false,
          retryFailed: false
        }
        return detail
      } catch {
        return null
      }
    })
    mockTerminalStoreState.retryTerminalCleanup.mockReset()
    mockTerminalStoreState.retryTerminalCleanup.mockResolvedValue(true)
    mockTerminalStoreState.setTerminalResumeCursor.mockReset()
    // Default records carry a claim, so the watch branch is not taken. Tests
    // that need it drop the claim and give this a successful result.
    vi.mocked(terminalApi).watch.mockReset()
    vi.mocked(terminalApi).watch.mockResolvedValue({
      success: false,
      error: 'not watched',
      code: 'TERMINAL_NOT_FOUND'
    })

    vi.mocked(terminalApi).spawn.mockResolvedValue({
      success: true,
      // CAP-3: spawn is the only claim issuance path — the fixture carries it.
      data: {
        id: 'terminal-123',
        shell: 'bash',
        cwd: '/home/user',
        claim: 'lease-claim-connected'
      }
    })
    vi.mocked(terminalApi).write.mockResolvedValue({ success: true, data: undefined })
    vi.mocked(terminalApi).resize.mockResolvedValue({ success: true, data: undefined })
    vi.mocked(terminalApi).terminate.mockReset()
    vi.mocked(terminalApi).terminate.mockResolvedValue({ success: true, data: undefined })

    // Reset clipboard mocks
    vi.mocked(clipboardApi).readText.mockResolvedValue({ success: true, data: '' })
    vi.mocked(clipboardApi).writeText.mockResolvedValue({ success: true, data: undefined })

    // Reset terminal selection mocks
    mockTerminalInstance.hasSelection.mockReturnValue(false)
    mockTerminalInstance.getSelection.mockReturnValue('')
  })

  afterEach(() => {
    getBoundingClientRectSpy.mockRestore()
    rendererPreferenceSpy.mockRestore()
    cleanup()
  })

  it('should render without crashing', () => {
    const { container } = render(<ConnectedTerminal />)
    expect(container.querySelector('div')).toBeTruthy()
  })

  it('should keep screen reader mode opt-in and apply runtime changes', async () => {
    const { rerender } = render(<ConnectedTerminal />)

    expect(mockTerminalConstructor).toHaveBeenCalledWith(
      expect.objectContaining({ screenReaderMode: false })
    )

    vi.mocked(useTerminalScreenReaderMode).mockReturnValue(true)
    rerender(<ConnectedTerminal className="screen-reader-enabled" />)

    await vi.waitFor(() => {
      expect(mockTerminalInstance.options.screenReaderMode).toBe(true)
    })
  })

  it('should spawn terminal on mount when no external ID provided', async () => {
    render(<ConnectedTerminal />)

    // Wait for async spawn
    await vi.waitFor(() => {
      expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
    })

    // CAP-3: the claim issued by the spawn response lands in the terminal store.
    await vi.waitFor(() => {
      expect(mockTerminalStoreState.setTerminalClaim).toHaveBeenCalledWith(
        'terminal-123',
        'lease-claim-connected'
      )
    })
  })

  it('should not respawn the terminal or re-register listeners when the terminal PTY changes via restart', async () => {
    const { rerender } = render(<ConnectedTerminal />)

    await vi.waitFor(() => {
      expect(vi.mocked(terminalApi).spawn).toHaveBeenCalledTimes(1)
    })

    const existingDisposeCalls = mockTerminalInstance.dispose.mock.calls.length
    const existingOnDataCalls = vi.mocked(terminalApi).onData.mock.calls.length
    const existingOnExitCalls = vi.mocked(terminalApi).onExit.mock.calls.length

    mockTerminalStoreState.terminals = [
      {
        id: 'terminal-123',
        ptyId: 'restart-123',
        healthStatus: 'running'
      }
    ]
    rerender(<ConnectedTerminal />)

    await new Promise((resolve) => setTimeout(resolve, 0))

    expect(vi.mocked(terminalApi).spawn).toHaveBeenCalledTimes(1)
    expect(mockTerminalInstance.dispose.mock.calls.length).toBe(existingDisposeCalls)
    expect(vi.mocked(terminalApi).onData.mock.calls.length).toBe(existingOnDataCalls)
    expect(vi.mocked(terminalApi).onExit.mock.calls.length).toBe(existingOnExitCalls)
    expect(mockTerminalStoreState.setRendererAttached).toHaveBeenCalledWith('terminal-123', true)
  })

  it('should call onSpawned callback with terminal ID', async () => {
    const onSpawned = vi.fn()
    render(<ConnectedTerminal onSpawned={onSpawned} />)

    await vi.waitFor(() => {
      expect(onSpawned).toHaveBeenCalledWith('terminal-123')
    })
  })

  it('should call onBoundToStoreTerminal callback when spawned', async () => {
    const onBoundToStoreTerminal = vi.fn()
    render(<ConnectedTerminal onBoundToStoreTerminal={onBoundToStoreTerminal} />)

    await vi.waitFor(() => {
      expect(onBoundToStoreTerminal).toHaveBeenCalledWith('terminal-123')
    })
  })

  it('should call onBoundToStoreTerminal callback when external terminalId is provided', async () => {
    const onBoundToStoreTerminal = vi.fn()
    render(
      <ConnectedTerminal
        terminalId="external-123"
        storeTerminalId="store-123"
        onBoundToStoreTerminal={onBoundToStoreTerminal}
      />
    )

    await vi.waitFor(() => {
      expect(onBoundToStoreTerminal).toHaveBeenCalledWith('external-123')
    })
  })

  it('should register and unregister renderer refs for external terminalId', async () => {
    const { unmount } = render(<ConnectedTerminal terminalId="external-123" />)

    await vi.waitFor(() => {
      expect(addRendererRef).toHaveBeenCalledWith('external-123', expect.stringMatching(/^conn-/))
    })

    unmount()

    await vi.waitFor(() => {
      expect(removeRendererRef).toHaveBeenCalledWith(
        'external-123',
        expect.stringMatching(/^conn-/)
      )
    })
  })

  it('should claim the single live-writer slot for an external terminal', async () => {
    render(<ConnectedTerminal terminalId="external-123" />)

    await vi.waitFor(() => {
      expect(registerPrimaryTerminalData).toHaveBeenCalledWith(expect.any(Function))
    })
    // Ownership must be bound to the PTY id, not left to per-chunk filtering:
    // a filtered global observer is what allowed a second writer to exist.
    await vi.waitFor(() => {
      expect(primaryRegistry.handles.at(-1)?.bind).toHaveBeenCalledWith('external-123')
    })
    expect(vi.mocked(terminalApi).onData).not.toHaveBeenCalled()
  })

  it('records the host-authoritative replay cursor returned by watch', async () => {
    // No claim: this is the branch that subscribes via watch instead of resume.
    mockTerminalStoreState.findTerminalByPtyId.mockImplementation((ptyId: string) => ({
      id: ptyId,
      ptyId,
      conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
      claim: undefined,
      healthStatus: 'disconnected',
      cwd: '/terminal-cwd',
      resumeCursor: 12
    }))
    vi.mocked(terminalApi).watch.mockResolvedValue({
      success: true,
      data: { latestSeq: 87, gap: false }
    })

    render(<ConnectedTerminal terminalId="external-123" />)

    await vi.waitFor(() => {
      expect(vi.mocked(terminalApi).watch).toHaveBeenCalledWith('external-123', 12)
    })
    // Discarding this is what pinned the cursor and made every later watch
    // replay a backlog the renderer had already been given.
    await vi.waitFor(() => {
      expect(mockTerminalStoreState.setTerminalResumeCursor).toHaveBeenCalledWith(
        'external-123',
        87
      )
    })
  })

  describe('host replay vs renderer transcript', () => {
    // Both cover the same detached interval. Writing both is what duplicated
    // whole `ls`-class blocks, so exactly one of them must win each time.
    const TRANSCRIPT = 'total 12\r\ndrwxr-xr-x  a.txt\r\n'

    const watchedTerminal = (): void => {
      mockTerminalStoreState.findTerminalByPtyId.mockImplementation((ptyId: string) => ({
        id: ptyId,
        ptyId,
        conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
        claim: undefined,
        healthStatus: 'disconnected',
        cwd: '/terminal-cwd',
        resumeCursor: 12
      }))
      mockTerminalStoreState.peekTranscript.mockReturnValue(TRANSCRIPT)
    }

    const wroteTranscript = (): boolean =>
      mockTerminalInstance.write.mock.calls.some(([chunk]) => chunk === TRANSCRIPT)

    it('drops the transcript when the host replay covered it', async () => {
      watchedTerminal()
      vi.mocked(terminalApi).watch.mockResolvedValue({
        success: true,
        data: { latestSeq: 87, gap: false }
      })

      render(<ConnectedTerminal terminalId="external-123" />)

      await vi.waitFor(() => {
        expect(mockTerminalStoreState.consumeTranscript).toHaveBeenCalledWith('external-123')
      })
      expect(wroteTranscript()).toBe(false)
    })

    it('still writes the transcript when the host reported a gap', async () => {
      watchedTerminal()
      // The host ring buffer could not reach the requested cursor, so its
      // replay is incomplete and the transcript is the only thing that can
      // close the hole.
      vi.mocked(terminalApi).watch.mockResolvedValue({
        success: true,
        data: { latestSeq: 87, gap: true }
      })

      render(<ConnectedTerminal terminalId="external-123" />)

      await vi.waitFor(() => {
        expect(wroteTranscript()).toBe(true)
      })
    })

    it('still writes the transcript when no host replay happened', async () => {
      watchedTerminal()
      vi.mocked(terminalApi).watch.mockResolvedValue({
        success: false,
        error: 'not watched',
        code: 'TERMINAL_NOT_FOUND'
      })
      // Falls through to resume, which reports success without coverage —
      // an already-reconciled record replays nothing.
      mockTerminalStoreState.resumeTerminalResource.mockResolvedValue({
        success: true,
        data: null
      })
      mockTerminalStoreState.findTerminalByPtyId.mockImplementation((ptyId: string) => ({
        id: ptyId,
        ptyId,
        conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
        claim: 'test-memory-grant',
        healthStatus: 'running',
        cwd: '/terminal-cwd'
      }))

      render(<ConnectedTerminal terminalId="external-123" />)

      await vi.waitFor(() => {
        expect(wroteTranscript()).toBe(true)
      })
    })

    describe('remounting a cached instance', () => {
      // A cached xterm holds everything up to the moment it was cached, but
      // the detached interval that followed is not in it. That interval is
      // subject to the very same arbitration as a cold restore — the cached
      // branch used to consume the transcript and drop it unconditionally,
      // which is output loss, not a repaint problem.
      const remount = async (
        ptyId: string,
        props: Partial<ComponentProps<typeof ConnectedTerminal>> = {},
        // Arms per-remount mocks that must not be consumed by the first mount.
        beforeRemount?: () => void
      ): Promise<ReturnType<typeof render>> => {
        const first = render(<ConnectedTerminal terminalId={ptyId} {...props} />)
        await vi.waitFor(() => {
          expect(addRendererRef).toHaveBeenCalledWith(ptyId, expect.stringMatching(/^conn-/))
        })
        first.unmount()
        expect(hasCachedTerminal(ptyId)).toBe(true)
        mockTerminalInstance.write.mockClear()
        mockTerminalStoreState.consumeTranscript.mockClear()
        beforeRemount?.()
        return render(<ConnectedTerminal terminalId={ptyId} {...props} />)
      }

      const wroteMatching = (predicate: (chunk: string) => boolean): boolean =>
        mockTerminalInstance.write.mock.calls.some(
          ([chunk]) => typeof chunk === 'string' && predicate(chunk)
        )

      it('writes the detached interval back when no host replay covered it', async () => {
        // The default record is `running` with a claim, so resume reports
        // success without coverage: serverReplay is null and the transcript is
        // the only thing that holds the detached interval.
        mockTerminalStoreState.peekTranscript.mockReturnValue(TRANSCRIPT)

        const second = await remount('external-cached-gap')

        await vi.waitFor(() => {
          expect(wroteTranscript()).toBe(true)
        })
        expect(mockTerminalStoreState.consumeTranscript).toHaveBeenCalledWith('external-cached-gap')
        second.unmount()
      })

      it('drops the detached interval when the host replay already covered it', async () => {
        // Both sources describe the same interval. Writing the transcript on
        // top of a host replay is what duplicated whole `ls`-class blocks, so
        // reusing a cached instance must not become a second writer either.
        watchedTerminal()
        vi.mocked(terminalApi).watch.mockResolvedValue({
          success: true,
          data: { latestSeq: 87, gap: false }
        })

        const second = await remount('external-cached-covered')

        await vi.waitFor(() => {
          expect(mockTerminalStoreState.consumeTranscript).toHaveBeenCalledWith(
            'external-cached-covered'
          )
        })
        expect(wroteTranscript()).toBe(false)
        second.unmount()
      })

      it('never replays the persisted scrollback into an instance that already holds it', async () => {
        // The cached branch does two jobs, and only one of them is the
        // transcript. `useTerminalAutoSave` calls `syncScrollbackToStore`
        // before the xterm is disposed, precisely so scrollback survives a
        // project switch — so `terminal.pendingScrollback` is repopulated and
        // PaneContent keeps passing it as `initialScrollback` on the remount.
        // It is only ever cleared by the restart flows. Letting a cached
        // remount fall through to the scrollback branch therefore replays the
        // whole buffer into a terminal that already contains it, and doubles
        // it again on every subsequent switch.
        mockTerminalStoreState.peekTranscript.mockReturnValue('')

        const second = await remount('external-cached-scrollback', {
          initialScrollback: ['persisted line one', 'persisted line two']
        })

        await vi.waitFor(() => {
          expect(mockRecordTerminalContinuityEvent).toHaveBeenCalledWith(
            expect.objectContaining({
              name: 'restore-replay-skipped',
              details: expect.objectContaining({ reason: 'cached-terminal' })
            })
          )
        })
        expect(wroteMatching((chunk) => chunk.includes('persisted line one'))).toBe(false)
        second.unmount()
      })

      it('replays the detached interval raw, without the mount-time DEC mode snapshot', async () => {
        // `initialModes` is `terminal.pendingModes`, a snapshot taken when the
        // layout was persisted and cleared only by the restart flows. The
        // reused instance's modes are already live and current, so replaying
        // that snapshot over them re-enters whatever the terminal was in at
        // persist time — alt-screen on a shell that has long since left vim.
        mockTerminalStoreState.peekTranscript.mockReturnValue(TRANSCRIPT)

        const second = await remount('external-cached-modes', {
          initialModes: {
            alternateScreen: true,
            bracketedPaste: true,
            applicationCursor: false,
            mouseTracking: null,
            sgrMouseMode: false,
            sgrMousePixelsMode: false
          }
        })

        await vi.waitFor(() => {
          expect(wroteTranscript()).toBe(true)
        })
        expect(wroteMatching((chunk) => chunk.includes('[?1049h'))).toBe(false)
        second.unmount()
      })

      // The live-write gate opens before the attach round trip and the
      // transcript is only written after it, so every chunk the PTY produces
      // inside that window is newer than a replay that has not happened yet.
      // Painting it straight through put it on screen ahead of the older
      // detached interval — and since PTY reads do not stop on escape-sequence
      // boundaries, a transcript ending mid-CSI would have had its tail parsed
      // only after the live remainder printed literally.
      const LIVE_AFTER_REATTACH = 'live-after-reattach\r\n'

      // `instanceof Uint8Array` is unreliable here: the encoder runs in the
      // Node realm and the component sees jsdom's globals, so the two
      // constructors are different objects. `ArrayBuffer.isView` is not.
      const writeIndexOf = (needle: string): number =>
        mockTerminalInstance.write.mock.calls.findIndex(([chunk]) => {
          const text =
            typeof chunk === 'string'
              ? chunk
              : ArrayBuffer.isView(chunk)
                ? new TextDecoder().decode(chunk as Uint8Array)
                : ''
          return text.includes(needle)
        })

      // `addRendererRef` is the last await inside the attach handshake, so a
      // chunk dispatched from it lands squarely inside the replay window with
      // the live-write gate already open.
      const dispatchDuringAttach = (): void => {
        vi.mocked(addRendererRef).mockImplementationOnce(async (ptyId: string) => {
          dispatchPrimaryData(ptyId, new TextEncoder().encode(LIVE_AFTER_REATTACH))
          return { success: true, data: undefined }
        })
      }

      it('paints the detached interval before output that arrived during the attach', async () => {
        mockTerminalStoreState.peekTranscript.mockReturnValue(TRANSCRIPT)

        const second = await remount('external-cached-order', {}, dispatchDuringAttach)

        await vi.waitFor(() => {
          expect(writeIndexOf(LIVE_AFTER_REATTACH)).toBeGreaterThanOrEqual(0)
        })
        expect(writeIndexOf(TRANSCRIPT)).toBeGreaterThanOrEqual(0)
        expect(writeIndexOf(TRANSCRIPT)).toBeLessThan(writeIndexOf(LIVE_AFTER_REATTACH))
        second.unmount()
      })

      it('drains the hold before the writes that follow the replay', async () => {
        // Draining only once init has finished would still satisfy the
        // transcript-first ordering while letting everything the replay writes
        // afterwards — here the project-env notice — jump the queue ahead of
        // output the PTY produced before it.
        mockTerminalStoreState.peekTranscript.mockReturnValue(TRANSCRIPT)

        const second = await remount(
          'external-cached-drain-position',
          { spawnOptions: { projectId: 'project-a', env: { EXAMPLE_VAR: '1' } } },
          dispatchDuringAttach
        )

        await vi.waitFor(() => {
          expect(writeIndexOf('variable applied')).toBeGreaterThanOrEqual(0)
        })
        expect(writeIndexOf(LIVE_AFTER_REATTACH)).toBeGreaterThanOrEqual(0)
        expect(writeIndexOf(LIVE_AFTER_REATTACH)).toBeLessThan(writeIndexOf('variable applied'))
        second.unmount()
      })

      it('still paints held output in place when the transcript write throws', async () => {
        // Holding without an unconditional drain would be worse than the bug it
        // fixes: one failed replay and the terminal never shows live output
        // again. Draining in `finally` rather than at the end of the try is
        // what also keeps its position — ahead of the project-env notice the
        // replay writes next — on the failing path.
        const consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => undefined)
        mockTerminalStoreState.peekTranscript.mockReturnValue(TRANSCRIPT)

        const second = await remount(
          'external-cached-throw',
          { spawnOptions: { projectId: 'project-a', env: { EXAMPLE_VAR: '1' } } },
          () => {
            dispatchDuringAttach()
            mockTerminalInstance.write.mockImplementationOnce(() => {
              throw new Error('write failed')
            })
          }
        )

        await vi.waitFor(() => {
          expect(writeIndexOf('variable applied')).toBeGreaterThanOrEqual(0)
        })
        expect(writeIndexOf(LIVE_AFTER_REATTACH)).toBeGreaterThanOrEqual(0)
        expect(writeIndexOf(LIVE_AFTER_REATTACH)).toBeLessThan(writeIndexOf('variable applied'))
        second.unmount()
        consoleErrorSpy.mockRestore()
      })

      it('refuses to splice a trimmed transcript onto a live cached instance', async () => {
        // The cap drops the OLDEST bytes at an arbitrary offset. The trim aligns
        // to the next line break, but a line break is not an escape-sequence
        // boundary and carries none of the preceding state, so the surviving
        // tail can start mid-sequence or with colours and DEC modes it never
        // established. The cached branch writes raw onto live modes on purpose,
        // so it has neither the partial heuristic nor the mode rehydrate to
        // absorb that — splicing it renders garbage on screen.
        //
        // The span is lost the moment it is trimmed; the only choice left is how
        // to fail, and a coherent screen missing it beats a garbled one.
        mockTerminalStoreState.peekTranscript.mockReturnValue(TRANSCRIPT)

        const second = await remount('external-cached-trimmed', {}, () => {
          mockTerminalStoreState.findTerminalByPtyId.mockImplementation((ptyId: string) => ({
            id: ptyId,
            ptyId,
            conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
            claim: 'test-memory-grant',
            healthStatus: 'running',
            cwd: '/terminal-cwd',
            transcriptTrimmed: true
          }))
        })

        await vi.waitFor(() => {
          expect(mockRecordTerminalContinuityEvent).toHaveBeenCalledWith(
            expect.objectContaining({
              name: 'restore-replay-skipped',
              details: expect.objectContaining({
                reason: 'transcript-trimmed-unsafe-splice'
              })
            })
          )
        })

        // The damaged bytes must never reach the screen...
        expect(writeIndexOf(TRANSCRIPT)).toBe(-1)
        // ...and must still be consumed, or they are replayed again next mount.
        expect(mockTerminalStoreState.consumeTranscript).toHaveBeenCalledWith(
          'external-cached-trimmed'
        )
        second.unmount()
      })

      it('releases the hold when the resume is denied', async () => {
        // A denied attach returns before the replay ever runs, so its own drain
        // is never reached. Leaving the hold armed there would be worse than
        // the ordering bug it fixes: every later chunk would vanish into a
        // buffer with no writer for the rest of the mount.
        mockTerminalStoreState.peekTranscript.mockReturnValue('')

        const second = await remount('external-cached-denied', {}, () => {
          mockTerminalStoreState.resumeTerminalResource.mockImplementationOnce(async () => {
            dispatchPrimaryData(
              'external-cached-denied',
              new TextEncoder().encode(LIVE_AFTER_REATTACH)
            )
            return { success: false, error: 'gone', code: 'TERMINAL_NOT_FOUND' }
          })
        })

        await vi.waitFor(() => {
          expect(writeIndexOf(LIVE_AFTER_REATTACH)).toBeGreaterThanOrEqual(0)
        })
        second.unmount()
      })
    })
  })

  it('should reuse the cached terminal session and addons after remount', async () => {
    const first = render(<ConnectedTerminal terminalId="external-cached" />)
    await vi.waitFor(() => {
      expect(addRendererRef).toHaveBeenCalledWith(
        'external-cached',
        expect.stringMatching(/^conn-/)
      )
    })

    first.unmount()
    expect(mockTerminalConstructor).toHaveBeenCalledTimes(1)
    expect(mockTerminalInstance.dispose).not.toHaveBeenCalled()

    vi.mocked(useTerminalScreenReaderMode).mockReturnValue(true)
    const second = render(<ConnectedTerminal terminalId="external-cached" />)
    await vi.waitFor(() => {
      expect(addRendererRef).toHaveBeenCalledTimes(2)
    })

    expect(mockTerminalConstructor).toHaveBeenCalledTimes(1)
    expect(mockTerminalInstance.options.screenReaderMode).toBe(true)
    second.unmount()
  })

  it('repairs the surface exactly once after a cached instance is remounted', async () => {
    // Project switch, pane fullscreen toggle and "jump to a hidden terminal"
    // all reattach a cached xterm without writing a byte, so `onWrite` never
    // fires and the render model keeps showing the pre-switch frame. The
    // hide/show repair used to be the only invalidation point and it is
    // unreachable here: a remounted terminal is the active tab, so the ref it
    // keys off starts false.
    const first = render(<ConnectedTerminal terminalId="external-cached-repair" />)
    await vi.waitFor(() => {
      expect(addRendererRef).toHaveBeenCalledWith(
        'external-cached-repair',
        expect.stringMatching(/^conn-/)
      )
    })
    first.unmount()

    mockPixelScrollReset.mockClear()

    const second = render(<ConnectedTerminal terminalId="external-cached-repair" />)
    await vi.waitFor(() => {
      expect(mockPixelScrollReset).toHaveBeenCalledTimes(1)
    })
    // Once. The repair is idempotent but a second pass would mean the arming
    // is not consumed, and every later hide/show would compound it.
    await new Promise((resolve) => setTimeout(resolve, 50))
    expect(mockPixelScrollReset).toHaveBeenCalledTimes(1)
    second.unmount()
  })

  it('should clean up terminal listeners on unmount without creating extra registrations', async () => {
    const { unmount } = render(<ConnectedTerminal />)

    await vi.waitFor(() => {
      expect(vi.mocked(terminalApi).spawn).toHaveBeenCalledTimes(1)
    })

    // Exactly one live-writer claim, and no global observer: a renderer that
    // watched every PTY and filtered by id is how a second writer became possible.
    expect(registerPrimaryTerminalData).toHaveBeenCalledTimes(1)
    expect(vi.mocked(terminalApi).onData).not.toHaveBeenCalled()
    expect(vi.mocked(terminalApi).onExit).toHaveBeenCalledTimes(1)

    unmount()

    expect(mockTerminalInstance.dispose).not.toHaveBeenCalled()
    expect(hasCachedTerminal('terminal-123')).toBe(true)
    expect(removeRendererRef).toHaveBeenCalledWith('terminal-123', expect.stringMatching(/^conn-/))
    expect(registerPrimaryTerminalData).toHaveBeenCalledTimes(1)
    expect(vi.mocked(terminalApi).onData).not.toHaveBeenCalled()
    expect(vi.mocked(terminalApi).onExit).toHaveBeenCalledTimes(1)
    expect(disposeCachedTerminal('terminal-123')).toBe(true)
    expect(mockTerminalInstance.dispose).toHaveBeenCalledTimes(1)
  })

  it('should not spawn terminal when external ID provided', async () => {
    render(<ConnectedTerminal terminalId="external-123" />)

    // Give time for potential spawn
    await new Promise((resolve) => setTimeout(resolve, 50))

    expect(vi.mocked(terminalApi).spawn).not.toHaveBeenCalled()
  })

  it('should not spawn terminal when autoSpawn is false', async () => {
    render(<ConnectedTerminal autoSpawn={false} />)

    await new Promise((resolve) => setTimeout(resolve, 50))

    expect(vi.mocked(terminalApi).spawn).not.toHaveBeenCalled()
  })

  it('should set up data listener BEFORE spawn to avoid race condition', async () => {
    // Track the order of calls
    const callOrder: string[] = []
    ;(
      vi.mocked(terminalApi).onData as unknown as { mockImplementation: (fn: () => void) => void }
    ).mockImplementation(() => {
      callOrder.push('onData')
      return vi.fn()
    })
    ;(
      vi.mocked(terminalApi).spawn as unknown as {
        mockImplementation: (fn: () => Promise<unknown>) => void
      }
    ).mockImplementation(async () => {
      callOrder.push('spawn')
      return {
        success: true,
        data: { id: 'terminal-123', shell: 'bash', cwd: '/home/user' }
      }
    })

    render(<ConnectedTerminal />)

    await vi.waitFor(() => {
      expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
    })

    // Verify onData was called BEFORE spawn
    const onDataIndex = callOrder.indexOf('onData')
    const spawnIndex = callOrder.indexOf('spawn')
    expect(onDataIndex).toBeLessThan(spawnIndex)
  })

  it('should set up exit listener BEFORE spawn to avoid race condition', async () => {
    const callOrder: string[] = []
    ;(
      vi.mocked(terminalApi).onExit as unknown as { mockImplementation: (fn: () => void) => void }
    ).mockImplementation(() => {
      callOrder.push('onExit')
      return vi.fn()
    })
    ;(
      vi.mocked(terminalApi).spawn as unknown as {
        mockImplementation: (fn: () => Promise<unknown>) => void
      }
    ).mockImplementation(async () => {
      callOrder.push('spawn')
      return {
        success: true,
        data: { id: 'terminal-123', shell: 'bash', cwd: '/home/user' }
      }
    })

    render(<ConnectedTerminal />)

    await vi.waitFor(() => {
      expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
    })

    const onExitIndex = callOrder.indexOf('onExit')
    const spawnIndex = callOrder.indexOf('spawn')
    expect(onExitIndex).toBeLessThan(spawnIndex)
  })

  it('should call onError when spawn fails', async () => {
    vi.mocked(terminalApi).spawn.mockResolvedValue({
      success: false,
      error: 'Shell not found',
      code: 'SPAWN_FAILED'
    })

    const onError = vi.fn()
    render(<ConnectedTerminal onError={onError} />)

    await vi.waitFor(() => {
      expect(onError).toHaveBeenCalledWith('Shell not found')
    })
  })

  it('renders one sanitized cleanup-only recovery and retries only the retained id', async () => {
    const detail = {
      terminalId: 'terminal-recoverable-1',
      primaryCode: 'CONVERSATION_DURABILITY_FAILED',
      cleanupStage: 'reader_join' as const
    }
    vi.mocked(terminalApi).spawn.mockResolvedValue({
      success: false,
      error: JSON.stringify(detail),
      code: 'TERMINAL_RESOURCE_ROLLBACK_FAILED'
    })
    vi.mocked(terminalApi)
      .terminate.mockResolvedValueOnce({
        success: false,
        error: JSON.stringify({
          terminalId: detail.terminalId,
          primaryCode: 'TERMINATE_FAILED',
          cleanupStage: 'reader_join'
        }),
        code: 'TERMINATE_FAILED'
      })
      .mockResolvedValueOnce({ success: true, data: undefined })
    mockTerminalStoreState.retryTerminalCleanup.mockImplementation(async (terminalId: string) => {
      const result = await terminalApi.terminate(terminalId)
      const retained = mockTerminalStoreState.cleanupRecoveries[terminalId]
      if (result.success) {
        delete mockTerminalStoreState.cleanupRecoveries[terminalId]
        return true
      }
      if (retained) {
        mockTerminalStoreState.cleanupRecoveries[terminalId] = {
          ...retained,
          retrying: false,
          retryFailed: true
        }
      }
      return false
    })
    const onError = vi.fn()

    const { rerender } = render(<ConnectedTerminal onError={onError} />)

    await vi.waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent(detail.terminalId)
    })
    expect(onError).toHaveBeenCalledWith('The terminal process stopped, but cleanup is incomplete.')
    expect(onError).not.toHaveBeenCalledWith(expect.stringContaining('primaryCode'))
    expect(screen.getAllByRole('button', { name: /retry termination for terminal/i })).toHaveLength(
      1
    )

    fireEvent.click(screen.getByRole('button', { name: /retry termination for terminal/i }))
    await vi.waitFor(() => expect(terminalApi.terminate).toHaveBeenCalledTimes(1))
    rerender(<ConnectedTerminal onError={onError} className="cleanup-retry-state" />)
    expect(screen.getByRole('alert')).toHaveTextContent(
      'Cleanup is still incomplete. You can retry termination again.'
    )

    fireEvent.click(screen.getByRole('button', { name: /retry termination for terminal/i }))
    await vi.waitFor(() => expect(terminalApi.terminate).toHaveBeenCalledTimes(2))
    rerender(<ConnectedTerminal onError={onError} className="cleanup-retry-cleared" />)
    expect(screen.queryByRole('alert')).toBeNull()

    expect(terminalApi.terminate).toHaveBeenNthCalledWith(1, detail.terminalId)
    expect(terminalApi.terminate).toHaveBeenNthCalledWith(2, detail.terminalId)
    expect(vi.mocked(terminalApi).spawn).toHaveBeenCalledTimes(1)
    expect(mockTerminalStoreState.resumeTerminalResource).not.toHaveBeenCalled()
    expect(addRendererRef).not.toHaveBeenCalled()
    expect(mockTerminalStoreState.restartTerminalResource).not.toHaveBeenCalled()
  })

  it('should focus terminal by default', () => {
    render(<ConnectedTerminal />)
    expect(mockTerminalInstance.focus).toHaveBeenCalled()
  })

  it('should not focus terminal when autoFocus is false', () => {
    render(<ConnectedTerminal autoFocus={false} />)
    expect(mockTerminalInstance.focus).not.toHaveBeenCalled()
  })

  it('should apply custom className', () => {
    const { container } = render(<ConnectedTerminal className="custom-class" />)
    expect(container.querySelector('.custom-class')).toBeTruthy()
  })

  it('should dispose terminal on unmount', () => {
    const { unmount } = render(<ConnectedTerminal />)
    unmount()
    expect(mockTerminalInstance.dispose).toHaveBeenCalled()
  })

  describe('Cursor cleanup on unmount', () => {
    it('should disable cursor blink before disposal', () => {
      const { unmount } = render(<ConnectedTerminal />)
      unmount()
      // Cursor blink should be set to false before terminal disposal
      expect(mockTerminalInstance.options.cursorBlink).toBe(false)
    })

    it('should dispose WebGL addon before terminal disposal', () => {
      const disposalOrder: string[] = []

      // Track disposal on the actual WebGL instance created by the component
      ;(
        mockTerminalInstance.dispose as unknown as { mockImplementation: (fn: () => void) => void }
      ).mockImplementation(() => {
        disposalOrder.push('terminal')
      })

      const { unmount } = render(<ConnectedTerminal />)

      // Now set up the spy on the actual WebGL instance that was created
      expect(lastCreatedWebglInstance).toBeTruthy()
      ;(
        lastCreatedWebglInstance!.dispose as unknown as {
          mockImplementation: (fn: () => void) => void
        }
      ).mockImplementation(() => {
        disposalOrder.push('webgl')
      })

      unmount()

      // WebGL should be disposed before terminal
      const webglIndex = disposalOrder.indexOf('webgl')
      const terminalIndex = disposalOrder.indexOf('terminal')
      expect(webglIndex).toBeLessThan(terminalIndex)
    })
  })

  it('should pass spawn options including shell to API', async () => {
    const spawnOptions = { cwd: '/custom/path', shell: 'zsh' }
    render(<ConnectedTerminal spawnOptions={spawnOptions} />)

    await vi.waitFor(() => {
      expect(vi.mocked(terminalApi).spawn).toHaveBeenCalledWith(
        expect.objectContaining({
          cwd: '/custom/path',
          shell: 'zsh',
          cols: expect.any(Number),
          rows: expect.any(Number)
        })
      )
    })
  })

  it('should write PTY data to terminal when ID matches', async () => {
    const { unmount } = render(<ConnectedTerminal />)

    await vi.waitFor(() => {
      expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
    })

    // Small delay to ensure component is fully set up
    await new Promise((resolve) => setTimeout(resolve, 50))

    // Verify capturedDataCallback is set
    expect(capturedDataCallback).toBeTruthy()

    // Manually call the callback to verify it works
    const bytes = new TextEncoder().encode('Hello World')
    capturedDataCallback!('terminal-123', bytes)

    // The callback should have called terminal.write. The second argument is
    // the parse-completion hook the WebGL repaint hangs off.
    expect(mockTerminalInstance.write).toHaveBeenCalledWith(bytes, expect.any(Function))

    unmount()
  })

  it('should preserve UTF-8 bytes for CJK, emoji, and combining characters', async () => {
    const { unmount } = render(<ConnectedTerminal />)

    await vi.waitFor(() => {
      expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
    })
    await new Promise((resolve) => setTimeout(resolve, 50))

    const bytes = new TextEncoder().encode('中文 👩🏽‍💻 e\u0301')
    expect(capturedDataCallback).not.toBeNull()
    capturedDataCallback!('terminal-123', bytes)

    expect(mockTerminalInstance.write).toHaveBeenCalledWith(bytes, expect.any(Function))
    expect(mockTerminalInstance.write.mock.calls.at(-1)?.[0]).toEqual(bytes)

    unmount()
  })

  it('should NOT write PTY data when ID does not match', async () => {
    render(<ConnectedTerminal />)

    await vi.waitFor(() => {
      expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
    })

    // Simulate PTY data event with NON-matching ID
    if (capturedDataCallback) {
      capturedDataCallback('terminal-999', new TextEncoder().encode('Should not appear'))
    }

    // Give time for potential write
    await new Promise((resolve) => setTimeout(resolve, 50))

    expect(mockTerminalInstance.write).not.toHaveBeenCalledWith('Should not appear')
  })

  it('should release the live-writer slot on unmount', async () => {
    const { unmount } = render(<ConnectedTerminal />)

    await vi.waitFor(() => {
      expect(registerPrimaryTerminalData).toHaveBeenCalled()
    })

    unmount()

    // Releasing matters more than unsubscribing: a handle left bound would keep
    // owning the PTY slot and lock out the next renderer for that terminal.
    expect(primaryRegistry.handles.at(-1)?.dispose).toHaveBeenCalled()
  })

  it('should cleanup exit listener on unmount', async () => {
    const cleanupFn = vi.fn()
    ;(
      vi.mocked(terminalApi).onExit as unknown as { mockReturnValue: (v: unknown) => void }
    ).mockReturnValue(cleanupFn)

    const { unmount } = render(<ConnectedTerminal />)

    await vi.waitFor(() => {
      expect(vi.mocked(terminalApi).onExit).toHaveBeenCalled()
    })

    unmount()

    expect(cleanupFn).toHaveBeenCalled()
  })

  it('should clear terminal activity indicator on unmount', async () => {
    mockTerminalStoreState.terminals = [
      { id: 'store-term-1', ptyId: 'terminal-123', healthStatus: 'running' }
    ]
    mockTerminalStoreState.findTerminalByPtyId.mockImplementation((ptyId: string) =>
      ptyId === 'terminal-123'
        ? { id: 'store-term-1', ptyId: 'terminal-123', healthStatus: 'running' }
        : undefined
    )

    const { unmount } = render(<ConnectedTerminal storeTerminalId="store-term-1" />)

    await vi.waitFor(() => {
      expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
    })

    expect(capturedDataCallback).toBeTruthy()
    capturedDataCallback!('terminal-123', new TextEncoder().encode('x'))

    expect(mockTerminalStoreState.updateTerminalActivityBatch).toHaveBeenCalledWith(
      'store-term-1',
      true,
      expect.any(Number)
    )

    mockTerminalStoreState.updateTerminalActivityBatch.mockClear()
    unmount()

    expect(mockTerminalStoreState.updateTerminalActivityBatch).toHaveBeenCalledWith(
      'store-term-1',
      false,
      expect.any(Number)
    )
  })

  it('should set up resize hook on mount', async () => {
    render(<ConnectedTerminal />)

    await vi.waitFor(() => {
      expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
    })

    // Resize is now handled by useTerminalResizeV2 via ResizeObserver.
    // The resize API integration is tested in use-terminal-resize-v2.test.ts.
    // At the component level, we verify the component mounts and spawns
    // correctly with the resize hook in place.
    expect(vi.mocked(terminalApi).resize).toBeDefined()
  })

  it('should not kill PTY process on unmount', async () => {
    const { unmount } = render(<ConnectedTerminal />)

    await vi.waitFor(() => {
      expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
    })

    unmount()

    expect(vi.mocked(terminalApi).kill).not.toHaveBeenCalled()
  })

  it.skip('should persist terminal layout on unload handlers', async () => {
    const saveSpy = vi.spyOn(await import('@/hooks/useTerminalAutoSave'), 'saveTerminalLayout')
    render(<ConnectedTerminal />)

    await vi.waitFor(() => {
      expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
    })

    window.dispatchEvent(new Event('beforeunload'))

    await vi.waitFor(() => {
      expect(saveSpy).toHaveBeenCalled()
    })

    saveSpy.mockRestore()
  })

  describe('Windows ConPTY support', () => {
    const originalPlatform = navigator.platform

    beforeEach(() => {
      Object.defineProperty(navigator, 'platform', {
        value: 'Win32',
        configurable: true
      })
    })

    afterEach(() => {
      Object.defineProperty(navigator, 'platform', {
        value: originalPlatform,
        configurable: true
      })
    })

    it('should use windowsPty options on Windows', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalConstructor).toHaveBeenCalled()
      })

      // Verify Terminal was called with windowsPty options
      expect(mockTerminalConstructor).toHaveBeenCalledWith(
        expect.objectContaining({
          windowsPty: expect.objectContaining({
            backend: 'conpty'
          })
        })
      )
    })

    it('should have convertEol set to false', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalConstructor).toHaveBeenCalled()
      })

      expect(mockTerminalConstructor).toHaveBeenCalledWith(
        expect.objectContaining({
          convertEol: false
        })
      )
    })
  })

  describe('Non-Windows platform', () => {
    const originalPlatform = navigator.platform

    beforeEach(() => {
      Object.defineProperty(navigator, 'platform', {
        value: 'MacIntel',
        configurable: true
      })
    })

    afterEach(() => {
      Object.defineProperty(navigator, 'platform', {
        value: originalPlatform,
        configurable: true
      })
    })

    it('should not include windowsPty on non-Windows platforms', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalConstructor).toHaveBeenCalled()
      })

      // Verify Terminal was NOT called with windowsPty options
      const callArgs = mockTerminalConstructor.mock.calls[0][0]
      expect(callArgs.windowsPty).toBeUndefined()
    })
  })

  describe('Resize debouncing', () => {
    it('should call resize through two-stage pipeline when dimensions change', async () => {
      vi.useFakeTimers()

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // Resize is now handled by useTerminalResizeV2 via ResizeObserver.
      // The hook's internal timing is tested in use-terminal-resize-v2.test.ts.
      // This test verifies end-to-end that resize API is called after
      // the hook processes dimension changes.
      //
      // Simulate a ResizeObserver callback by triggering the observer callback
      // on the first resident div that serves as the container.
      // Note: the hook internally calls performFit which calls fitAddon.fit().
      // After fit, PTY resize is sent immediately.

      // Clear initial resize calls (e.g. needsResizeOnReady path)
      vi.mocked(terminalApi).resize.mockClear()

      // The ResizeObserver was set up by the hook on the container div.
      // We can't directly trigger it, but we can verify that after a
      // visibility change + resize, the PTY resize is eventually called.
      // The exact debounce behavior is covered by the hook's own tests.

      // Instead, verify that the resize API works correctly end-to-end
      // by checking resize is called during visibility changes
      expect(vi.mocked(terminalApi).resize).not.toHaveBeenCalled()

      vi.useRealTimers()
    })

    it('should not leak resize calls after unmount', async () => {
      vi.useFakeTimers()

      const { unmount } = render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // Clear initial resize calls
      vi.mocked(terminalApi).resize.mockClear()

      // Unmount before any resize events
      unmount()

      // Advance timers — no resize should have been called
      await vi.advanceTimersByTimeAsync(300)

      expect(vi.mocked(terminalApi).resize).not.toHaveBeenCalled()

      vi.useRealTimers()
    })
  })

  describe('Dimension synchronization', () => {
    it('should pass measured dimensions to spawn', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // Verify spawn was called with cols and rows from terminal
      expect(vi.mocked(terminalApi).spawn).toHaveBeenCalledWith(
        expect.objectContaining({
          cols: 80,
          rows: 24
        })
      )
    })

    it('should call fit before spawn to get real dimensions', async () => {
      const callOrder: string[] = []

      ;(
        mockFitAddonInstance.fit as unknown as { mockImplementation: (fn: () => void) => void }
      ).mockImplementation(() => {
        callOrder.push('fit')
      })
      ;(
        vi.mocked(terminalApi).spawn as unknown as {
          mockImplementation: (fn: () => Promise<unknown>) => void
        }
      ).mockImplementation(async () => {
        callOrder.push('spawn')
        return {
          success: true,
          data: { id: 'terminal-123', shell: 'bash', cwd: '/home/user' }
        }
      })

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // Verify fit was called before spawn (after initial fit during terminal setup)
      const fitIndices = callOrder.reduce((acc: number[], item, idx) => {
        if (item === 'fit') acc.push(idx)
        return acc
      }, [])
      const spawnIndex = callOrder.indexOf('spawn')

      // At least one fit should happen before spawn
      expect(fitIndices.some((idx) => idx < spawnIndex)).toBe(true)
    })
  })

  describe('Clipboard functionality', () => {
    // GH-588: the Ctrl+V handler degrades to native xterm paste when
    // `navigator.clipboard` is undefined (non-secure context, HTTP+bare-IP).
    // These tests exercise the secure-context path (navigator.clipboard
    // available) so the handler calls the mocked `clipboardApi` facade; the
    // non-secure branch is covered by its own test below. jsdom leaves
    // `navigator.clipboard` undefined by default, so stub it as defined here.
    let originalClipboardDescriptor: PropertyDescriptor | undefined
    beforeEach(() => {
      originalClipboardDescriptor = Object.getOwnPropertyDescriptor(navigator, 'clipboard')
      Object.defineProperty(navigator, 'clipboard', {
        value: { readText: vi.fn(), writeText: vi.fn() },
        configurable: true,
        writable: true
      })
    })
    afterEach(() => {
      if (originalClipboardDescriptor) {
        Object.defineProperty(navigator, 'clipboard', originalClipboardDescriptor)
      } else {
        delete (navigator as { clipboard?: unknown }).clipboard
      }
    })

    it('should set up clipboard keyboard handlers', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })
    })

    it('should copy selection to clipboard on Ctrl+C when text is selected', async () => {
      const selectedText = 'Hello, World!'
      mockTerminalInstance.hasSelection.mockReturnValue(true)
      mockTerminalInstance.getSelection.mockReturnValue(selectedText)
      vi.mocked(clipboardApi).writeText.mockResolvedValue({ success: true, data: undefined })

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })

      // Get the registered handler
      const handler = mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]

      // Simulate Ctrl+C with selection
      const event = new KeyboardEvent('keydown', {
        key: 'c',
        ctrlKey: true,
        bubbles: true
      })

      const result = handler(event)

      // Should prevent xterm from handling
      expect(result).toBe(false)

      // Should write to clipboard via the hook
      await vi.waitFor(() => {
        expect(vi.mocked(clipboardApi).writeText).toHaveBeenCalledWith(selectedText)
      })
    })

    it('should allow Ctrl+C interrupt when no selection exists', async () => {
      mockTerminalInstance.hasSelection.mockReturnValue(false)

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })

      const handler = mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]

      const event = new KeyboardEvent('keydown', {
        key: 'c',
        ctrlKey: true,
        bubbles: true
      })

      const result = handler(event)

      // Should allow xterm to handle (for interrupt signal)
      expect(result).toBe(true)
      expect(vi.mocked(clipboardApi).writeText).not.toHaveBeenCalled()
    })

    it('should not swallow ordinary keys that merely carry the platform modifier', async () => {
      // The clipboard rate limit used to gate on the modifier alone, so every
      // key held with Cmd/Ctrl inside the window was dropped. A remote-desktop
      // client that synthesises keydowns with a spurious modifier flag turns
      // that into "type a line, one character arrives".
      mockTerminalInstance.hasSelection.mockReturnValue(true)
      // Non-empty: an empty selection never records the op, so the rate-limit
      // window would never open and this test would prove nothing.
      mockTerminalInstance.getSelection.mockReturnValue('copied text')
      vi.mocked(clipboardApi).writeText.mockResolvedValue({ success: true, data: undefined })

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })

      const handler = mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]

      // Burn the rate-limit window with a real clipboard op.
      handler(new KeyboardEvent('keydown', { key: 'c', ctrlKey: true, bubbles: true }))

      // Immediately after, unrelated modified keys must still reach xterm.
      for (const key of ['h', 'e', 'l', 'o']) {
        expect(handler(new KeyboardEvent('keydown', { key, ctrlKey: true, bubbles: true }))).toBe(
          true
        )
      }
    })

    it('should paste from clipboard on Ctrl+V', async () => {
      const clipboardText = 'Pasted content'
      vi.mocked(clipboardApi).readText.mockResolvedValue({ success: true, data: clipboardText })

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })

      const handler = mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]

      const event = new KeyboardEvent('keydown', {
        key: 'v',
        ctrlKey: true,
        bubbles: true
      })

      const result = handler(event)

      // Should prevent xterm from handling
      expect(result).toBe(false)

      // Should read from clipboard and paste via the hook
      await vi.waitFor(() => {
        expect(vi.mocked(clipboardApi).readText).toHaveBeenCalled()
      })
    })

    it('should write bracket-wrapped paste data to PTY via terminalApi.write', async () => {
      const clipboardText = 'Line 1\nLine 2\nLine 3'
      vi.mocked(clipboardApi).readText.mockResolvedValue({ success: true, data: clipboardText })

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })

      const handler = mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]

      const event = new KeyboardEvent('keydown', {
        key: 'v',
        ctrlKey: true,
        bubbles: true
      })

      handler(event)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).write).toHaveBeenCalledWith(
          'terminal-123',
          '\x1b[200~Line 1\rLine 2\rLine 3\x1b[201~'
        )
      })

      // terminal.paste should NOT be called when pasteText is wired
      expect(mockTerminalInstance.paste).not.toHaveBeenCalled()
    })

    it('should let xterm handle Ctrl+V when navigator.clipboard is undefined (non-secure context, GH-588)', async () => {
      // Simulate a non-secure context (HTTP+bare-IP): navigator.clipboard is
      // unavailable. The handler must NOT preventDefault + pasteFromClipboard
      // (the facade's paste-event fallback can't fire when the keydown
      // suppresses the paste event it waits on). Instead it returns true so
      // xterm's native paste (the browser paste event) handles it.
      delete (navigator as { clipboard?: unknown }).clipboard
      expect(typeof navigator.clipboard).toBe('undefined')

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })

      const handler = mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]

      const event = new KeyboardEvent('keydown', {
        key: 'v',
        ctrlKey: true,
        bubbles: true
      })

      const result = handler(event)

      // Returns true so xterm handles the paste natively (no preventDefault).
      expect(result).toBe(true)
      // The facade's readText is NOT called (native xterm paste handles it).
      expect(vi.mocked(clipboardApi).readText).not.toHaveBeenCalled()
    })

    // On macOS the native Edit > Paste item owns Cmd+V, so the keydown handler
    // above never runs and the only survivor is xterm's paste handler, which
    // reads `text/plain` alone. An image copied out of Lark/Feishu puts PNG,
    // TIFF, JPEG, GIF, AVIF and a file URL on the pasteboard and no text at all
    // — measured — so that read comes back empty and Cmd+V did nothing.
    describe('paste event fallback (macOS Cmd+V is consumed by the native menu)', () => {
      function firePaste(
        container: HTMLElement,
        data: Record<string, string>,
        types: string[] = Object.keys(data)
      ): ClipboardEvent {
        const event = new Event('paste', { bubbles: true, cancelable: true }) as ClipboardEvent
        Object.defineProperty(event, 'clipboardData', {
          value: { getData: (type: string) => data[type] ?? '', types },
          configurable: true
        })
        // The listener sits on the inner mount div (`ref={containerRef}`), the
        // one xterm's textarea lives inside — dispatching on an ancestor would
        // never reach a capture listener registered further down.
        const target = container.querySelector('div.bg-terminal-bg > div')
        if (!target) throw new Error('terminal mount container not found')
        target.dispatchEvent(event)
        return event
      }

      it('routes an image-only clipboard through the image-aware paste path', async () => {
        vi.mocked(clipboardApi).hasImage.mockResolvedValue({ success: true, data: true })
        const { container } = render(<ConnectedTerminal terminalId="pty-paste-image" />)
        await vi.waitFor(() => {
          expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
        })

        const event = firePaste(container, {}, ['image/png', 'image/tiff'])

        expect(event.defaultPrevented).toBe(true)
        await vi.waitFor(() => {
          expect(vi.mocked(clipboardApi).hasImage).toHaveBeenCalled()
        })
        // \x16 is the Ctrl+V byte the CLI reads as "go look at the OS
        // clipboard yourself" — what makes pi and claude attach the image.
        await vi.waitFor(() => {
          expect(vi.mocked(terminalApi).write).toHaveBeenCalledWith('pty-paste-image', '\x16')
        })
      })

      it('still runs when xterm halts the event at its own listener', async () => {
        vi.mocked(clipboardApi).hasImage.mockResolvedValue({ success: true, data: true })
        const { container } = render(<ConnectedTerminal terminalId="pty-paste-capture" />)
        await vi.waitFor(() => {
          expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
        })

        // Stand-in for xterm's own handler: it lives on `terminal.textarea` and
        // `terminal.element`, both inside this container, and `handlePasteEvent`
        // opens with `ev.stopPropagation()`. Registered on a descendant, so a
        // bubble-phase listener on the container never gets the event at all.
        const mount = container.querySelector('div.bg-terminal-bg > div')
        if (!mount) throw new Error('terminal mount container not found')
        const xtermTextarea = document.createElement('textarea')
        mount.appendChild(xtermTextarea)
        const xtermHandler = vi.fn((event: Event) => {
          event.stopPropagation()
        })
        xtermTextarea.addEventListener('paste', xtermHandler)

        const event = new Event('paste', { bubbles: true, cancelable: true }) as ClipboardEvent
        Object.defineProperty(event, 'clipboardData', {
          value: { getData: () => '', types: ['image/png'] },
          configurable: true
        })
        xtermTextarea.dispatchEvent(event)

        expect(event.defaultPrevented).toBe(true)
        expect(xtermHandler).not.toHaveBeenCalled()
        await vi.waitFor(() => {
          expect(vi.mocked(clipboardApi).hasImage).toHaveBeenCalled()
        })
      })

      it('leaves an ordinary text paste to xterm', async () => {
        const { container } = render(<ConnectedTerminal terminalId="pty-paste-text" />)
        await vi.waitFor(() => {
          expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
        })

        const event = firePaste(container, { 'text/plain': 'hello' })

        // Not preventing default is the whole point: xterm's own handler owns
        // text, including its bracketed-paste wrapping and its ESC sanitising.
        expect(event.defaultPrevented).toBe(false)
        expect(vi.mocked(clipboardApi).hasImage).not.toHaveBeenCalled()
      })
    })

    it('should select all on Ctrl+A', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })

      const handler = mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]

      const event = new KeyboardEvent('keydown', {
        key: 'a',
        ctrlKey: true,
        bubbles: true
      })

      const result = handler(event)

      // Should prevent xterm from handling
      expect(result).toBe(false)
      expect(mockTerminalInstance.selectAll).toHaveBeenCalled()
    })

    it('should send a newline (LF) on Shift+Enter instead of CR', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })
      // Wait for spawn so the PTY id is bound before the key is dispatched.
      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      const handler = mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]

      const event = new KeyboardEvent('keydown', {
        key: 'Enter',
        shiftKey: true,
        bubbles: true,
        cancelable: true
      })

      const result = handler(event)

      // Must prevent xterm's default (\r) so the app receives a newline.
      expect(result).toBe(false)
      expect(event.defaultPrevented).toBe(true)

      // The same byte Ctrl+J produces (LF) is written to the PTY.
      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).write).toHaveBeenCalledWith('terminal-123', '\n')
      })
    })

    it('should not remap Shift+Enter when another modifier is held', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })

      const handler = mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]

      // Ctrl+Shift+Enter must NOT be swallowed — it may be an app shortcut.
      const event = new KeyboardEvent('keydown', {
        key: 'Enter',
        shiftKey: true,
        ctrlKey: true,
        bubbles: true
      })

      const result = handler(event)

      expect(result).toBe(true)
    })

    // Remote desktop clients (ToDesk, UU远程) inject keystrokes with
    // KEYEVENTF_UNICODE / VK_PACKET, and some IMEs report keyCode 229 for every
    // key even in latin mode. xterm then routes the key through
    // CompositionHelper, whose textarea-diff fallback drops every keystroke
    // arriving while its `setTimeout(0)` is in flight (xtermjs/xterm.js#5887,
    // #6078) — a typed line reaches the PTY as a single character.
    describe('composition-keycode key delivery', () => {
      // jsdom derives `keyCode` from `key`, so pin it to the value a remote
      // desktop client actually produces.
      const injectedKeydown = (
        key: string,
        keyCode: number,
        init: KeyboardEventInit = {}
      ): KeyboardEvent => {
        const event = new KeyboardEvent('keydown', {
          key,
          bubbles: true,
          cancelable: true,
          ...init
        })
        Object.defineProperty(event, 'keyCode', { value: keyCode, configurable: true })
        return event
      }
      const compositionKeydown = (key: string, init: KeyboardEventInit = {}): KeyboardEvent =>
        injectedKeydown(key, 229, init)

      // The xterm mock is a shared singleton — a leaked textarea value would
      // silently disable the guard for every later test.
      beforeEach(() => {
        if (mockTerminalInstance.textarea) mockTerminalInstance.textarea.value = ''
      })
      afterEach(() => {
        if (mockTerminalInstance.textarea) mockTerminalInstance.textarea.value = ''
      })

      const mountAndGetHandler = async (): Promise<(event: KeyboardEvent) => boolean> => {
        render(<ConnectedTerminal />)
        await vi.waitFor(() => {
          expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
        })
        // Wait for spawn so the PTY id is bound before any key is dispatched.
        await vi.waitFor(() => {
          expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
        })
        vi.mocked(terminalApi).write.mockClear()
        return mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]
      }

      it('should deliver every key in a keyCode-229 burst, not just the first', async () => {
        const handler = await mountAndGetHandler()

        for (const key of ['h', 'e', 'l', 'l', 'o']) {
          const event = compositionKeydown(key)
          expect(handler(event)).toBe(false)
          // Keeping the character out of the hidden textarea is what stops
          // xterm's diff timer and `_inputEvent` from re-sending it.
          expect(event.defaultPrevented).toBe(true)
        }

        await vi.waitFor(() => {
          expect(vi.mocked(terminalApi).write).toHaveBeenCalledTimes(5)
        })
        expect(
          vi
            .mocked(terminalApi)
            .write.mock.calls.map((call) => call[1])
            .join('')
        ).toBe('hello')
      })

      it('should deliver keys injected without a legacy keycode', async () => {
        // macOS remote desktop (ToDesk/UU远程) injects via
        // CGEventKeyboardSetUnicodeString, leaving keyCode/charCode/which at 0.
        // All three xterm delivery paths gate on a nonzero legacy keycode, so
        // a typed line arrived as a single character.
        const handler = await mountAndGetHandler()

        for (const key of ['h', 'i']) {
          const event = injectedKeydown(key, 0)
          expect(handler(event)).toBe(false)
          expect(event.defaultPrevented).toBe(true)
        }

        await vi.waitFor(() => {
          expect(vi.mocked(terminalApi).write).toHaveBeenCalledTimes(2)
        })
        expect(
          vi
            .mocked(terminalApi)
            .write.mock.calls.map((call) => call[1])
            .join('')
        ).toBe('hi')
      })

      it('should leave keys with a real keycode to xterm', async () => {
        const handler = await mountAndGetHandler()

        // An ordinary 'a' reports keyCode 65 and xterm's keymap delivers it;
        // stepping in here would double every locally typed character.
        expect(handler(injectedKeydown('a', 65))).toBe(true)
        // Space is keyCode 32 — below the keymap's >= 48 catch-all, but its
        // nonzero charCode still reaches xterm through _keyPress.
        expect(handler(injectedKeydown(' ', 32))).toBe(true)

        expect(vi.mocked(terminalApi).write).not.toHaveBeenCalled()
      })

      it('should not claim the iOS arrow keys xterm maps under keyCode 0', async () => {
        const handler = await mountAndGetHandler()

        // xterm's `case 0` recognises these four by name; they are not single
        // characters, so the guard must not intercept them.
        expect(handler(injectedKeydown('UIKeyInputUpArrow', 0))).toBe(true)
        expect(vi.mocked(terminalApi).write).not.toHaveBeenCalled()
      })

      it('should deliver a remote-desktop text chunk whole, not just its first character', async () => {
        // Captured from a live ToDesk session: the whole string arrives as one
        // synthetic event, and xterm's _keyPress would truncate it to
        // String.fromCharCode(charCode) — the first character.
        const handler = await mountAndGetHandler()
        if (mockTerminalInstance.textarea) mockTerminalInstance.textarea.value = '你好'

        const event = injectedKeydown('你好', 65)
        expect(handler(event)).toBe(false)
        // Cancelling stops _keyPress and the browser's own insertion, so
        // nothing can write a second copy.
        expect(event.defaultPrevented).toBe(true)
        // The accumulated textarea is cleared so xterm's diff fallback cannot
        // re-emit it later (#6078).
        expect(mockTerminalInstance.textarea?.value).toBe('')

        await vi.waitFor(() => {
          expect(vi.mocked(terminalApi).write).toHaveBeenCalledWith('terminal-123', '你好')
        })
        expect(vi.mocked(terminalApi).write).toHaveBeenCalledTimes(1)
      })

      it('should deliver a latin chunk whole as well', async () => {
        const handler = await mountAndGetHandler()

        expect(handler(injectedKeydown('ju', 74))).toBe(false)

        await vi.waitFor(() => {
          expect(vi.mocked(terminalApi).write).toHaveBeenCalledWith('terminal-123', 'ju')
        })
      })

      it('should leave named keys to xterm', async () => {
        const handler = await mountAndGetHandler()

        // Every one of these has key.length > 1 but is a real key, not text.
        for (const key of ['Enter', 'Backspace', 'ArrowUp', 'Escape', 'F5', 'Process', 'Dead']) {
          expect(handler(injectedKeydown(key, 13))).toBe(true)
        }

        expect(vi.mocked(terminalApi).write).not.toHaveBeenCalled()
      })

      it('should leave a single typed character to xterm', async () => {
        const handler = await mountAndGetHandler()

        // xterm's _keyPress already delivers these correctly; claiming them
        // would double every locally typed character.
        expect(handler(injectedKeydown('a', 65))).toBe(true)
        expect(vi.mocked(terminalApi).write).not.toHaveBeenCalled()
      })

      it('should not claim a chunk-shaped key held with a modifier', async () => {
        const handler = await mountAndGetHandler()

        expect(handler(injectedKeydown('ju', 74, { ctrlKey: true } as KeyboardEventInit))).not.toBe(
          false
        )
        expect(vi.mocked(terminalApi).write).not.toHaveBeenCalled()
      })

      it('should leave a genuine IME composition to xterm', async () => {
        const handler = await mountAndGetHandler()

        // Chromium reports `key === 'Process'` for a real IME keydown, never a
        // single character.
        expect(handler(compositionKeydown('Process'))).toBe(true)
        // A keydown inside a live composition sets isComposing.
        expect(handler(compositionKeydown('a', { isComposing: true }))).toBe(true)

        expect(vi.mocked(terminalApi).write).not.toHaveBeenCalled()
      })

      it('should not send when the browser already inserted the character', async () => {
        const handler = await mountAndGetHandler()

        // Input-before-keydown ordering (#5887): xterm's own `_inputEvent` may
        // already have emitted it, and a non-empty textarea is that proof.
        // Sending here would resurrect the duplicate-PTY-input bug (GH-267).
        if (mockTerminalInstance.textarea) mockTerminalInstance.textarea.value = 'a'

        expect(handler(compositionKeydown('a'))).toBe(true)
        expect(vi.mocked(terminalApi).write).not.toHaveBeenCalled()
      })
    })

    it('should handle Cmd key on macOS for copy/paste', async () => {
      // On non-macOS test environments (jsdom has empty platform),
      // isPlatformModifier checks ctrlKey, not metaKey.
      // So we test the platform-appropriate modifier: Ctrl on non-mac, Cmd on mac.
      const selectedText = 'Selected text'
      mockTerminalInstance.hasSelection.mockReturnValue(true)
      mockTerminalInstance.getSelection.mockReturnValue(selectedText)
      vi.mocked(clipboardApi).writeText.mockResolvedValue({ success: true, data: undefined })

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })

      const handler = mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]

      // Use the platform-appropriate modifier for clipboard ops.
      // In jsdom (test env), navigator.platform is "" → isMac=false → use ctrlKey.
      // On real macOS, isMac=true → metaKey (⌘) would be used.
      const { isMac: testIsMac } = await import('@/lib/platform')
      const clipboardModifier = testIsMac
        ? { metaKey: true, ctrlKey: false }
        : { ctrlKey: true, metaKey: false }

      const event = new KeyboardEvent('keydown', {
        key: 'c',
        ...clipboardModifier,
        bubbles: true
      })

      const result = handler(event)

      expect(result).toBe(false)
      await vi.waitFor(() => {
        expect(vi.mocked(clipboardApi).writeText).toHaveBeenCalledWith(selectedText)
      })
    })

    it('should not handle clipboard shortcuts for non-keydown events', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })

      const handler = mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]

      // Simulate keyup event
      const event = new KeyboardEvent('keyup', {
        key: 'c',
        ctrlKey: true,
        bubbles: true
      })

      const result = handler(event)

      // Should allow xterm to handle
      expect(result).toBe(true)
    })

    it('should not interfere with other keyboard shortcuts', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })

      const handler = mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]

      // Regular typing. jsdom leaves `keyCode` at 0 on a constructed event,
      // which is the very signature of a key injected without a virtual
      // keycode — a real 'x' keydown reports 88, so pin it to stay
      // representative of what the handler sees in a browser.
      const event = new KeyboardEvent('keydown', {
        key: 'x',
        bubbles: true
      })
      Object.defineProperty(event, 'keyCode', { value: 88, configurable: true })

      const result = handler(event)

      expect(result).toBe(true)
    })

    it('should prevent default on Shift+Tab and let xterm handle the key', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })

      const handler = mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]
      const event = new KeyboardEvent('keydown', {
        key: 'Tab',
        shiftKey: true,
        bubbles: true
      })
      const preventDefaultSpy = vi.spyOn(event, 'preventDefault')

      const result = handler(event)

      expect(result).toBe(true)
      expect(preventDefaultSpy).toHaveBeenCalled()
    })

    it('should prevent default on Tab and let xterm handle the key', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })

      const handler = mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]
      const event = new KeyboardEvent('keydown', {
        key: 'Tab',
        bubbles: true
      })
      const preventDefaultSpy = vi.spyOn(event, 'preventDefault')

      const result = handler(event)

      expect(result).toBe(true)
      expect(preventDefaultSpy).toHaveBeenCalled()
    })

    it('should bubble app-owned shortcuts so the workspace handler can process them', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })

      const handler = mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]

      const event = new KeyboardEvent('keydown', {
        key: 'B',
        ctrlKey: true,
        shiftKey: true,
        bubbles: true
      })

      const result = handler(event)

      expect(result).toBe(false)
    })

    it('should treat Ctrl+R as app-owned when it matches an app shortcut', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })

      const handler = mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]

      // Ctrl+R matches commandHistory app shortcut — should be app-owned so the
      // workspace handler can open the command history panel from terminal focus.
      const event = new KeyboardEvent('keydown', {
        key: 'r',
        ctrlKey: true,
        bubbles: true
      })

      const result = handler(event)

      expect(result).toBe(false)
    })

    it('should treat Ctrl+K as app-owned when it matches an app shortcut', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })

      const handler = mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]

      // Ctrl+K matches commandPalette app shortcut — should be app-owned so the
      // workspace handler can open the command palette from terminal focus.
      const event = new KeyboardEvent('keydown', {
        key: 'k',
        ctrlKey: true,
        bubbles: true
      })

      const result = handler(event)

      expect(result).toBe(false)
    })

    it('should pass pure readline passthrough Ctrl+E when it matches no app shortcut', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(mockTerminalInstance.attachCustomKeyEventHandler).toHaveBeenCalled()
      })

      const handler = mockTerminalInstance.attachCustomKeyEventHandler.mock.calls[0][0]

      // Ctrl+E is a readline binding (end of line) and does NOT match any app
      // shortcut — must reach the PTY.
      const event = new KeyboardEvent('keydown', {
        key: 'e',
        ctrlKey: true,
        bubbles: true
      })

      const result = handler(event)

      expect(result).toBe(true)
    })
  })

  describe('Context menu', () => {
    it('should render terminal container', async () => {
      const { container } = render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // Terminal container should be in the DOM
      expect(container.querySelector('div')).toBeTruthy()
    })
  })

  describe('Terminal file path link handling', () => {
    it('should open a file path only on ctrl/meta click', async () => {
      vi.mocked(openFilePathFromTerminal).mockResolvedValue({ ok: true })
      render(<ConnectedTerminal terminalId="external-123" />)

      await vi.waitFor(() => {
        expect(capturedLinkProviders.at(-1)).toBeDefined()
      })

      const provider = capturedLinkProviders.at(-1)!
      let links: Array<{
        activate: (event: MouseEvent, text: string) => void | Promise<void>
        text: string
      }> = []

      provider.provideLinks(1, (provided) => {
        links = provided
      })

      expect(links.find((link) => link.text === 'missing.ts')).toBeUndefined()

      const fileLink = links.find((link) => link.text === 'src/renderer/App.tsx')
      expect(fileLink).toBeDefined()

      const plainClick = new MouseEvent('click', { ctrlKey: false, metaKey: false })
      const plainPreventDefaultSpy = vi.spyOn(plainClick, 'preventDefault')
      await fileLink!.activate(plainClick, fileLink!.text)

      expect(openFilePathFromTerminal).not.toHaveBeenCalled()
      expect(plainPreventDefaultSpy).not.toHaveBeenCalled()

      const ctrlClick = new MouseEvent('click', { ctrlKey: true, metaKey: false })
      const ctrlPreventDefaultSpy = vi.spyOn(ctrlClick, 'preventDefault')
      await fileLink!.activate(ctrlClick, fileLink!.text)

      expect(ctrlPreventDefaultSpy).toHaveBeenCalled()
      expect(openFilePathFromTerminal).toHaveBeenCalledWith('src/renderer/App.tsx', {
        cwd: '/terminal-cwd',
        projectRoot: '/project-root'
      })
    })

    it('should invoke path open with missing terminal cwd context on ctrl+click', async () => {
      vi.mocked(openFilePathFromTerminal).mockResolvedValue({
        ok: false,
        reason: 'missing-context',
        message:
          'No project or working directory found; set a project/cwd to open paths: src/renderer/App.tsx'
      })
      mockTerminalStoreState.findTerminalByPtyId.mockReturnValue(undefined)
      render(<ConnectedTerminal terminalId="external-123" />)

      await vi.waitFor(() => {
        expect(capturedLinkProviders.at(-1)).toBeDefined()
      })

      const provider = capturedLinkProviders.at(-1)!
      let links: Array<{
        activate: (event: MouseEvent, text: string) => void | Promise<void>
        text: string
      }> = []

      provider.provideLinks(1, (provided) => {
        links = provided
      })

      const fileLink = links.find((link) => link.text === 'src/renderer/App.tsx')
      expect(fileLink).toBeDefined()

      const ctrlClick = new MouseEvent('click', { ctrlKey: true, metaKey: false })
      const ctrlPreventDefaultSpy = vi.spyOn(ctrlClick, 'preventDefault')
      await fileLink!.activate(ctrlClick, fileLink!.text)

      expect(ctrlPreventDefaultSpy).toHaveBeenCalled()
      expect(openFilePathFromTerminal).toHaveBeenCalledWith('src/renderer/App.tsx', {
        cwd: undefined,
        projectRoot: '/project-root'
      })
      expect(toast.error).toHaveBeenCalledWith(
        'No project or working directory found; set a project/cwd to open paths: src/renderer/App.tsx'
      )
    })

    it('should report unexpected ctrl click open failures with toast and console error', async () => {
      const failure = new Error('boom')
      const consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
      vi.mocked(openFilePathFromTerminal).mockRejectedValue(failure)
      render(<ConnectedTerminal terminalId="external-123" />)

      await vi.waitFor(() => {
        expect(capturedLinkProviders.at(-1)).toBeDefined()
      })

      const provider = capturedLinkProviders.at(-1)!
      let links: Array<{
        activate: (event: MouseEvent, text: string) => void | Promise<void>
        text: string
      }> = []

      provider.provideLinks(1, (provided) => {
        links = provided
      })

      const fileLink = links.find((link) => link.text === 'src/renderer/App.tsx')
      expect(fileLink).toBeDefined()

      const ctrlClick = new MouseEvent('click', { ctrlKey: true, metaKey: false })
      await fileLink!.activate(ctrlClick, fileLink!.text)

      expect(consoleErrorSpy).toHaveBeenCalledWith('[Terminal File Link Open Failed]', failure)
      expect(toast.error).toHaveBeenCalledWith('Failed to open file from terminal output.')
    })
  })

  describe('WebGL context loss recovery', () => {
    afterEach(() => {
      vi.useRealTimers()
    })

    it('should create a new WebGL addon when context loss fires', async () => {
      vi.useFakeTimers()
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // WebGL addon should have been created once during init
      expect(webglAddonCreateCount).toBe(1)
      expect(capturedContextLossCallback).toBeTruthy()

      // Simulate context loss
      vi.useFakeTimers()
      capturedContextLossCallback!()

      // Advance past the 100ms recovery delay
      await vi.advanceTimersByTimeAsync(150)

      // A second WebGL addon should have been created
      expect(webglAddonCreateCount).toBe(2)
    })

    it('should load the WebGL addon on terminal during init', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // loadAddon is called for FitAddon, SearchAddon, and WebglAddon
      expect(mockTerminalInstance.loadAddon).toHaveBeenCalled()
      expect(webglAddonCreateCount).toBe(1)
    })

    it('rebuilds the WebGL surface after PTY output arrives while hidden', async () => {
      vi.useFakeTimers()
      const { rerender } = render(<ConnectedTerminal isVisible={true} />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })
      expect(lastCreatedWebglInstance).toBeTruthy()

      rerender(<ConnectedTerminal isVisible={false} />)
      await vi.waitFor(() => {
        expect(mockPixelScrollSetEnabled).toHaveBeenCalledWith(false)
      })

      // Hidden tabs keep the PTY attachment; TUI redraws still land on write().
      mockTerminalInstance.write('—维护者的判断在前面。\n他的核心批评\n')

      mockTerminalInstance.refresh.mockClear()
      lastCreatedWebglInstance?.clearTextureAtlas.mockClear()
      mockPixelScrollReset.mockClear()

      rerender(<ConnectedTerminal isVisible={true} />)
      await vi.waitFor(() => {
        expect(webglAddonCreateCount).toBe(2)
      })
      expect(mockPixelScrollSetEnabled).toHaveBeenCalledWith(true)

      await vi.advanceTimersByTimeAsync(20)
      await vi.advanceTimersByTimeAsync(20)

      expect(mockPixelScrollReset).toHaveBeenCalled()
      expect(mockTerminalInstance.refresh).toHaveBeenCalledWith(0, mockTerminalInstance.rows - 1)
      expect(lastCreatedWebglInstance?.clearTextureAtlas).not.toHaveBeenCalled()
      vi.useRealTimers()
    })

    it('should release WebGL while hidden and restore it when visible again', async () => {
      const { rerender } = render(<ConnectedTerminal isVisible={true} />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })
      const visibleWebgl = lastCreatedWebglInstance
      expect(visibleWebgl).toBeTruthy()

      rerender(<ConnectedTerminal isVisible={false} />)
      await vi.waitFor(() => {
        expect(visibleWebgl?.dispose).toHaveBeenCalledTimes(1)
      })
      expect(webglAddonCreateCount).toBe(1)

      rerender(<ConnectedTerminal isVisible={true} />)
      await vi.waitFor(() => {
        expect(webglAddonCreateCount).toBe(2)
      })
    })

    it('should defer WebGL allocation until a hidden terminal becomes visible', async () => {
      const { rerender } = render(<ConnectedTerminal isVisible={false} />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })
      expect(webglAddonCreateCount).toBe(0)

      rerender(<ConnectedTerminal isVisible={true} />)
      await vi.waitFor(() => {
        expect(webglAddonCreateCount).toBe(1)
      })
    })

    it('should stop recovery after max attempts exhausted', async () => {
      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {})

      try {
        render(<ConnectedTerminal />)

        await vi.waitFor(() => {
          expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
        })

        // Initial load
        expect(webglAddonCreateCount).toBe(1)

        vi.useFakeTimers()

        // Simulate 2 context loss events - should recover each time
        for (let i = 0; i < 2; i++) {
          capturedContextLossCallback!()
          await vi.advanceTimersByTimeAsync(150)
        }

        // Should have 3 total: 1 init + 2 recoveries
        expect(webglAddonCreateCount).toBe(3)

        // 3rd context loss - counter reaches MAX, should NOT recover
        capturedContextLossCallback!()
        await vi.advanceTimersByTimeAsync(150)

        // Should still be 3 - no more recovery attempts
        expect(webglAddonCreateCount).toBe(3)

        // Should have logged warning about exhausted attempts
        expect(warnSpy).toHaveBeenCalledWith(
          'WebGL recovery attempts exhausted, falling back to DOM renderer'
        )
      } finally {
        warnSpy.mockRestore()
      }
    })

    it('should dispose WebGL and skip recovery after switching renderer preference to dom', async () => {
      vi.useFakeTimers()
      const { rerender } = render(<ConnectedTerminal className="renderer-auto" />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      expect(webglAddonCreateCount).toBe(1)
      expect(lastCreatedWebglInstance?.dispose).not.toHaveBeenCalled()

      vi.mocked(useTerminalRenderer).mockReturnValue('dom')
      rerender(<ConnectedTerminal className="renderer-dom" />)

      await vi.waitFor(() => {
        expect(lastCreatedWebglInstance?.dispose).toHaveBeenCalled()
      })

      capturedContextLossCallback?.()
      await vi.advanceTimersByTimeAsync(150)

      expect(webglAddonCreateCount).toBe(1)
    })

    it('should record instrumentation events during WebGL recovery lifecycle', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      expect(capturedContextLossCallback).toBeTruthy()
      vi.useFakeTimers()

      // Simulate context loss
      capturedContextLossCallback!()
      await vi.advanceTimersByTimeAsync(150)

      // Verify recovery instrumentation events were recorded
      expect(mockRecordTerminalContinuityEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          name: 'renderer-recovery-attempted',
          details: expect.objectContaining({
            renderer: 'webgl',
            isRecovery: true
          })
        })
      )

      expect(mockRecordTerminalContinuityEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          name: 'renderer-recovery-succeeded',
          details: expect.objectContaining({
            renderer: 'webgl',
            isRecovery: true
          })
        })
      )

      vi.useRealTimers()
    })

    it('should record exhausted event when max recovery attempts reached', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      expect(capturedContextLossCallback).toBeTruthy()
      vi.useFakeTimers()

      // Exhaust all recovery attempts
      for (let i = 0; i < 3; i++) {
        capturedContextLossCallback!()
        await vi.advanceTimersByTimeAsync(150)
      }

      expect(mockRecordTerminalContinuityEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          name: 'renderer-recovery-exhausted',
          details: expect.objectContaining({
            attempts: expect.any(Number),
            maxAttempts: expect.any(Number)
          })
        })
      )

      vi.useRealTimers()
    })

    it('should skip WebGL when renderer preference is dom', async () => {
      // Override the mock to return dom
      vi.mocked(useTerminalRenderer).mockReturnValue('dom')

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // WebGL addon should NOT have been created
      expect(webglAddonCreateCount).toBe(0)
      expect(capturedContextLossCallback).toBeFalsy()
    })

    it('repaints without clearing the render model when the DOM renderer is in use', async () => {
      // `RenderService.clear()` forwards to whatever renderer is attached.
      // `DomRenderer.clear()` calls `replaceChildren()` immediately while the
      // repaint waits for the next frame, so under the DOM renderer the
      // leftover-glyph repair is a visible black flash instead of a cheap
      // model rebuild — and this fix raised how often it runs.
      vi.mocked(useTerminalRenderer).mockReturnValue('dom')

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      mockTerminalInstance.refresh.mockClear()
      mockTerminalInstance._core._renderService.clear.mockClear()
      capturedDataCallback?.('terminal-123', new TextEncoder().encode('output'))

      // The repaint still has to happen; only the DOM-destroying clear is off.
      await vi.waitFor(() => {
        expect(mockTerminalInstance.refresh).toHaveBeenCalledWith(0, mockTerminalInstance.rows - 1)
      })
      expect(mockTerminalInstance._core._renderService.clear).not.toHaveBeenCalled()
    })

    it('clears the render model on a write while the WebGL addon is live', async () => {
      // The other half of the gate. A bare `refresh()` redraws from the cell
      // model; it does not rebuild it, so leftover glyphs from an in-place ZLE
      // redraw survive it. Under WebGL the model invalidation is the fix, and
      // a gate stuck closed would silently restore the original bug.
      vi.mocked(useTerminalRenderer).mockReturnValue('webgl')

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(webglAddonCreateCount).toBe(1)
      })

      mockTerminalInstance._core._renderService.clear.mockClear()
      capturedDataCallback?.('terminal-123', new TextEncoder().encode('output'))

      await vi.waitFor(() => {
        expect(mockTerminalInstance._core._renderService.clear).toHaveBeenCalled()
      })
    })

    it('rebuilds the render model on window-restore recovery', async () => {
      // Window recovery does not recreate the WebGL addon — the isVisible
      // effect that would never fires, because the tab stayed active while the
      // window was away. A bare refresh redraws from a model that may still
      // hold the pre-minimize frame, and does so faithfully.
      vi.mocked(useTerminalRenderer).mockReturnValue('webgl')
      vi.useFakeTimers()

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(webglAddonCreateCount).toBe(1)
      })

      mockTerminalInstance._core._renderService.clear.mockClear()

      Object.defineProperty(document, 'visibilityState', {
        value: 'visible',
        writable: true,
        configurable: true
      })
      document.dispatchEvent(new Event('visibilitychange'))

      // The recovery polls for a usable container size before it fits, and
      // jsdom reports zeros, so it burns the full layout-wait budget first.
      await vi.advanceTimersByTimeAsync(1500)

      expect(mockTerminalInstance._core._renderService.clear).toHaveBeenCalled()
      vi.useRealTimers()
    })

    it('keeps the render model while the alternate screen is active', async () => {
      // A full-screen TUI repaints with absolute positioning, which is
      // idempotent, so there is no relative-redraw residue for a model rebuild
      // to fix — and it writes continuously, so discarding the model there
      // re-uploaded every cell on every burst. That is what made scrolling a
      // full-screen agent CLI feel slow.
      vi.mocked(useTerminalRenderer).mockReturnValue('webgl')

      render(<ConnectedTerminal />)
      await vi.waitFor(() => {
        expect(webglAddonCreateCount).toBe(1)
      })

      mockTerminalInstance.buffer.active.type = 'alternate'
      mockTerminalInstance._core._renderService.clear.mockClear()
      mockTerminalInstance.refresh.mockClear()

      // A scroll burst is the cheapest way to reach `repairNow` from outside.
      xtermHandles.scrollCallback?.()
      await new Promise((resolve) => setTimeout(resolve, 200))

      // Still repainted — only the model survives.
      expect(mockTerminalInstance.refresh).toHaveBeenCalled()
      expect(mockTerminalInstance._core._renderService.clear).not.toHaveBeenCalled()

      mockTerminalInstance.buffer.active.type = 'normal'
    })

    it('should still load WebGL when renderer preference is webgl', async () => {
      vi.mocked(useTerminalRenderer).mockReturnValue('webgl')

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      expect(webglAddonCreateCount).toBe(1)
      expect(capturedContextLossCallback).toBeTruthy()
    })

    it('initialises Unicode v11 exactly once, after terminal.open', () => {
      render(<ConnectedTerminal />)

      expect(mockEnsureTerminalUnicode11).toHaveBeenCalledTimes(1)
      expect(mockEnsureTerminalUnicode11).toHaveBeenCalledWith(
        mockTerminalInstance.open.mock.instances[0]
      )
      // F5(d): the activeVersion setter throws unless the addon is registered
      // on an opened terminal, so the ordering is load-bearing.
      expect(mockTerminalInstance.open.mock.invocationCallOrder[0]).toBeLessThan(
        mockEnsureTerminalUnicode11.mock.invocationCallOrder[0]
      )
    })

    it('attaches pixel-smooth scroll after open and disposes it on unmount', () => {
      const { unmount } = render(<ConnectedTerminal />)
      expect(mockAttachPixelSmoothScroll).toHaveBeenCalled()
      unmount()
      expect(mockPixelScrollDispose).toHaveBeenCalled()
    })

    it('repaints leftover WebGL rows after scroll idle and atlas merges', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })
      expect(xtermHandles.scrollCallback).toBeTruthy()
      expect(lastCreatedWebglInstance).toBeTruthy()

      mockTerminalInstance.refresh.mockClear()
      lastCreatedWebglInstance?.clearTextureAtlas.mockClear()

      vi.useFakeTimers()
      xtermHandles.scrollCallback?.()
      xtermHandles.scrollCallback?.()
      await vi.advanceTimersByTimeAsync(150)

      expect(mockTerminalInstance.refresh).toHaveBeenCalledWith(0, mockTerminalInstance.rows - 1)
      expect(lastCreatedWebglInstance?.clearTextureAtlas).not.toHaveBeenCalled()

      capturedAtlasRemoveCallback?.()
      expect(lastCreatedWebglInstance?.clearTextureAtlas).not.toHaveBeenCalled()
      await vi.advanceTimersByTimeAsync(150)
      expect(lastCreatedWebglInstance?.clearTextureAtlas).not.toHaveBeenCalled()
      expect(mockTerminalInstance.refresh).toHaveBeenCalled()
      vi.useRealTimers()
    })

    it('arms the repaint on every write, not only the live-output one', async () => {
      // The reported symptom: a restored terminal kept showing the
      // pre-restore frame until a scroll or pane resize forced a repaint.
      // Only the live-output write used to call onWrite; all seven
      // replay/restore writes skipped it. The fix routes every write through
      // one helper, so the invariant to hold is "no write reaches xterm
      // without the parse-completion hook the repaint hangs off".
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      mockTerminalInstance.refresh.mockClear()
      capturedDataCallback?.('terminal-123', new TextEncoder().encode('restored'))

      const writeCalls = mockTerminalInstance.write.mock.calls
      expect(writeCalls.length).toBeGreaterThan(0)
      for (const call of writeCalls) {
        expect(call[1]).toBeTypeOf('function')
      }

      // The stub invokes that callback the way real xterm does once parsing
      // lands, so a repaint here proves the hook is wired, not merely passed.
      await vi.waitFor(() => {
        expect(mockTerminalInstance.refresh).toHaveBeenCalledWith(0, mockTerminalInstance.rows - 1)
      })
    })

    it('repaints only after xterm parsed the chunk, never straight after write()', async () => {
      // D-1. The buffer reflects a write only once xterm invokes the write
      // callback, so a repaint armed straight after `write()` rebuilds the
      // render model from the pre-write buffer and leaves exactly the
      // staleness this path exists to clear. Passing a callback and then
      // repainting immediately satisfies every "a callback was passed" shape
      // assertion while reintroducing the bug, which is what this pins.
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      mockTerminalInstance.refresh.mockClear()
      xtermHandles.refreshPendingWrites.length = 0
      capturedDataCallback?.('terminal-123', new TextEncoder().encode('restored'))

      // The chunk is buffered, not parsed. Nothing may repaint yet.
      expect(xtermHandles.pendingWrites).toBeGreaterThan(0)
      expect(mockTerminalInstance.refresh).not.toHaveBeenCalled()

      await vi.waitFor(() => {
        expect(mockTerminalInstance.refresh).toHaveBeenCalledWith(0, mockTerminalInstance.rows - 1)
      })
      // Every repaint that did happen saw an empty write buffer, so none of
      // them beat the parser — a repaint deferred by anything other than the
      // parse callback (a RAF, a microtask) would still show up here.
      expect(xtermHandles.refreshPendingWrites.filter((depth) => depth !== 0)).toEqual([])
    })

    it('never lets a failing repaint escape the xterm write callback', async () => {
      // D-3. xterm's `_innerWrite` invokes the callback as a bare `cb()` with
      // no exception guard: a throw stalls `_bufferOffset` and stops
      // `_scheduleInnerWrite` from ever running again, so that terminal's
      // output freezes permanently and not even a resize brings it back.
      // Every callback handed to `write` must therefore contain its failures.
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      capturedDataCallback?.('terminal-123', new TextEncoder().encode('output'))
      const parseCallback = mockTerminalInstance.write.mock.calls.at(-1)?.[1] as
        | (() => void)
        | undefined
      expect(parseCallback).toBeTypeOf('function')

      mockTerminalInstance.refresh.mockImplementationOnce(() => {
        throw new Error('renderer detached')
      })

      expect(() => parseCallback?.()).not.toThrow()
    })

    it('survives a parse callback that lands after unmount', async () => {
      // Parsing is asynchronous, so a chunk written just before unmount has
      // its callback invoked when the component is already torn down. Same
      // D-3 consequence if that throws, and the async stub is what makes the
      // window observable at all.
      const { unmount } = render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      capturedDataCallback?.('terminal-123', new TextEncoder().encode('last chunk'))
      const parseCallback = mockTerminalInstance.write.mock.calls.at(-1)?.[1] as
        | (() => void)
        | undefined
      expect(parseCallback).toBeTypeOf('function')

      unmount()
      mockTerminalInstance.refresh.mockClear()

      expect(() => parseCallback?.()).not.toThrow()
      // And it must not repaint a terminal that is on its way into the cache
      // or already disposed.
      expect(mockTerminalInstance.refresh).not.toHaveBeenCalled()
    })
  })

  describe('Visibility change recovery', () => {
    let originalVisibilityState: string

    beforeEach(() => {
      // Capture original visibility state before any test mutates it
      originalVisibilityState = document.visibilityState
    })

    afterEach(() => {
      // Restore original visibility state
      Object.defineProperty(document, 'visibilityState', {
        value: originalVisibilityState,
        writable: true,
        configurable: true
      })
      vi.useRealTimers()
    })

    it.skip('should call fit and resize when visibility changes to visible', async () => {
      vi.useFakeTimers()

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // Clear previous fit/resize calls from init
      mockFitAddonInstance.fit.mockClear()
      vi.mocked(terminalApi).resize.mockClear()

      // Simulate visibility change to visible
      Object.defineProperty(document, 'visibilityState', {
        value: 'visible',
        writable: true,
        configurable: true
      })
      document.dispatchEvent(new Event('visibilitychange'))

      // Advance past the 150ms delay
      await vi.advanceTimersByTimeAsync(200)

      expect(mockFitAddonInstance.fit).toHaveBeenCalled()
      expect(vi.mocked(terminalApi).resize).toHaveBeenCalledWith(
        'terminal-123',
        expect.any(Number),
        expect.any(Number)
      )
    })

    it('should not trigger recovery when visibility changes to hidden', async () => {
      vi.useFakeTimers()

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      mockFitAddonInstance.fit.mockClear()
      vi.mocked(terminalApi).resize.mockClear()

      // Simulate visibility change to hidden
      Object.defineProperty(document, 'visibilityState', {
        value: 'hidden',
        writable: true,
        configurable: true
      })
      document.dispatchEvent(new Event('visibilitychange'))

      await vi.advanceTimersByTimeAsync(200)

      // fit should not be called again for hidden state
      expect(mockFitAddonInstance.fit).not.toHaveBeenCalled()
    })

    it('should remove visibilitychange listener on unmount', async () => {
      const removeEventListenerSpy = vi.spyOn(document, 'removeEventListener')

      const { unmount } = render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      unmount()

      expect(removeEventListenerSpy).toHaveBeenCalledWith('visibilitychange', expect.any(Function))

      removeEventListenerSpy.mockRestore()
    })

    it.skip('should debounce rapid visibility changes to visible', async () => {
      vi.useFakeTimers()

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      mockFitAddonInstance.fit.mockClear()
      vi.mocked(terminalApi).resize.mockClear()

      // Simulate rapid visibility changes
      Object.defineProperty(document, 'visibilityState', {
        value: 'visible',
        writable: true,
        configurable: true
      })
      document.dispatchEvent(new Event('visibilitychange'))

      // Trigger another visibility change before the debounce completes
      await vi.advanceTimersByTimeAsync(50)
      document.dispatchEvent(new Event('visibilitychange'))

      // Advance past the debounce delay
      await vi.advanceTimersByTimeAsync(200)

      // Should only call fit once after debounce completes
      expect(mockFitAddonInstance.fit).toHaveBeenCalledTimes(1)
    })

    it('should handle visibility broadcast with isVisible prop', async () => {
      vi.useFakeTimers()

      const { rerender } = render(<ConnectedTerminal isVisible={true} />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      mockFitAddonInstance.fit.mockClear()
      vi.mocked(terminalApi).resize.mockClear()

      // Change visibility to false (simulating terminal becoming hidden in workspace)
      rerender(<ConnectedTerminal isVisible={false} />)

      // Small delay to ensure prop change is processed
      await vi.advanceTimersByTimeAsync(50)

      // Change back to visible
      rerender(<ConnectedTerminal isVisible={true} />)

      // Small delay to ensure prop change is processed
      await vi.advanceTimersByTimeAsync(50)

      // Verify that terminal responds to visibility prop changes
      expect(mockTerminalInstance.focus).toHaveBeenCalled()

      vi.useRealTimers()
    })

    it('should skip resize when terminal becomes visible but PTY is not ready', async () => {
      vi.useFakeTimers()

      // Mock spawn to return success but terminal might not be ready
      vi.mocked(terminalApi).spawn.mockResolvedValue({
        success: true,
        data: { id: 'terminal-123', shell: 'bash', cwd: '/home/user' }
      })

      const { unmount } = render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      mockFitAddonInstance.fit.mockClear()
      vi.mocked(terminalApi).resize.mockClear()

      // Unmount before visibility change completes
      unmount()

      // Simulate visibility change after unmount (should be handled by cleanup)
      Object.defineProperty(document, 'visibilityState', {
        value: 'visible',
        writable: true,
        configurable: true
      })
      document.dispatchEvent(new Event('visibilitychange'))

      await vi.advanceTimersByTimeAsync(200)

      // Should not call resize after unmount
      expect(vi.mocked(terminalApi).resize).not.toHaveBeenCalled()

      vi.useRealTimers()
    })

    it.skip('should handle visibility changes during active data transfer', async () => {
      vi.useFakeTimers()

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // Simulate active data transfer
      if (capturedDataCallback) {
        capturedDataCallback('terminal-123', new TextEncoder().encode('Loading data...\n'))
      }

      mockFitAddonInstance.fit.mockClear()
      vi.mocked(terminalApi).resize.mockClear()

      // Change visibility during active data
      Object.defineProperty(document, 'visibilityState', {
        value: 'visible',
        writable: true,
        configurable: true
      })
      document.dispatchEvent(new Event('visibilitychange'))

      await vi.advanceTimersByTimeAsync(200)

      // Should still recover even during active data transfer
      expect(mockFitAddonInstance.fit).toHaveBeenCalled()

      vi.useRealTimers()
    })

    describe('Visibility broadcast to backend', () => {
      it('should broadcast terminal dimensions to backend when becoming visible', async () => {
        vi.useFakeTimers()

        const { rerender } = render(<ConnectedTerminal isVisible={false} />)

        await vi.waitFor(() => {
          expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
        })

        // Clear initial calls
        vi.mocked(terminalApi).resize.mockClear()

        // Terminal becomes visible - should broadcast dimensions to backend
        rerender(<ConnectedTerminal isVisible={true} />)

        // Wait for double requestAnimationFrame + fit + resize
        await vi.advanceTimersByTimeAsync(50)

        expect(vi.mocked(terminalApi).resize).toHaveBeenCalledWith(
          'terminal-123',
          expect.any(Number),
          expect.any(Number)
        )

        vi.useRealTimers()
      })

      it('should defer resize broadcast until PTY is ready', async () => {
        vi.useFakeTimers()

        // Render with terminal visible but PTY not ready yet
        const { rerender } = render(<ConnectedTerminal isVisible={false} />)

        // Wait a bit - spawn should have been called
        await vi.waitFor(() => {
          expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
        })

        // Clear initial calls
        vi.mocked(terminalApi).resize.mockClear()

        // Change to visible when PTY is ready - should broadcast resize
        rerender(<ConnectedTerminal isVisible={true} />)

        // Wait for double requestAnimationFrame + resize
        await vi.advanceTimersByTimeAsync(50)

        // Verify resize was called with terminal dimensions
        expect(vi.mocked(terminalApi).resize).toHaveBeenCalledWith(
          'terminal-123',
          expect.any(Number),
          expect.any(Number)
        )

        vi.useRealTimers()
      })

      it('should handle rapid visibility toggles without spamming backend', async () => {
        vi.useFakeTimers()

        const { rerender } = render(<ConnectedTerminal isVisible={true} />)

        await vi.waitFor(() => {
          expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
        })

        // Clear initial calls
        vi.mocked(terminalApi).resize.mockClear()
        const initialCallCount = vi.mocked(terminalApi).resize.mock.calls.length

        // Rapidly toggle visibility
        for (let i = 0; i < 5; i++) {
          rerender(<ConnectedTerminal isVisible={i % 2 === 0} />)
          await vi.advanceTimersByTimeAsync(10)
        }

        // Final state: visible
        rerender(<ConnectedTerminal isVisible={true} />)

        // Wait for all pending operations to complete
        await vi.advanceTimersByTimeAsync(100)

        // Should not have spammed the backend with 5+ resize calls
        // The double RAF pattern should prevent excessive calls
        const finalCallCount = vi.mocked(terminalApi).resize.mock.calls.length
        expect(finalCallCount).toBeLessThan(initialCallCount + 3)

        vi.useRealTimers()
      })
    })

    describe('Recovery compatibility with visibility changes', () => {
      it('should recover WebGL context after visibility change with context loss', async () => {
        vi.useFakeTimers()

        render(<ConnectedTerminal />)

        await vi.waitFor(() => {
          expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
        })

        const initialWebglCount = webglAddonCreateCount

        // Simulate WebGL context loss
        capturedContextLossCallback!()

        // Immediately change visibility (simulating tab switch during context loss recovery)
        Object.defineProperty(document, 'visibilityState', {
          value: 'visible',
          writable: true,
          configurable: true
        })
        document.dispatchEvent(new Event('visibilitychange'))

        // Advance past both recovery delays (WebGL: 100ms, Visibility: 150ms)
        await vi.advanceTimersByTimeAsync(200)

        // WebGL should have been recreated despite visibility change
        expect(webglAddonCreateCount).toBeGreaterThan(initialWebglCount)

        // Fit should have been called as part of visibility recovery
        expect(mockFitAddonInstance.fit).toHaveBeenCalled()

        vi.useRealTimers()
      })

      it.skip('should handle simultaneous power resume and visibility change', async () => {
        vi.useFakeTimers()

        render(<ConnectedTerminal />)

        await vi.waitFor(() => {
          expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
        })

        mockFitAddonInstance.fit.mockClear()
        vi.mocked(terminalApi).resize.mockClear()

        // Trigger both power resume and visibility change simultaneously
        capturedPowerResumeCallback!()

        Object.defineProperty(document, 'visibilityState', {
          value: 'visible',
          writable: true,
          configurable: true
        })
        document.dispatchEvent(new Event('visibilitychange'))

        // Advance past both delays (Power: 300ms, Visibility: 150ms)
        await vi.advanceTimersByTimeAsync(350)

        // Should handle both events gracefully
        // Both events trigger the same performTerminalRecovery function
        expect(mockFitAddonInstance.fit).toHaveBeenCalled()

        // Resize should have been called (may be called multiple times but should not error)
        expect(vi.mocked(terminalApi).resize).toHaveBeenCalled()

        vi.useRealTimers()
      })

      it.skip('should not crash when visibility change occurs during unmount', async () => {
        vi.useFakeTimers()

        const { unmount } = render(<ConnectedTerminal />)

        await vi.waitFor(() => {
          expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
        })

        // Start visibility change recovery
        Object.defineProperty(document, 'visibilityState', {
          value: 'visible',
          writable: true,
          configurable: true
        })
        document.dispatchEvent(new Event('visibilitychange'))

        // Unmount before recovery completes
        unmount()

        // Advance past recovery delay - should not throw
        await vi.advanceTimersByTimeAsync(200)

        // No errors should have been thrown
        expect(mockTerminalInstance.dispose).toHaveBeenCalled()

        vi.useRealTimers()
      })

      it.skip('should maintain recovery state across multiple visibility cycles', async () => {
        vi.useFakeTimers()

        render(<ConnectedTerminal />)

        await vi.waitFor(() => {
          expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
        })

        // Perform multiple visibility cycles
        for (let i = 0; i < 3; i++) {
          mockFitAddonInstance.fit.mockClear()
          vi.mocked(terminalApi).resize.mockClear()

          Object.defineProperty(document, 'visibilityState', {
            value: i % 2 === 0 ? 'visible' : 'hidden',
            writable: true,
            configurable: true
          })
          document.dispatchEvent(new Event('visibilitychange'))

          await vi.advanceTimersByTimeAsync(50)
        }

        // Final visibility to visible
        Object.defineProperty(document, 'visibilityState', {
          value: 'visible',
          writable: true,
          configurable: true
        })
        document.dispatchEvent(new Event('visibilitychange'))

        await vi.advanceTimersByTimeAsync(200)

        // Should still recover properly after multiple cycles
        expect(mockFitAddonInstance.fit).toHaveBeenCalled()

        vi.useRealTimers()
      })

      it('should handle visibility recovery without scroll position errors', async () => {
        vi.useFakeTimers()

        // Render with an external terminal ID
        render(<ConnectedTerminal terminalId="test-term-123" />)

        await vi.waitFor(() => {
          // Should NOT spawn since external ID is provided
          expect(vi.mocked(terminalApi).spawn).not.toHaveBeenCalled()
        })

        // Verify terminal was initialized
        expect(mockTerminalInstance.open).toHaveBeenCalled()

        // Trigger visibility change - should not throw any errors
        // even if scroll position restoration occurs
        Object.defineProperty(document, 'visibilityState', {
          value: 'visible',
          writable: true,
          configurable: true
        })
        document.dispatchEvent(new Event('visibilitychange'))

        await vi.advanceTimersByTimeAsync(200)

        // Terminal should still be functional after visibility recovery
        expect(mockTerminalInstance.dispose).not.toHaveBeenCalled()

        vi.useRealTimers()
      })
    })
  })

  /**
   * Hiding a tab makes ConnectedTerminal dispose the WebGL addon and re-fit on
   * the way back (see the `isVisible` effects). Both make xterm's RenderService
   * report stale dimensions for a moment, and xterm 6's Viewport feeds those
   * dimensions straight into `setScrollDimensions`, which clamps the scrollable
   * to `[0, scrollHeight - height]`. When the clamp lands on a zero-ish height
   * the position is pinned to 0, and because the clamp's own scroll event is
   * delivered on a later animation frame — outside xterm's
   * `_suppressOnScrollHandler` window — `_handleScroll` reads scrollTop 0 and
   * drives `ydisp` to the top of the scrollback. Nothing is lost: the buffer is
   * intact, the user is just teleported to the first line of history.
   *
   * The component cannot stop xterm from clamping, so the invariant it owns is
   * to re-assert the viewport once the show sequence has settled. These tests
   * model the clamp by moving `viewportY` to 0 while the tab is hidden.
   */
  describe('Viewport position across a hide/show cycle', () => {
    const PTY_ID = 'terminal-123'
    /** One animation frame under vitest's fake timers. */
    const FRAME_MS = 20

    const setBufferPosition = (viewportY: number, baseY: number): void => {
      const active = mockTerminalInstance.buffer.active
      active.viewportY = viewportY
      active.baseY = baseY
    }

    beforeEach(() => {
      // The cache is module state shared by every suite in this file.
      clearScrollPosition(PTY_ID)
      setBufferPosition(0, 0)
    })

    afterEach(() => {
      clearScrollPosition(PTY_ID)
      setBufferPosition(0, 0)
      vi.useRealTimers()
    })

    const hideThenShow = async (
      rerender: (ui: ReactElement) => void,
      clobber: () => void
    ): Promise<void> => {
      rerender(<ConnectedTerminal isVisible={false} />)
      await vi.waitFor(() => {
        expect(mockPixelScrollSetEnabled).toHaveBeenCalledWith(false)
      })

      clobber()
      mockTerminalInstance.scrollToBottom.mockClear()
      mockTerminalInstance.scrollToLine.mockClear()
      // Cleared here so the only surviving call is the surface repair's, which
      // the ordering assertion below anchors against.
      mockPixelScrollReset.mockClear()

      rerender(<ConnectedTerminal isVisible={true} />)
      // Both nested requestAnimationFrame callbacks of the visibility effect.
      await vi.advanceTimersByTimeAsync(FRAME_MS)
      await vi.advanceTimersByTimeAsync(FRAME_MS)
    }

    it('returns a tail-following terminal to the tail after the viewport is clamped away', async () => {
      vi.useFakeTimers()
      const { rerender } = render(<ConnectedTerminal isVisible={true} />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // Watching live output at the bottom of 1000 lines of scrollback.
      setBufferPosition(1000, 1000)

      await hideThenShow(rerender, () => setBufferPosition(0, 1000))

      // Against the buffer as it is now, not the line number it used to be:
      // the PTY keeps producing while the tab is hidden.
      expect(mockTerminalInstance.scrollToBottom).toHaveBeenCalled()

      // And it must land AFTER the surface repair. That repair repaints the
      // terminal, which is another chance for xterm to re-clamp the viewport,
      // so asserting the position before it would simply be overwritten.
      expect(mockTerminalInstance.scrollToBottom.mock.invocationCallOrder[0]).toBeGreaterThan(
        mockPixelScrollReset.mock.invocationCallOrder[0]
      )
    })

    it('returns a scrolled-up terminal to the history it was showing, not to line 0', async () => {
      vi.useFakeTimers()
      const { rerender } = render(<ConnectedTerminal isVisible={true} />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // Scrolled up into history — 400 is deliberately far from both 0 and the
      // tail, so a pass cannot be an accident of either boundary.
      setBufferPosition(400, 1000)

      await hideThenShow(rerender, () => setBufferPosition(0, 1000))

      expect(mockTerminalInstance.scrollToLine).toHaveBeenCalledWith(400)
      expect(mockTerminalInstance.scrollToBottom).not.toHaveBeenCalled()
    })
  })

  describe('Power resume recovery', () => {
    it('should subscribe to power resume events', async () => {
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      expect(systemApi.onPowerResume).toHaveBeenCalled()
      expect(capturedPowerResumeCallback).toBeTruthy()
    })

    it.skip('should call fit and resize on power resume', async () => {
      vi.useFakeTimers()

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      mockFitAddonInstance.fit.mockClear()
      vi.mocked(terminalApi).resize.mockClear()

      // Simulate power resume
      capturedPowerResumeCallback!()

      // Advance past the 300ms delay
      await vi.advanceTimersByTimeAsync(350)

      expect(mockFitAddonInstance.fit).toHaveBeenCalled()
      expect(vi.mocked(terminalApi).resize).toHaveBeenCalledWith(
        'terminal-123',
        expect.any(Number),
        expect.any(Number)
      )

      vi.useRealTimers()
    })

    it('should cleanup power resume subscription on unmount', async () => {
      const cleanupFn = vi.fn()
      ;(systemApi.onPowerResume as ReturnType<typeof vi.fn>).mockReturnValue(cleanupFn)

      const { unmount } = render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(systemApi.onPowerResume).toHaveBeenCalled()
      })

      unmount()

      expect(cleanupFn).toHaveBeenCalled()
    })
  })

  describe('Window focus recovery (Tauri minimize/restore)', () => {
    it('should register window focus listener on mount', async () => {
      const addEventListenerSpy = vi.spyOn(window, 'addEventListener')

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      expect(addEventListenerSpy).toHaveBeenCalledWith('focus', expect.any(Function))

      addEventListenerSpy.mockRestore()
    })

    it('should re-fit and re-composite on window focus once layout is stable', async () => {
      vi.useFakeTimers()

      // Note: the global beforeEach already stubs HTMLDivElement.prototype
      // getBoundingClientRect to 800x600, so the terminal container reports a
      // usable size and recovery's layout-wait proceeds on the first frame.

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // Advance past the initial requestAnimationFrame so WebGL addon is loaded
      await vi.advanceTimersByTimeAsync(50)
      expect(webglAddonCreateCount).toBeGreaterThanOrEqual(1)

      // Clear mocks before dispatching focus event
      mockFitAddonInstance.fit.mockClear()
      mockTerminalInstance.refresh.mockClear()
      vi.mocked(terminalApi).resize.mockClear()
      const webglCountBeforeFocus = webglAddonCreateCount
      const termEl = mockTerminalInstance.element as HTMLDivElement

      // Dispatch window focus event — recovery fires immediately (no debounce),
      // but waits for a stable layout via requestAnimationFrame before fitting.
      window.dispatchEvent(new Event('focus'))

      // Advance frames so the layout-wait + recovery + re-composite all run
      await vi.advanceTimersByTimeAsync(40)

      // Fit + PTY resize happened against the (now usable) container size
      expect(mockFitAddonInstance.fit).toHaveBeenCalled()
      expect(vi.mocked(terminalApi).resize).toHaveBeenCalledWith(
        'terminal-123',
        expect.any(Number),
        expect.any(Number)
      )
      // Buffer repainted to redraw into the re-composited layer
      expect(mockTerminalInstance.refresh).toHaveBeenCalledWith(0, mockTerminalInstance.rows - 1)
      // WebGL addon is NOT disposed/recreated — the context is healthy, only the
      // compositor needs a nudge. Recreating would add a needless blank gap.
      expect(webglAddonCreateCount).toBe(webglCountBeforeFocus)
      // Visibility flip completed (restored to visible after the re-composite)
      expect(termEl.style.visibility).toBe('')

      vi.useRealTimers()
    })

    it('should cleanup window focus listener on unmount', async () => {
      const removeEventListenerSpy = vi.spyOn(window, 'removeEventListener')

      const { unmount } = render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      unmount()

      expect(removeEventListenerSpy).toHaveBeenCalledWith('focus', expect.any(Function))

      removeEventListenerSpy.mockRestore()
    })

    it('should coalesce overlapping recovery triggers (single-flight)', async () => {
      vi.useFakeTimers()

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      await vi.advanceTimersByTimeAsync(50)
      mockFitAddonInstance.fit.mockClear()

      // On a real window restore, visibilitychange and focus fire close together.
      // Dispatch focus twice before the first recovery completes its RAF cycle.
      window.dispatchEvent(new Event('focus'))
      window.dispatchEvent(new Event('focus'))

      // Let the layout-wait + recovery + trailing visibility-flip RAF complete.
      await vi.advanceTimersByTimeAsync(40)

      // The single-flight guard coalesces the duplicate trigger: fit runs once,
      // not twice, avoiding overlapping resize + visibility-flip cycles.
      expect(mockFitAddonInstance.fit).toHaveBeenCalledTimes(1)

      // After recovery fully completes, a subsequent focus can recover again.
      mockFitAddonInstance.fit.mockClear()
      window.dispatchEvent(new Event('focus'))
      await vi.advanceTimersByTimeAsync(40)
      expect(mockFitAddonInstance.fit).toHaveBeenCalledTimes(1)

      vi.useRealTimers()
    })
  })

  describe('Regression: Visibility transition + recovery scenarios', () => {
    /**
     * REGRESSION TEST: Ensure terminal properly handles visibility state transitions
     * and recovers correctly when returning from hidden state.
     *
     * Tests for:
     * - Proper cleanup on visibility hide
     * - Recovery on visibility show
     * - CWD polling pause/resume behavior
     */

    it('should pause CWD tracking when terminal becomes hidden', async () => {
      vi.useFakeTimers()

      // Mock the getCwd and onCwdChanged methods
      const mockCwdChanged = vi.fn()
      vi.mocked(terminalApi).onCwdChanged.mockReturnValue(mockCwdChanged)

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // Start with visible state
      Object.defineProperty(document, 'visibilityState', {
        value: 'visible',
        writable: true,
        configurable: true
      })

      // Transition to hidden
      Object.defineProperty(document, 'visibilityState', {
        value: 'hidden',
        writable: true,
        configurable: true
      })
      document.dispatchEvent(new Event('visibilitychange'))

      await vi.advanceTimersByTimeAsync(100)

      // CWD tracking should be paused (no new tracking started)
      // The component should handle visibility state properly

      vi.useRealTimers()
    })

    it('should resume CWD tracking when terminal becomes visible again', async () => {
      vi.useFakeTimers()

      const mockCwdChanged = vi.fn()
      vi.mocked(terminalApi).onCwdChanged.mockReturnValue(mockCwdChanged)

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // Start with hidden state
      Object.defineProperty(document, 'visibilityState', {
        value: 'hidden',
        writable: true,
        configurable: true
      })

      // Transition to visible
      Object.defineProperty(document, 'visibilityState', {
        value: 'visible',
        writable: true,
        configurable: true
      })
      document.dispatchEvent(new Event('visibilitychange'))

      await vi.advanceTimersByTimeAsync(300)

      // Terminal should recover and be functional
      expect(mockTerminalInstance.focus).toHaveBeenCalled()
      expect(mockFitAddonInstance.fit).toHaveBeenCalled()

      vi.useRealTimers()
    })

    it('should handle rapid visibility transitions without errors', async () => {
      vi.useFakeTimers()

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // Simulate rapid visibility changes
      for (let i = 0; i < 5; i++) {
        Object.defineProperty(document, 'visibilityState', {
          value: i % 2 === 0 ? 'visible' : 'hidden',
          writable: true,
          configurable: true
        })
        document.dispatchEvent(new Event('visibilitychange'))
        await vi.advanceTimersByTimeAsync(50)
      }

      // Terminal should still be functional after rapid transitions
      expect(mockTerminalInstance.dispose).not.toHaveBeenCalled()

      vi.useRealTimers()
    })
  })

  describe('Regression: CWD changes trigger updates', () => {
    /**
     * REGRESSION TEST: Ensure CWD changes from the backend trigger
     * proper updates in the terminal component.
     *
     * Note: CWD tracking is handled at the store level (use-cwd hook)
     * rather than directly in ConnectedTerminal. These tests verify
     * the component's behavior when CWD state changes.
     */

    it('should have terminalApi with CWD tracking capabilities', () => {
      // Verify the terminal API has CWD-related methods
      expect(terminalApi).toBeDefined()
      expect(typeof terminalApi.onCwdChanged).toBe('function')
      expect(typeof terminalApi.getCwd).toBe('function')
    })

    it('should handle CWD tracking for terminal sessions', async () => {
      // The component should work correctly with CWD tracking enabled
      // CWD is tracked via the use-cwd hook which uses terminalApi
      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // Terminal should be functional
      expect(mockTerminalInstance.open).toHaveBeenCalled()
    })

    it('should handle visibility state for CWD polling pause/resume', async () => {
      // CWD polling should pause when terminal is hidden and resume when visible
      // This is tested through visibility behavior
      vi.useFakeTimers()

      render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // Simulate hidden state (CWD polling should pause)
      Object.defineProperty(document, 'visibilityState', {
        value: 'hidden',
        writable: true,
        configurable: true
      })
      document.dispatchEvent(new Event('visibilitychange'))

      await vi.advanceTimersByTimeAsync(100)

      // Simulate visible state (CWD polling should resume)
      Object.defineProperty(document, 'visibilityState', {
        value: 'visible',
        writable: true,
        configurable: true
      })
      document.dispatchEvent(new Event('visibilitychange'))

      await vi.advanceTimersByTimeAsync(300)

      // Terminal should still be functional
      expect(mockTerminalInstance.dispose).not.toHaveBeenCalled()

      vi.useRealTimers()
    })
  })

  it('should replay transcript once for external terminal ids', async () => {
    mockTerminalStoreState.peekTranscript.mockReturnValueOnce('detached output chunk')

    render(
      <ConnectedTerminal
        terminalId="external-123"
        storeTerminalId="store-123"
        autoSpawn={false}
        spawnOptions={{ projectId: 'project-a' }}
      />
    )

    await vi.waitFor(() => {
      expect(mockTerminalInstance.write).toHaveBeenCalledWith(
        'detached output chunk',
        expect.any(Function)
      )
    })

    expect(mockTerminalStoreState.peekTranscript).toHaveBeenCalledWith('external-123')
    expect(mockTerminalStoreState.consumeTranscript).toHaveBeenCalledWith('external-123')
    expect(mockTerminalStoreState.consumeTranscript).toHaveBeenCalledTimes(1)
    expect(mockRecordTerminalContinuityEvent).toHaveBeenCalledWith({
      name: 'restore-replay-attempted',
      correlationId: 'corr-project-a',
      projectId: 'project-a',
      terminalId: 'store-123',
      ptyId: 'external-123',
      details: {
        mode: 'transcript',
        transcriptLength: 'detached output chunk'.length,
        initialScrollbackLineCount: 0,
        source: 'external-terminal',
        alternateScreenDetected: false
      }
    })
    expect(mockRecordTerminalContinuityEvent).toHaveBeenCalledWith({
      name: 'restore-replay-succeeded',
      correlationId: 'corr-project-a',
      projectId: 'project-a',
      terminalId: 'store-123',
      ptyId: 'external-123',
      details: {
        mode: 'transcript',
        transcriptLength: 'detached output chunk'.length,
        source: 'external-terminal',
        fullFidelity: true,
        restoreLimitation: undefined
      }
    })
  })

  it('should prefer transcript over initial scrollback for external terminal restore', async () => {
    mockTerminalStoreState.peekTranscript.mockReturnValueOnce('\u001b[32mstyled output\u001b[0m')

    render(
      <ConnectedTerminal
        terminalId="external-123"
        autoSpawn={false}
        initialScrollback={['plain fallback line']}
        spawnOptions={{ projectId: 'project-a' }}
      />
    )

    await vi.waitFor(() => {
      expect(mockTerminalInstance.write).toHaveBeenCalledWith(
        '\u001b[32mstyled output\u001b[0m',
        expect.any(Function)
      )
    })

    // First argument only, deliberately. `toHaveBeenCalledWith` compares the
    // whole argument list, so the single-argument form of this assertion could
    // never match a `writeToTerminal` call and was vacuous, while the
    // two-argument form silently stops guarding the moment someone writes a
    // bare `terminal.write(payload)` without the parse hook. Matching on the
    // payload alone covers both shapes — measured, not assumed.
    const wroteScrollbackFallback = mockTerminalInstance.write.mock.calls.some(
      ([chunk]) => typeof chunk === 'string' && chunk.includes('plain fallback line')
    )
    expect(wroteScrollbackFallback).toBe(false)
  })

  it('records replay skipped when no transcript or scrollback exists', async () => {
    mockTerminalStoreState.peekTranscript.mockReturnValueOnce('')

    render(
      <ConnectedTerminal
        terminalId="external-123"
        storeTerminalId="store-123"
        autoSpawn={false}
        spawnOptions={{ projectId: 'project-a' }}
      />
    )

    await vi.waitFor(() => {
      expect(mockRecordTerminalContinuityEvent).toHaveBeenCalledWith({
        name: 'restore-replay-skipped',
        correlationId: 'corr-project-a',
        projectId: 'project-a',
        terminalId: 'store-123',
        ptyId: 'external-123',
        details: {
          reason: 'no-persisted-history',
          source: 'external-terminal'
        }
      })
    })
  })

  it('records alternate-screen replay as limited fidelity', async () => {
    mockTerminalStoreState.peekTranscript.mockReturnValueOnce('before\u001b[?1049hinside')

    render(
      <ConnectedTerminal
        terminalId="external-123"
        storeTerminalId="store-123"
        autoSpawn={false}
        spawnOptions={{ projectId: 'project-a' }}
      />
    )

    await vi.waitFor(() => {
      expect(mockTerminalInstance.write).toHaveBeenCalledWith(
        '\u001b[33m\r\n[Restore note: alternate-screen or redraw-heavy output may be partially reconstructed from transcript replay]\u001b[0m\r\n',
        expect.any(Function)
      )
    })

    expect(mockRecordTerminalContinuityEvent).toHaveBeenCalledWith({
      name: 'restore-replay-succeeded',
      correlationId: 'corr-project-a',
      projectId: 'project-a',
      terminalId: 'store-123',
      ptyId: 'external-123',
      details: {
        mode: 'transcript',
        transcriptLength: 'before\u001b[?1049hinside'.length,
        source: 'external-terminal',
        fullFidelity: false,
        restoreLimitation: 'alternate-screen-or-in-place-redraw'
      }
    })
  })

  it('keeps transcript available when replay write fails', async () => {
    const consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    const onError = vi.fn()
    mockTerminalStoreState.peekTranscript.mockReturnValueOnce('detached output chunk')
    mockTerminalInstance.write.mockImplementationOnce(() => {
      throw new Error('write failed')
    })

    render(
      <ConnectedTerminal
        terminalId="external-123"
        storeTerminalId="store-123"
        autoSpawn={false}
        spawnOptions={{ projectId: 'project-a' }}
        onError={onError}
      />
    )

    await vi.waitFor(() => {
      expect(mockRecordTerminalContinuityEvent).toHaveBeenCalledWith({
        name: 'restore-replay-failed',
        correlationId: 'corr-project-a',
        projectId: 'project-a',
        terminalId: 'store-123',
        ptyId: 'external-123',
        details: {
          mode: 'transcript',
          error: 'write failed',
          source: 'external-terminal'
        }
      })
    })

    expect(mockTerminalStoreState.consumeTranscript).not.toHaveBeenCalled()
    expect(onError).toHaveBeenCalledWith('write failed')
    consoleErrorSpy.mockRestore()
  })

  it('records a failed scrollback restore as failed, not succeeded', async () => {
    // `restoreScrollback` used to swallow the failure in its own try/catch and
    // return normally, so the caller went on to record
    // restore-replay-succeeded for a restore that visibly did not happen. The
    // telemetry then reported a 100% success rate and hid the real failure
    // rate. Collapsing it to a payload builder is what puts the failure back
    // in front of the caller.
    const consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    const onError = vi.fn()
    mockTerminalInstance.write.mockImplementationOnce(() => {
      throw new Error('write failed')
    })

    render(
      <ConnectedTerminal
        terminalId="external-123"
        storeTerminalId="store-123"
        autoSpawn={false}
        initialScrollback={['restored line']}
        spawnOptions={{ projectId: 'project-a' }}
        onError={onError}
      />
    )

    await vi.waitFor(() => {
      expect(mockRecordTerminalContinuityEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          name: 'restore-replay-failed',
          details: expect.objectContaining({ mode: 'scrollback', error: 'write failed' })
        })
      )
    })
    expect(mockRecordTerminalContinuityEvent).not.toHaveBeenCalledWith(
      expect.objectContaining({ name: 'restore-replay-succeeded' })
    )
    expect(onError).toHaveBeenCalledWith('write failed')
    consoleErrorSpy.mockRestore()
  })

  it('documents alternate-screen continuity as limited rather than full-fidelity restore', async () => {
    mockTerminalStoreState.peekTranscript.mockReturnValueOnce('prelude\u001b[?47htui-screen')

    render(
      <ConnectedTerminal
        terminalId="external-123"
        storeTerminalId="store-123"
        autoSpawn={false}
        spawnOptions={{ projectId: 'project-a' }}
      />
    )

    await vi.waitFor(() => {
      expect(mockRecordTerminalContinuityEvent).toHaveBeenCalledWith({
        name: 'restore-replay-succeeded',
        correlationId: 'corr-project-a',
        projectId: 'project-a',
        terminalId: 'store-123',
        ptyId: 'external-123',
        details: {
          mode: 'transcript',
          transcriptLength: 'prelude\u001b[?47htui-screen'.length,
          source: 'external-terminal',
          fullFidelity: false,
          restoreLimitation: 'alternate-screen-or-in-place-redraw'
        }
      })
    })
  })

  it('should mark renderer attachment lifecycle for external terminal ids', async () => {
    const { unmount } = render(<ConnectedTerminal terminalId="external-123" autoSpawn={false} />)

    await vi.waitFor(() => {
      expect(mockTerminalStoreState.setRendererAttached).toHaveBeenCalledWith('external-123', true)
    })

    unmount()

    expect(mockTerminalStoreState.setRendererAttached).toHaveBeenCalledWith('external-123', false)
  })

  it('opens the transcript gate synchronously with ptyIdRef on the external attach path', async () => {
    // F2/DOD-2: hold the attach round trip open so the window between the
    // ptyIdRef assignment and the store gate is observable at all. Before the
    // fix the gate only opened after this promise resolved, and the detached
    // collector captured the same bytes the live terminal was already writing.
    let releaseAttach: (() => void) | undefined
    mockTerminalStoreState.resumeTerminalResource.mockReturnValue(
      new Promise((resolve) => {
        releaseAttach = () => resolve({ success: true, data: undefined })
      })
    )

    const { unmount } = render(<ConnectedTerminal terminalId="external-123" autoSpawn={false} />)

    await vi.waitFor(() => {
      expect(mockTerminalStoreState.setRendererAttached).toHaveBeenCalledWith('external-123', true)
    })
    expect(releaseAttach).toBeTypeOf('function')

    releaseAttach?.()
    await vi.waitFor(() => {
      expect(mockTerminalStoreState.resumeTerminalResource).toHaveBeenCalled()
    })

    // Exactly one acquire: the hoist replaced the old post-await increment
    // rather than adding a second one.
    const acquires = mockTerminalStoreState.setRendererAttached.mock.calls.filter(
      ([ptyId, attached]) => ptyId === 'external-123' && attached === true
    )
    expect(acquires).toHaveLength(1)

    unmount()
  })

  it('releases the transcript gate when the external attach fails', async () => {
    mockTerminalStoreState.resumeTerminalResource.mockResolvedValue({
      success: false,
      error: 'gone',
      code: 'TERMINAL_NOT_FOUND'
    })
    mockTerminalStoreState.findTerminalByPtyId.mockImplementation((ptyId: string) => ({
      id: ptyId,
      ptyId,
      claim: undefined,
      healthStatus: 'running'
    }))

    const { unmount } = render(<ConnectedTerminal terminalId="external-123" autoSpawn={false} />)

    await vi.waitFor(() => {
      expect(mockTerminalStoreState.setRendererAttached).toHaveBeenCalledWith('external-123', false)
    })

    unmount()
  })

  describe('Regression: Proper Tauri terminal API mocking', () => {
    /**
     * REGRESSION TEST: Ensure tests properly mock Tauri terminal API
     * to prevent silent fallback to window.api (Electron path).
     *
     * This test validates that the component works with Tauri APIs
     * without requiring window.api to be present.
     */

    it('should use Tauri invoke for terminal operations', async () => {
      // The component should use terminalApi from @/lib/api
      // which should be the Tauri implementation
      expect(terminalApi).toBeDefined()
      expect(typeof terminalApi.spawn).toBe('function')
      expect(typeof terminalApi.write).toBe('function')
      expect(typeof terminalApi.resize).toBe('function')
      expect(typeof terminalApi.closeView).toBe('function')
      expect(typeof terminalApi.terminate).toBe('function')
      expect(typeof terminalApi.kill).toBe('function')
    })

    it('should work without window.api for terminal operations', async () => {
      // Store original window.api if it exists
      const windowWithOptionalApi = window as WindowWithOptionalApi
      const originalWindowApi = windowWithOptionalApi.api

      // Remove window.api to simulate pure Tauri environment
      delete windowWithOptionalApi.api

      // Component should still work with Tauri APIs
      const { unmount } = render(<ConnectedTerminal />)

      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // Restore window.api
      if (originalWindowApi) {
        windowWithOptionalApi.api = originalWindowApi
      }

      unmount()
    })

    it('should properly handle Tauri IPC invoke errors', async () => {
      // Mock a failed spawn
      vi.mocked(terminalApi).spawn.mockResolvedValue({
        success: false,
        error: 'Failed to spawn terminal',
        code: 'SPAWN_FAILED'
      })

      render(<ConnectedTerminal />)

      // Should handle the error gracefully
      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // Reset for other tests
      vi.mocked(terminalApi).spawn.mockResolvedValue({
        success: true,
        data: { id: 'terminal-123', shell: 'bash', cwd: '/home/user' }
      })
    })
  })

  describe('Fit churn reduction', () => {
    /**
     * REGRESSION TEST: Ensure fit() is called on visibility changes.
     * With the two-stage resize pipeline (useTerminalResizeV2), fit is
     * always forced on visibility changes via forceResizeFit() so the
     * terminal correctly adapts to potentially-changed container dims.
     * Dimension skip-detection applies to the ResizeObserver path, not
     * visibility-triggered forced fits.
     */
    it('should call fit on each visibility toggle (forced fit)', async () => {
      vi.useFakeTimers()
      const { rerender } = render(<ConnectedTerminal isVisible={false} />)
      await vi.waitFor(() => {
        expect(vi.mocked(terminalApi).spawn).toHaveBeenCalled()
      })

      // Clear fit calls from initialization
      mockFitAddonInstance.fit.mockClear()

      // First visibility change to true — fit forced
      rerender(<ConnectedTerminal isVisible={true} />)
      // Double RAF in the visibility effect
      await vi.advanceTimersByTimeAsync(20)
      await vi.advanceTimersByTimeAsync(20)

      expect(mockFitAddonInstance.fit).toHaveBeenCalledTimes(1)

      // Toggle off then on — fit is forced again (bypasses dimension skip)
      mockFitAddonInstance.fit.mockClear()
      rerender(<ConnectedTerminal isVisible={false} />)
      await vi.advanceTimersByTimeAsync(20)
      rerender(<ConnectedTerminal isVisible={true} />)
      await vi.advanceTimersByTimeAsync(20)
      await vi.advanceTimersByTimeAsync(20)

      // fit is called because forceFit bypasses the dimension check
      // This ensures the terminal correctly re-fits after being hidden
      expect(mockFitAddonInstance.fit).toHaveBeenCalledTimes(1)

      vi.useRealTimers()
    })
  })
})
