import { FitAddon } from '@xterm/addon-fit'
import { SearchAddon } from '@xterm/addon-search'
import { WebglAddon } from '@xterm/addon-webgl'
import type { IDisposable } from '@xterm/xterm'
import { Terminal } from '@xterm/xterm'
import { AlertTriangle, RefreshCcw } from 'lucide-react'
import { memo, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import '@xterm/xterm/css/xterm.css'
import type { PrimaryTerminalDataHandle, TerminalReplayCoverage } from '@shared/types/ipc.types'
import { useShallow } from 'zustand/shallow'
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuTrigger
} from '@/components/ui/context-menu'
import { useCompanionTerminalGeometry } from '@/hooks/use-companion-terminal-geometry'
import { useCompanionTerminalTextScale } from '@/hooks/use-companion-terminal-text-scale'
import { useMobileWebShell } from '@/hooks/use-mobile-web-shell'
import { useTerminalClipboard } from '@/hooks/use-terminal-clipboard'
import { useTerminalColorTheme } from '@/hooks/use-terminal-color-theme'
import { useTerminalResizeV2 } from '@/hooks/use-terminal-resize-v2'
import { isTerminalPendingPtyAssignment } from '@/hooks/use-terminal-restore'
import { systemApi, terminalApi } from '@/lib/api'
import { openTerminalUrl } from '@/lib/browser/terminal-url-navigation'
import { buildTerminalPathLinks, openFilePathFromTerminal } from '@/lib/file-path-links'
import { logFrontendError } from '@/lib/log-api'
import { isMac, isPlatformModifier } from '@/lib/platform'
import { addRendererRef, registerPrimaryTerminalData, removeRendererRef } from '@/lib/terminal-api'
import {
  getOrCreateProjectContinuityCorrelation,
  recordTerminalContinuityEvent
} from '@/lib/terminal-continuity-instrumentation'
import { buildTerminalUrlLinks, isSupportedTerminalUrl } from '@/lib/terminal-url-links'
import { applyThemeToTerminal, getActiveTerminalTheme } from '@/lib/themes'
import {
  useTerminalBufferSize,
  useTerminalFontFamily,
  useTerminalFontSize,
  useTerminalRenderer,
  useTerminalScreenReaderMode,
  useTerminalSymbolFontFamily
} from '@/stores/app-settings-store'
import { matchesShortcut, useKeyboardShortcutsStore } from '@/stores/keyboard-shortcuts-store'
import { useActiveProject, useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import type { TerminalModes, TerminalSpawnOptions } from '../../../shared/types/ipc.types'
import {
  buildRehydrateSequences,
  buildScrollbackRestorePayload,
  captureScrollPosition,
  registerTerminal,
  restoreScrollPosition,
  unregisterTerminal
} from '../../utils/terminal-registry'
import { cacheTerminal, takeCachedTerminal } from './terminal-cache'
import { applyTerminalRenderOptions, getTerminalOptions } from './terminal-config'
import { attachPixelSmoothScroll, type PixelSmoothScrollHandle } from './terminal-pixel-scroll'
import { ensureTerminalUnicode11 } from './terminal-unicode'
import {
  clearWebglRenderModel,
  createWebglScrollRepair,
  restoreVisibleTerminalSurface,
  type WebglScrollRepair
} from './terminal-webgl-repair'

// Common readline/shell Ctrl sequences that should always pass through to the
// PTY regardless of platform. On macOS these are already protected by the
// isMac guard, but on Windows/Linux they would otherwise be swallowed when a
// matching app shortcut exists (e.g. commandPalette=ctrl+k, commandHistory=ctrl+r).
const READLINE_PASSTHROUGH_KEYS = new Set([
  'a', // Ctrl+A  move to beginning of line
  'e', // Ctrl+E  move to end of line
  'k', // Ctrl+K  kill to end of line
  'r', // Ctrl+R  reverse-i-search
  'f', // Ctrl+F  move forward one char
  'b', // Ctrl+B  move back one char
  'w', // Ctrl+W  delete previous word
  'u', // Ctrl+U  delete to beginning of line
  'p', // Ctrl+P  previous history entry
  'n', // Ctrl+N  next history entry
  'l', // Ctrl+L  clear screen
  'd' // Ctrl+D  EOF / delete char
])

function isReadlinePassthrough(event: KeyboardEvent): boolean {
  return (
    event.ctrlKey &&
    !event.metaKey &&
    !event.shiftKey &&
    !event.altKey &&
    READLINE_PASSTHROUGH_KEYS.has(event.key.toLowerCase())
  )
}

function isAppOwnedTerminalShortcut(
  event: KeyboardEvent,
  shortcuts: ReturnType<typeof useKeyboardShortcutsStore.getState>['shortcuts']
): boolean {
  // 1. App shortcuts take priority over readline passthrough.
  // This ensures commandPalette, commandHistory, etc. work from terminal
  // focus even though their Ctrl+key also matches a readline binding.
  for (const shortcut of Object.values(shortcuts)) {
    const activeKey = shortcut.customKey ?? shortcut.defaultKey
    if (matchesShortcut(event, activeKey)) {
      return true
    }
  }

  // 2. No app shortcut matched — check readline passthrough.
  // Ctrl+letter readline bindings must reach the PTY on every platform.
  // On macOS the isMac guard in matchesShortcut already prevents Ctrl+key
  // from matching app shortcuts, so the readline behavior is preserved.
  if (isReadlinePassthrough(event)) {
    return false
  }

  return false
}

/**
 * Legacy keycodes xterm refuses to deliver a printable character for.
 *
 * xterm has exactly three paths that can send a typed character, and every one
 * of them gates on a legacy keycode (verified against the bundled
 * `@xterm/xterm` 6.1 build, not from memory):
 *
 * - `_keyDown` keymap catch-all: `e.keyCode >= 48 && e.key.length === 1`
 * - `_keyPress`: `if (0 === e.which || 0 === e.charCode) return false`
 * - `_inputEvent`: `(!e.composed || !this._keyDownSeen)` — and `_keyDownSeen`
 *   is set at the top of `_keyDown`, so it is false only between a keyup and
 *   the next keydown.
 *
 * Two shapes fall through all three:
 *
 * `0` — a key synthesised without a virtual keycode. macOS remote desktop
 * clients (ToDesk, UU远程) inject text with
 * `CGEventKeyboardSetUnicodeString`, which leaves keyCode/charCode/which at 0.
 * The keymap's `case 0` only recognises the four iOS `UIKeyInput*Arrow`
 * values, so an ordinary character is dropped. Typing a line delivers a single
 * character — the one where `keyup` happened to reset `_keyDownSeen` before
 * the `input` event landed, letting `_inputEvent` through.
 *
 * `229` — "an IME is processing this key". Windows `KEYEVENTF_UNICODE` /
 * VK_PACKET injection and some IMEs report it for every key even in latin
 * mode; xterm then routes through CompositionHelper, whose textarea-diff
 * fallback drops every keystroke arriving while its single `setTimeout(0)` is
 * in flight (xtermjs/xterm.js#5887, #6078).
 *
 * Both shapes are unambiguous: a genuine IME keydown reports `key === 'Process'`
 * and a dead key `key === 'Dead'` — never a single character — and any keydown
 * inside a live composition sets `isComposing`. Modifier combos are excluded so
 * app shortcuts and the shell's ctrl bindings keep the normal path. Real keys
 * always carry a nonzero keyCode, so this never competes with ordinary typing.
 *
 * The empty-textarea condition is the "not inserted yet" proof. xterm cancels
 * every printable keydown it handles itself, so the textarea is empty in
 * steady state; a non-empty one means the browser already inserted this
 * character (the input-before-keydown ordering reported in #5887) and
 * `_inputEvent` may already have sent it. Bail there rather than risk the
 * duplicate-PTY-input class of bug (GH-267).
 */
const KEYCODES_XTERM_CANNOT_DELIVER = new Set([0, 229])

function isKeyXtermWillDrop(
  event: KeyboardEvent,
  textarea: HTMLTextAreaElement | undefined
): boolean {
  return (
    KEYCODES_XTERM_CANNOT_DELIVER.has(event.keyCode) &&
    !event.isComposing &&
    event.key.length === 1 &&
    !event.ctrlKey &&
    !event.metaKey &&
    !event.altKey &&
    !textarea?.value
  )
}

/**
 * Every multi-character `KeyboardEvent.key` value that names a real key rather
 * than carrying text. No physical key produces a multi-character text value,
 * so anything outside this set with `key.length > 1` is injected text.
 */
const NAMED_KEY_VALUES = new Set([
  'Alt',
  'AltGraph',
  'ArrowDown',
  'ArrowLeft',
  'ArrowRight',
  'ArrowUp',
  'Backspace',
  'CapsLock',
  'Clear',
  'ContextMenu',
  'Control',
  'Copy',
  'Cut',
  'Dead',
  'Delete',
  'End',
  'Enter',
  'Escape',
  'Fn',
  'FnLock',
  'Help',
  'Home',
  'Insert',
  'Meta',
  'NumLock',
  'PageDown',
  'PageUp',
  'Paste',
  'Pause',
  'PrintScreen',
  'Process',
  'ScrollLock',
  'Shift',
  'Super',
  'Symbol',
  'Tab',
  'Undo',
  'Unidentified',
  // The four iOS values xterm's own `case 0` recognises; they are key names,
  // not text, despite not appearing in the DOM named-key list.
  'UIKeyInputUpArrow',
  'UIKeyInputDownArrow',
  'UIKeyInputLeftArrow',
  'UIKeyInputRightArrow'
])

function isNamedKeyValue(key: string): boolean {
  return NAMED_KEY_VALUES.has(key) || /^F\d{1,2}$/.test(key)
}

/**
 * A whole typed chunk delivered as one synthetic key event.
 *
 * macOS remote desktop clients (ToDesk, UU远程) do not replay keystrokes one
 * by one — they inject the accumulated text as a single event whose `key` is
 * the entire string, with a placeholder `code`/`keyCode` borrowed from some
 * unrelated key. Captured from a live ToDesk session typing 你好:
 *
 *     keydown   key="你好" code="KeyA" keyCode=65    charCode=0
 *     keypress  key="你好" code="KeyA" keyCode=20320 charCode=20320
 *     input     data="你好" inputType="insertText"
 *
 * All three xterm delivery paths mishandle that shape, and they defeat each
 * other (verified against the bundled `@xterm/xterm` 6.1 build):
 *
 * - `_keyDown`'s printable catch-all requires `1 === e.key.length`, so a chunk
 *   never matches and `_keyDownHandled` stays false.
 * - `_keyPress` then runs and sends `String.fromCharCode(e.charCode)` — the
 *   FIRST character only (20320 is 你) — and sets `_keyPressHandled`.
 * - `_inputEvent` opens with `if (this._keyPressHandled) return false`, so the
 *   one event that carries the full string is discarded.
 *
 * Net effect: a typed line arrives as its first character. Delivering the chunk
 * here and cancelling the event stops `_keyPress` and the browser's own text
 * insertion, so the string is written exactly once.
 */
function isInjectedTextChunk(event: KeyboardEvent): boolean {
  return (
    event.key.length > 1 &&
    !isNamedKeyValue(event.key) &&
    !event.isComposing &&
    !event.ctrlKey &&
    !event.metaKey &&
    !event.altKey
  )
}

/** Prevent browser reverse-tab focus traversal; xterm still handles Tab / Shift+Tab. */
function trapTerminalTabFocusNavigation(event: KeyboardEvent): boolean {
  if (event.key !== 'Tab') {
    return false
  }
  event.preventDefault()
  return true
}

const MAX_WEBGL_RECOVERY_ATTEMPTS = 3
const WEBGL_CONTEXT_LOSS_RECOVERY_DELAY_MS = 100

// Prompt themes (starship/powerlevel10k) rely on Nerd Font glyphs and CJK
// text; the configured mono font rarely ships them, so a symbol font (user
// selectable in App Preferences) plus a CJK tail is appended after the main
// family and before the trailing generic family.
const TERMINAL_CJK_TAIL = '"PingFang SC", "Microsoft YaHei", monospace'
const TERMINAL_SYMBOL_FONTS_AUTO =
  '"MesloLGLDZ Nerd Font Mono", "MesloLGLDZ Nerd Font", "MesloLGS NF", "MesloLGL NF", "MesloLGM NF", "JetBrainsMono Nerd Font", "JetBrainsMono Nerd Font Mono", "FiraCode Nerd Font", "Hack Nerd Font", "Symbols Nerd Font"'

function buildTerminalFontChain(mainFamily: string, symbolFont: string): string {
  const trimmed = mainFamily.trim()
  const genericMatch = /,\s*(monospace|sans-serif|serif|cursive|fantasy)\s*$/i.exec(trimmed)
  const head = trimmed ? (genericMatch ? trimmed.slice(0, genericMatch.index).trim() : trimmed) : ''
  const symbol = symbolFont === 'none' ? '' : symbolFont || TERMINAL_SYMBOL_FONTS_AUTO
  return [head, symbol, TERMINAL_CJK_TAIL].filter(Boolean).join(', ')
}
const VISIBILITY_RECOVERY_DELAY_MS = 150
const POWER_RESUME_RECOVERY_DELAY_MS = 300
const ACTIVITY_DEBOUNCE_MS = 1000
const CLIPBOARD_RATE_LIMIT_MS = 100

/**
 * Ceiling on the live bytes held back while a replay is still in flight.
 *
 * The hold exists to keep the detached interval ahead of the bytes that
 * followed it, and the window it covers is one attach round trip — single-digit
 * milliseconds. 2 MiB is far more than any PTY produces in that time, so
 * reaching it means the window is no longer the one this was designed for.
 * At that point bounded memory wins over ordering: the hold drains in order and
 * the rest of the window writes straight through. Nothing is dropped; the
 * transcript can land after live output, which the replay telemetry records as
 * `replayHoldOverflowed`.
 */
const MAX_REPLAY_HOLD_BYTES = 2 * 1024 * 1024

// Platform-aware shortcut modifier for the terminal context-menu labels
// (⌘ on macOS, Ctrl elsewhere). Mirrors GlobalContextMenu's SHORTCUT_MOD.
const SHORTCUT_MOD = isMac ? '⌘' : 'Ctrl'

const shouldUseWebglRenderer = (rendererPreference: 'auto' | 'webgl' | 'dom'): boolean =>
  rendererPreference !== 'dom'

export interface TerminalSearchHandle {
  findNext: (term: string) => boolean
  findPrevious: (term: string) => boolean
  clearDecorations: () => void
  writeText: (text: string) => void
}

export interface ConnectedTerminalProps {
  terminalId?: string
  storeTerminalId?: string
  spawnOptions?: TerminalSpawnOptions
  onSpawned?: (terminalId: string) => void
  autoSpawn?: boolean
  onBoundToStoreTerminal?: (ptyId: string) => void
  onExit?: (exitCode: number, signal?: number) => void
  onError?: (error: string) => void
  onCommand?: (command: string) => void
  className?: string
  autoFocus?: boolean
  initialScrollback?: string[]
  /**
   * R3: captured DEC private-mode snapshot to replay before `initialScrollback`
   * on terminal mount, so an alt-screen TUI (vim/tmux/less) restores its
   * screen/modes. Optional — absence degrades to content-only restore.
   */
  initialModes?: TerminalModes | null
  searchRef?: React.Ref<TerminalSearchHandle>
  isVisible?: boolean
}

function getInstrumentationProjectId(spawnOptions?: TerminalSpawnOptions): string | undefined {
  const candidate = spawnOptions?.projectId
  return typeof candidate === 'string' ? candidate : undefined
}

interface ResumedRendererAttachment {
  attached: boolean
  stale: boolean
  /**
   * What the host replayed during this attach, or `null` when it replayed
   * nothing. The caller uses it to decide whether its own detached-output
   * transcript is redundant — both cover the same detached interval, so
   * writing both duplicates whole blocks of output.
   */
  serverReplay: TerminalReplayCoverage | null
}

/**
 * Complete the host-authorized resume handshake for an already-running PTY.
 *
 * The caller owns the renderer-side store gate (`claimRendererGate` /
 * `releaseRendererGate`): this is a module-level function and cannot see the
 * component refs that make that gate idempotent, and the gate has to open
 * synchronously with the `ptyIdRef` assignment rather than after these awaits.
 */
async function attachResumedTerminalRenderer(
  terminalId: string,
  storeTerminalId: string | undefined,
  rendererId: string
): Promise<ResumedRendererAttachment> {
  const store = useTerminalStore.getState()
  const record =
    (storeTerminalId
      ? store.terminals.find((terminal) => terminal.id === storeTerminalId)
      : undefined) ?? store.findTerminalByPtyId(terminalId)
  if (!record) {
    void logFrontendError({
      level: 'warn',
      source: 'connected-terminal.resume',
      message: 'code=TERMINAL_NOT_FOUND'
    })
    return { attached: false, stale: true, serverReplay: null }
  }

  if (!record.claim && terminalApi.watch) {
    const watched = await terminalApi.watch(terminalId, record.resumeCursor ?? 0)
    if (watched.success) {
      const rendererRef = await addRendererRef(terminalId, rendererId)
      if (!rendererRef.success) {
        void logFrontendError({
          level: 'warn',
          source: 'connected-terminal.watch',
          message: `code=${rendererRef.code} terminalRecordId=${record.id}`
        })
      }
      // Close the cursor loop the resume path already has (terminal-store.ts
      // hydrateTerminalResource). Without this the next watch presents the same
      // stale cursor and the host replays a backlog it already delivered.
      const nextStore = useTerminalStore.getState()
      nextStore.setTerminalHealthStatus(record.id, 'running')
      nextStore.setTerminalResumeCursor(record.id, watched.data.latestSeq)
      return {
        attached: true,
        stale: false,
        serverReplay: { latestSeq: watched.data.latestSeq, gap: watched.data.gap }
      }
    }
    void logFrontendError({
      level: 'warn',
      source: 'connected-terminal.watch',
      message: `code=${watched.code} terminalRecordId=${record.id}`
    })
  }

  const resumed = await store.resumeTerminalResource(record.id)
  if (!resumed.success) {
    void logFrontendError({
      level: 'warn',
      source: 'connected-terminal.resume',
      message: `code=${resumed.code} terminalRecordId=${record.id}`
    })
    // Conversation-only records (no project) that the host no longer knows
    // about can be dropped. Project terminals still receive an ephemeral
    // conversationId from spawn — closing those blanks the tab.
    if (resumed.code === 'TERMINAL_NOT_FOUND' || resumed.code === 'UNAUTHORIZED') {
      if (record.conversationId && !record.projectId) {
        useTerminalStore.getState().closeTerminal(record.id, record.projectId ?? '')
        return { attached: false, stale: true, serverReplay: null }
      }
      useTerminalStore.getState().setTerminalHealthStatus(record.id, 'disconnected')
      void logFrontendError({
        level: 'warn',
        source: 'connected-terminal.resume',
        message: `kept project terminal disconnected code=${resumed.code} terminalRecordId=${record.id}`
      })
      return { attached: false, stale: false, serverReplay: null }
    }
    return { attached: false, stale: false, serverReplay: null }
  }

  const reconciledStore = useTerminalStore.getState()
  const reconciled =
    reconciledStore.terminals.find((terminal) => terminal.id === record.id) ??
    reconciledStore.findTerminalByPtyId(terminalId)
  if (
    !reconciled?.claim ||
    reconciled.ptyId !== terminalId ||
    reconciled.healthStatus === 'disconnected'
  ) {
    useTerminalStore.getState().setTerminalHealthStatus(record.id, 'disconnected')
    void logFrontendError({
      level: 'warn',
      source: 'connected-terminal.resume',
      message: `code=UNAUTHORIZED terminalRecordId=${record.id}`
    })
    return { attached: false, stale: false, serverReplay: null }
  }

  const rendererRef = await addRendererRef(terminalId, rendererId)
  if (!rendererRef.success) {
    const nextStore = useTerminalStore.getState()
    nextStore.setTerminalClaim(terminalId, undefined)
    nextStore.setTerminalHealthStatus(record.id, 'disconnected')
    void logFrontendError({
      level: 'warn',
      source: 'connected-terminal.renderer-ref',
      message: `code=${rendererRef.code} terminalRecordId=${record.id}`
    })
    return { attached: false, stale: false, serverReplay: null }
  }

  // `?? null` on purpose: a transport that reports success without coverage
  // must degrade to "no host replay", never to an undefined that the caller
  // would read `.gap` off of.
  return { attached: true, stale: false, serverReplay: resumed.data ?? null }
}

function ConnectedTerminalComponent({
  terminalId: externalTerminalId,
  storeTerminalId,
  spawnOptions,
  onSpawned,
  autoSpawn = true,
  onExit,
  onError,
  onCommand,
  onBoundToStoreTerminal,
  className = '',
  autoFocus = true,
  initialScrollback,
  initialModes,
  searchRef,
  isVisible = true
}: ConnectedTerminalProps): React.JSX.Element {
  const { t } = useTranslation('terminal')
  const tRef = useRef(t)
  tRef.current = t

  // 1. STABLE ID DERIVATION
  const targetId = storeTerminalId || externalTerminalId
  const [cleanupRecoveryId, setCleanupRecoveryId] = useState<string | null>(null)

  // 2. STORE HOOKS (Must be at the top)
  const {
    healthStatus,
    cleanupRecovery,
    recordTerminalCleanupFailure,
    retryTerminalCleanup,
    restartTerminalResource
  } = useTerminalStore(
    useShallow((state) => {
      const term = state.terminals.find((t) => t.id === targetId)
      const recoveryId = cleanupRecoveryId ?? term?.ptyId ?? externalTerminalId ?? targetId
      return {
        healthStatus: term?.healthStatus || 'running',
        cleanupRecovery: recoveryId ? state.cleanupRecoveries[recoveryId] : undefined,
        recordTerminalCleanupFailure: state.recordTerminalCleanupFailure,
        retryTerminalCleanup: state.retryTerminalCleanup,
        restartTerminalResource: state.restartTerminalResource
      }
    })
  )

  const fontFamily = buildTerminalFontChain(useTerminalFontFamily(), useTerminalSymbolFontFamily())
  const isMobileWebShell = useMobileWebShell()
  const companionGeometry = useCompanionTerminalGeometry()
  const companionTextScale = useCompanionTerminalTextScale()
  const [parkedByPhone, setParkedByPhone] = useState(false)
  const fontSize = Math.max(
    6,
    Math.round(useTerminalFontSize() * (isMobileWebShell ? companionTextScale.scale : 1))
  )
  const bufferSize = useTerminalBufferSize()
  const rendererPreference = useTerminalRenderer()
  const screenReaderMode = useTerminalScreenReaderMode()
  const activeProject = useActiveProject()
  const shortcuts = useKeyboardShortcutsStore((state) => state.shortcuts)

  // 3. REFS
  const instanceIdRef = useRef<string>(`conn-${Math.random().toString(36).slice(2, 9)}`)
  const instanceId = instanceIdRef.current
  const containerRef = useRef<HTMLDivElement>(null)
  const terminalRef = useRef<Terminal | null>(null)
  const fitAddonRef = useRef<FitAddon | null>(null)
  const searchAddonRef = useRef<SearchAddon | null>(null)
  const webglAddonRef = useRef<WebglAddon | null>(null)
  const fileLinkProviderDisposableRef = useRef<IDisposable | null>(null)
  const webglRecoveryAttemptsRef = useRef<number>(0)
  const webglRecoveryTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const loadWebglAddonRef = useRef<((term: Terminal, isRecovery?: boolean) => void) | null>(null)
  const webglContextLostRef = useRef<boolean>(false)
  const webglScrollRepairRef = useRef<WebglScrollRepair | null>(null)
  const pixelScrollRef = useRef<PixelSmoothScrollHandle | null>(null)
  const needsSurfaceRestoreRef = useRef(!isVisible)
  // Single-flight guard for performTerminalRecovery. On a window restore both
  // the visibilitychange and focus handlers (and sometimes power-resume) can
  // fire close together; without this guard each would start its own
  // layout-wait RAF loop and overlapping fit + visibility-flip cycles.
  const recoveryInProgressRef = useRef<boolean>(false)
  // Track visibility prop for recovery path guards (tab-active, not window-visible).
  // Ref avoids stale closures in event listeners referencing isVisible directly.
  const isVisibleRef = useRef(isVisible)
  isVisibleRef.current = isVisible
  // Read by the hide branch of the visibility effect. A plain closure over the
  // prop would force it into that effect's dependency list, and a changing id
  // would then tear down and reload the WebGL addon for no reason.
  const externalTerminalIdRef = useRef(externalTerminalId)
  externalTerminalIdRef.current = externalTerminalId
  const rendererPreferenceRef = useRef(rendererPreference)
  rendererPreferenceRef.current = rendererPreference
  const activeProjectPathRef = useRef<string | undefined>(activeProject?.path)
  activeProjectPathRef.current = activeProject?.path
  const shortcutsRef = useRef(shortcuts)
  shortcutsRef.current = shortcuts
  const cleanupDataListenerRef = useRef<(() => void) | null>(null)
  const primaryDataHandleRef = useRef<PrimaryTerminalDataHandle | null>(null)
  const cleanupExitListenerRef = useRef<(() => void) | null>(null)
  const rendererRefAttachedRef = useRef(false)
  // Distinct from rendererRefAttachedRef on purpose. This ref tracks the
  // RENDERER-SIDE store refcount (rendererAttachmentCount, which gates
  // transcript capture in use-terminal-detached-output.ts); rendererRefAttachedRef
  // tracks the HOST-SIDE renderer reference (addRendererRef/removeRendererRef,
  // which gates PTY lifetime). They are acquired at different moments by design
  // and must not be merged.
  const rendererGateHeldRef = useRef(false)
  const ptyIdRef = useRef<string | null>(null)
  const spawnInFlightRef = useRef(false)
  const didInitRef = useRef(false)
  const initializedTerminalIdRef = useRef<string | undefined>(undefined)
  const onExitRef = useRef(onExit)
  onExitRef.current = onExit
  const onErrorRef = useRef(onError)
  onErrorRef.current = onError
  const onSpawnedRef = useRef(onSpawned)
  onSpawnedRef.current = onSpawned
  const onCommandRef = useRef(onCommand)
  onCommandRef.current = onCommand
  const onBoundToStoreTerminalRef = useRef(onBoundToStoreTerminal)
  onBoundToStoreTerminalRef.current = onBoundToStoreTerminal
  const spawnOptionsRef = useRef(spawnOptions)
  spawnOptionsRef.current = spawnOptions
  // R3: keep the latest captured modes available to asynchronous replay work.
  const initialModesRef = useRef(initialModes)
  initialModesRef.current = initialModes
  const currentLineRef = useRef<string>('')
  const continuityProjectIdRef = useRef<string | undefined>(
    getInstrumentationProjectId(spawnOptions)
  )
  const needsResizeOnReadyRef = useRef<boolean>(false)
  // Track last fitted container dimensions to avoid redundant fit() calls
  const lastContainerWidthRef = useRef<number>(0)
  const lastContainerHeightRef = useRef<number>(0)
  const activityTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const lastActivityUpdateRef = useRef<number>(0)
  const pendingActivityUpdateRef = useRef<{ id: string } | null>(null)
  const lastClipboardOpRef = useRef<number>(0)

  /**
   * F2/DOD-2: one owner for the renderer-side transcript gate.
   *
   * The live-write gate (`handleTerminalOutput`) opens the moment `ptyIdRef` is
   * assigned, so the store gate must flip in that same tick. Routing every
   * transition through this idempotent pair is what stops the two gates from
   * drifting apart again; both are safe to call twice, which is what makes an
   * unconditional release in cleanup correct.
   */
  const claimRendererGate = useCallback((ptyId: string): void => {
    if (rendererGateHeldRef.current) return
    rendererGateHeldRef.current = true
    useTerminalStore.getState().setRendererAttached(ptyId, true)
  }, [])

  const releaseRendererGate = useCallback((ptyId: string): void => {
    if (!rendererGateHeldRef.current) return
    rendererGateHeldRef.current = false
    useTerminalStore.getState().setRendererAttached(ptyId, false)
  }, [])

  /** Clear sidebar activity indicator when this view unmounts (e.g. tab switch). */
  const clearTerminalActivityOnUnmount = useCallback((): void => {
    if (activityTimeoutRef.current) {
      clearTimeout(activityTimeoutRef.current)
      activityTimeoutRef.current = null
    }
    pendingActivityUpdateRef.current = null
    lastActivityUpdateRef.current = 0

    const store = useTerminalStore.getState()
    const storeTerminalId =
      (ptyIdRef.current ? store.findTerminalByPtyId(ptyIdRef.current)?.id : undefined) ??
      (targetId
        ? (store.terminals.find((t) => t.id === targetId)?.id ??
          store.findTerminalByPtyId(targetId)?.id)
        : undefined)

    if (storeTerminalId) {
      store.updateTerminalActivityBatch(storeTerminalId, false, Date.now())
    }
  }, [targetId])

  // 8ms fit debounce, then immediate PTY resize (host no-ops same-size).
  const companionGeometryRef = useRef(companionGeometry)
  companionGeometryRef.current = companionGeometry
  const parkedByPhoneRef = useRef(false)
  parkedByPhoneRef.current = parkedByPhone

  const handlePtyResize = useCallback(
    async (cols: number, rows: number): Promise<void> => {
      const ptyId = ptyIdRef.current
      if (!ptyId) return
      const geometry = companionGeometryRef.current
      if (geometry?.surfaceActive && geometry.preferredMode === 'phone' && isVisibleRef.current) {
        if (geometry.keyboardOpen || !terminalApi.setDisplayMode) return
        try {
          await terminalApi.setDisplayMode(ptyId, 'phone', { cols, rows })
        } catch {
          // Ignore takeover errors during rapid resize
        }
        return
      }
      if (parkedByPhoneRef.current || isMobileWebShell) return
      try {
        await terminalApi.resize(ptyId, cols, rows)
      } catch {
        // Ignore resize errors during rapid resize
      }
    },
    [isMobileWebShell]
  )

  const { forceFit: forceResizeFit } = useTerminalResizeV2({
    onPtyResize: handlePtyResize,
    terminalRef,
    fitAddonRef,
    containerRef,
    isVisible
  })
  const forceResizeFitRef = useRef(forceResizeFit)
  forceResizeFitRef.current = forceResizeFit

  useEffect(() => {
    if (!companionGeometry || !terminalApi.setDisplayMode) return
    const ptyId = ptyIdRef.current
    if (!ptyId) return
    const shouldOwn =
      companionGeometry.surfaceActive && companionGeometry.preferredMode === 'phone' && isVisible
    if (!shouldOwn) {
      void terminalApi.setDisplayMode(ptyId, 'desktop')
    }
  }, [companionGeometry, isVisible])

  useEffect(() => {
    if (!terminalApi.onDisplayModeChanged) return
    return terminalApi.onDisplayModeChanged((event) => {
      if (event.terminalId !== ptyIdRef.current) return
      const parked = event.mode === 'phone'
      parkedByPhoneRef.current = parked
      setParkedByPhone(parked)
      if (!parked) {
        const geometry = companionGeometryRef.current
        if (geometry?.surfaceActive && isVisibleRef.current) {
          geometry.setPreferredMode('desktop')
        }
        forceResizeFit()
      }
      void logFrontendError({
        level: 'warn',
        source: 'terminal.display-mode',
        message: `mode=${event.mode} ${event.cols}x${event.rows}`
      })
    })
  }, [forceResizeFit])

  const [terminalInstance, setTerminalInstance] = useState<Terminal | null>(null)
  useTerminalColorTheme(terminalInstance)

  // 5. CALLBACKS & EFFECTS
  const disposeWebglAddon = useCallback((): void => {
    if (webglRecoveryTimeoutRef.current) {
      clearTimeout(webglRecoveryTimeoutRef.current)
      webglRecoveryTimeoutRef.current = null
    }
    if (webglAddonRef.current) {
      webglAddonRef.current.dispose()
      webglAddonRef.current = null
    }
    webglContextLostRef.current = false
  }, [])

  const performFit = (force = false): boolean => {
    if (!fitAddonRef.current || !terminalRef.current || !containerRef.current) return false
    const rect = containerRef.current.getBoundingClientRect()
    const width = Math.round(rect.width)
    const height = Math.round(rect.height)
    if (
      !force &&
      width > 0 &&
      height > 0 &&
      width === lastContainerWidthRef.current &&
      height === lastContainerHeightRef.current
    ) {
      return false
    }
    try {
      fitAddonRef.current.fit()
      if (width > 0 && height > 0) {
        lastContainerWidthRef.current = width
        lastContainerHeightRef.current = height
      }
      return true
    } catch {
      return false
    }
  }

  const { copySelection, pasteFromClipboard, hasSelection } = useTerminalClipboard({
    terminal: terminalInstance,
    pasteText: async (text: string) => {
      const ptyId = ptyIdRef.current
      if (!ptyId) return
      try {
        const result = await terminalApi.write(ptyId, text)
        if (!result.success && onErrorRef.current) {
          onErrorRef.current(result.error)
        }
      } catch (err) {
        if (onErrorRef.current) {
          onErrorRef.current(
            err instanceof Error ? err.message : tRef.current('errors.pasteWriteFailed')
          )
        }
      }
    },
    onImagePaste: async () => {
      const ptyId = ptyIdRef.current
      if (!ptyId) return
      // Send Ctrl+V byte to PTY - CLI apps like OpenCode read the OS clipboard directly
      await terminalApi.write(ptyId, '\x16')
    }
  })

  // The mount effect runs once, so it cannot close over the paste callback.
  const pasteFromClipboardRef = useRef(pasteFromClipboard)
  pasteFromClipboardRef.current = pasteFromClipboard

  useEffect(() => {
    if (externalTerminalId) ptyIdRef.current = externalTerminalId
  }, [externalTerminalId])

  // biome-ignore lint/correctness/useExhaustiveDependencies: re-run on spawnOptions change but read latest via ref
  useEffect(() => {
    if (continuityProjectIdRef.current)
      continuityProjectIdRef.current = getInstrumentationProjectId(spawnOptionsRef.current)
  }, [spawnOptions])

  const instrumentationProjectId = getInstrumentationProjectId(spawnOptions)

  useEffect(() => {
    if (instrumentationProjectId) {
      continuityProjectIdRef.current = instrumentationProjectId
    }
  }, [instrumentationProjectId])

  // Memoize spawn options to prevent unnecessary re-spawns
  // biome-ignore lint/correctness/useExhaustiveDependencies: deps intentionally track specific spawnOptions fields
  const memoizedSpawnOptions = useMemo(
    () => spawnOptions,
    [
      spawnOptions?.shell,
      spawnOptions?.cwd,
      spawnOptions?.cols,
      spawnOptions?.rows,
      spawnOptions?.env
    ]
  )

  // Handle input from xterm to PTY
  const handleTerminalData = useCallback(async (data: string): Promise<void> => {
    const ptyId = ptyIdRef.current
    if (!ptyId) return

    // Track command input for history
    if (data === '\r' || data === '\n') {
      // Enter pressed - capture command
      const command = currentLineRef.current
      currentLineRef.current = ''
      if (command && onCommandRef.current) {
        onCommandRef.current(command)
      }
    } else if (data === '\x7f' || data === '\b') {
      // Backspace
      currentLineRef.current = currentLineRef.current.slice(0, -1)
    } else if (data === '\x03') {
      // Ctrl+C - clear current line
      currentLineRef.current = ''
    } else if (data.length === 1 && data.charCodeAt(0) >= 32) {
      // Printable character
      currentLineRef.current += data
    } else if (data.length > 1) {
      // Pasted text
      currentLineRef.current += data
    }

    try {
      const result = await terminalApi.write(ptyId, data)
      if (!result.success && onErrorRef.current) {
        onErrorRef.current(result.error)
      }
    } catch (err) {
      if (onErrorRef.current) {
        onErrorRef.current(err instanceof Error ? err.message : tRef.current('errors.writeFailed'))
      }
    }
  }, [])

  /**
   * The single way this component puts bytes on screen.
   *
   * xterm's WebGL renderer keeps its own cell model; a bare `write()` updates
   * the buffer but can leave that model showing the previous frame, which is
   * why a restored terminal used to sit on stale content until a scroll or a
   * pane resize forced a repaint. `WebglScrollRepair.onWrite` is the repaint,
   * and routing every write through here is what stops a future write site
   * from silently skipping it — only the live-output path used to call it, and
   * all seven restore/replay writes did not.
   *
   * The repair is armed from xterm's write callback rather than straight after
   * `write()`: parsing is asynchronous, so repainting immediately would rebuild
   * the model from the pre-write buffer and leave the same staleness behind.
   *
   * D-3: whatever runs in that callback must contain its own failures. xterm's
   * `_innerWrite` invokes it as a bare `cb()` with no exception guard, so a
   * throw leaves `_bufferOffset` stalled and `_scheduleInnerWrite` unarmed —
   * this terminal's output freezes for good, and not even a resize recovers
   * it. `repairNow` wraps both its steps today; anything added here needs the
   * same treatment.
   */
  const writeToTerminal = useCallback(
    (terminal: Pick<Terminal, 'write'>, data: string | Uint8Array): void => {
      terminal.write(data, () => {
        webglScrollRepairRef.current?.onWrite()
      })
    },
    []
  )

  // Initialize terminal, set up IPC listeners, and spawn PTY
  // biome-ignore lint/correctness/useExhaustiveDependencies: intentionally narrow deps; a full list would recreate the terminal instance on every render
  useEffect(() => {
    const debugId = `${instanceId}-${Date.now().toString().slice(-6)}`
    let disposed = false

    devLog(`[ConnectedTerminal] MOUNT [${debugId}]`, {
      instanceId,
      externalTerminalId,
      autoSpawn,
      spawnOptions,
      isVisible
    })

    if (!containerRef.current) {
      devLog(`[ConnectedTerminal] SKIP [${debugId}]: no container`)
      return
    }

    // Check if we're initializing a new terminal (different from previous)
    const terminalKey = externalTerminalId ?? 'new'
    devLog(`[ConnectedTerminal] terminalKey check [${debugId}]`, {
      terminalKey,
      didInit: didInitRef.current,
      initializedKey: initializedTerminalIdRef.current,
      willSkip: didInitRef.current && initializedTerminalIdRef.current === terminalKey
    })

    if (didInitRef.current && initializedTerminalIdRef.current === terminalKey) {
      devLog(`[ConnectedTerminal] SKIP [${debugId}]: already initialized for ${terminalKey}`)
      return
    }

    // Reset init state for new terminal
    didInitRef.current = true
    initializedTerminalIdRef.current = terminalKey

    devLog(`[ConnectedTerminal] INITIALIZING [${debugId}] for key: ${terminalKey}`)

    // Merge platform-aware options with dynamic app settings
    const terminalOptions = {
      ...getTerminalOptions(navigator.platform),
      fontFamily,
      fontSize,
      scrollback: bufferSize,
      screenReaderMode
    }

    // Check for a cached terminal preserved across project switches.
    // If found, reuse it (preserves scrollback, alt buffer, cursor, etc.)
    // and skip both terminal.open() and transcript replay.
    const cacheKey = externalTerminalId || undefined
    const cachedSession = cacheKey ? takeCachedTerminal(cacheKey) : undefined

    let terminal: Terminal
    let fitAddon: FitAddon
    let searchAddon: SearchAddon
    if (cachedSession) {
      devLog(`[ConnectedTerminal] RESTORED cached terminal`, {
        cacheKey
      })
      terminal = cachedSession.terminal
      fitAddon = cachedSession.fitAddon
      searchAddon = cachedSession.searchAddon
      applyThemeToTerminal(terminal, getActiveTerminalTheme())
      applyTerminalRenderOptions(terminal)
      terminal.options.cursorBlink = terminalOptions.cursorBlink
      terminal.options.screenReaderMode = terminalOptions.screenReaderMode
    } else {
      terminal = new Terminal(terminalOptions)
      fitAddon = new FitAddon()
      terminal.loadAddon(fitAddon)
      searchAddon = new SearchAddon()
      terminal.loadAddon(searchAddon)
    }
    terminalRef.current = terminal
    setTerminalInstance(terminal)
    fitAddonRef.current = fitAddon
    searchAddonRef.current = searchAddon

    const handleFilePathActivate = async (event: MouseEvent, uri: string): Promise<void> => {
      if (!event.ctrlKey && !event.metaKey) {
        return
      }

      event.preventDefault()

      try {
        const record = useTerminalStore.getState().findTerminalByPtyId(ptyIdRef.current || '')
        // Resolve against the terminal's OWN project, not whichever project the
        // sidebar happens to have selected — clicking a path in a background
        // project's terminal otherwise joins it to the wrong root and reports a
        // spurious "file not found".
        const ownerProjectRoot = record?.projectId
          ? useProjectStore.getState().projects.find((p) => p.id === record.projectId)?.path
          : undefined
        const result = await openFilePathFromTerminal(uri, {
          cwd: record?.cwd,
          projectRoot: ownerProjectRoot ?? activeProjectPathRef.current
        })

        if (!result.ok) {
          toast.error(result.message)
        }
      } catch (error) {
        console.error('[Terminal File Link Open Failed]', error)
        toast.error(tRef.current('links.openFileFailed'))
      }
    }

    const handleUrlActivate = async (event: MouseEvent, url: string): Promise<void> => {
      if (!event.ctrlKey && !event.metaKey) {
        return
      }

      event.preventDefault()

      if (!isSupportedTerminalUrl(url)) {
        toast.error(tRef.current('links.unsupportedUrl'))
        return
      }

      try {
        await openTerminalUrl(url)
      } catch (error) {
        console.error('[Terminal URL Link Open Failed]', error)
        toast.error(tRef.current('links.openUrlFailed'))
      }
    }

    fileLinkProviderDisposableRef.current = terminal.registerLinkProvider({
      provideLinks(y, callback) {
        const line = terminal.buffer.active.getLine(y - 1)?.translateToString(true) ?? ''
        const pathLinks = buildTerminalPathLinks(line, y, handleFilePathActivate)
        const urlLinks = buildTerminalUrlLinks(line, y, handleUrlActivate)
        callback([...urlLinks, ...pathLinks])
      }
    })

    if (cachedSession) {
      // Reattach the preserved xterm element to the new container.
      // This avoids losing scrollback, alt-buffer, and cursor state.
      if (containerRef.current && terminal.element) {
        containerRef.current.appendChild(terminal.element)
      }

      // Note: the actual fix for "frozen terminal after rapid project
      // switches" lives in terminal-cache.ts (cacheTerminal disposes any
      // stale prior occupant before storing a new one). The fresh
      // component instance always arrives here with webglAddonRef.current
      // === null (the previous instance disposed its addon during cleanup),
      // so a guarded dispose here would be a no-op. We just reset the
      // context-lost flag so the WebGL addon load further down treats this
      // as a clean mount.
      webglContextLostRef.current = false

      // A remounted terminal is the active tab, so `needsSurfaceRestoreRef`
      // starts false (it is seeded from `!isVisible`) and the double-RAF
      // repair below never fires on any remount path — project switch, pane
      // fullscreen toggle, or jumping to a hidden terminal. Arm it here: those
      // paths reattach an existing xterm without writing a single byte, so
      // neither `onWrite` nor a hide/show flip is available to invalidate the
      // stale render model.
      needsSurfaceRestoreRef.current = true

      // Force a full refresh so the renderer repaints after DOM reattachment.
      // This one can still be swallowed: `RenderService._isPaused` is only
      // cleared from an IntersectionObserver callback, which cannot have been
      // delivered yet in this same task, so a terminal that was hidden long
      // enough to be observed as non-intersecting only records
      // `_needsFullRefresh`. The repair above is what actually lands.
      terminal.refresh(0, terminal.rows - 1)
    } else {
      terminal.open(containerRef.current)
    }

    // After the branch join so cached and fresh terminals converge on the same
    // width semantics, and after open() because the activeVersion setter needs
    // the addon registered first.
    ensureTerminalUnicode11(terminal)

    const pixelScroll = attachPixelSmoothScroll(terminal)
    pixelScrollRef.current = pixelScroll

    // Intercept keyboard shortcuts before xterm processes them
    // Return false to prevent xterm from handling, true to let xterm handle
    terminal.attachCustomKeyEventHandler((event: KeyboardEvent) => {
      if (event.type !== 'keydown') return true

      const shortcuts = shortcutsRef.current

      // Check if this key matches any app shortcut
      // On macOS: Ctrl+key shortcuts should pass through to the shell (not intercepted by app)
      // Only ⌘+key shortcuts are intercepted by the app on macOS
      if (isAppOwnedTerminalShortcut(event, shortcuts)) {
        // On macOS inside a terminal, don't intercept ctrl+... shortcuts from the app config.
        // These are ctrl-key combos that should go to the shell (e.g., ctrl+r = reverse-i-search).
        // The ⌘ equivalent is handled by the clipboardModifier block above.
        if (isMac && event.ctrlKey && !event.metaKey) {
          // Passthrough: let xterm send the raw ctrl sequence to the shell
          return true
        }

        // Don't call stopPropagation() - let event bubble to window handler
        // Return false to prevent xterm from handling the event
        return false
      }

      // Handle copy/paste/select all keyboard shortcuts
      // macOS convention: ⌘+C/V/A for clipboard operations, Ctrl+C = SIGINT
      // Windows/Linux convention: Ctrl+C/V/A for everything
      const clipboardModifier = isPlatformModifier(event)

      if (clipboardModifier) {
        const now = Date.now()
        // Rate-limit the clipboard operations themselves, never the modifier.
        // Gating on `clipboardModifier` alone swallowed EVERY key held with
        // Cmd/Ctrl for the whole window — and a remote-desktop client that
        // synthesises keydowns with a spurious modifier flag turns that into
        // "type a line, one character arrives".
        const clipboardOpRateLimited = now - lastClipboardOpRef.current < CLIPBOARD_RATE_LIMIT_MS

        switch (event.key.toLowerCase()) {
          case 'c':
            // Copy: if selection exists, copy and prevent xterm handling
            // Otherwise allow xterm to handle (for interrupt signal)
            if (terminal.hasSelection()) {
              event.preventDefault()
              if (clipboardOpRateLimited) return false
              const selection = terminal.getSelection()
              if (selection) {
                lastClipboardOpRef.current = now
                // Use the hook's copySelection for consistency
                void copySelection()
              }
              return false
            }
            // No selection - allow xterm to send Ctrl+C (interrupt signal)
            return true

          case 'v':
            // Paste: read clipboard and paste to terminal. In a non-secure
            // context (HTTP+bare-IP — GH-588), `navigator.clipboard` is
            // undefined and the facade's paste-event fallback can't fire
            // because preventDefault() here would suppress the very paste
            // event it waits on. Degrade to xterm's native paste (the browser
            // paste event on xterm's helper textarea) in that case; the
            // secure-context path keeps the bracketed + sanitized paste via
            // the facade (pasteFromClipboard).
            if (typeof navigator !== 'undefined' && typeof navigator.clipboard === 'undefined') {
              if (clipboardOpRateLimited) return false
              lastClipboardOpRef.current = now
              return true
            }
            event.preventDefault()
            if (clipboardOpRateLimited) return false
            lastClipboardOpRef.current = now
            // Use the hook's pasteFromClipboard for consistency
            void pasteFromClipboard()
            return false

          case 'a':
            // Select all
            terminal.selectAll()
            return false
        }
      }

      if (trapTerminalTabFocusNavigation(event)) {
        return true
      }

      // Shift+Enter → newline (LF). xterm.js sends \r for Enter regardless of
      // Shift, so multiline TUI apps (Claude Code, Ink, etc.) can't tell it
      // apart from a plain Enter and treat it as "submit". Send \n (LF) — the
      // same byte Ctrl+J produces — so Shift+Enter inserts a newline instead.
      // Pure Shift+Enter only: ignore it when other modifiers are held (so
      // Cmd/Ctrl+Shift+Enter app shortcuts are unaffected) and during IME
      // composition.
      if (
        event.key === 'Enter' &&
        event.shiftKey &&
        !event.ctrlKey &&
        !event.metaKey &&
        !event.altKey &&
        !event.isComposing
      ) {
        event.preventDefault()
        handleTerminalData('\n')
        return false
      }

      // A remote-desktop text chunk: deliver the whole string, not the single
      // character xterm's _keyPress would truncate it to. Clearing the textarea
      // undoes the accumulation the cancelled default would otherwise leave
      // behind (xtermjs/xterm.js#6078).
      if (isInjectedTextChunk(event)) {
        event.preventDefault()
        if (terminal.textarea) terminal.textarea.value = ''
        handleTerminalData(event.key)
        return false
      }

      // Deliver characters none of xterm's three input paths would send.
      // preventDefault keeps them out of the hidden textarea and suppresses
      // the keypress event, so no other path can send them a second time.
      if (isKeyXtermWillDrop(event, terminal.textarea)) {
        event.preventDefault()
        handleTerminalData(event.key)
        return false
      }

      return true
    })

    // WebGL addon loading with context loss recovery
    const loadWebglAddon = (term: Terminal, isRecovery: boolean = false): void => {
      if (!shouldUseWebglRenderer(rendererPreferenceRef.current)) {
        // Unreachable at HEAD — all four call sites are guarded on the same
        // preference. It nulls through `disposeWebglAddon` anyway because a
        // bare null assignment is the one mutation no test can catch: the
        // addon would stay attached and rendering while `isWebglActive` reads
        // false, which is exactly the state in which the repair skips
        // `clearWebglRenderModel` and the leftover glyphs come back.
        disposeWebglAddon()
        return
      }
      if (webglAddonRef.current) {
        return
      }
      if (webglRecoveryAttemptsRef.current >= MAX_WEBGL_RECOVERY_ATTEMPTS) {
        console.warn('WebGL recovery attempts exhausted, falling back to DOM renderer')
        recordTerminalContinuityEvent({
          name: 'renderer-recovery-exhausted',
          ptyId: ptyIdRef.current ?? undefined,
          details: {
            attempts: webglRecoveryAttemptsRef.current,
            maxAttempts: MAX_WEBGL_RECOVERY_ATTEMPTS,
            isRecovery
          }
        })
        return
      }
      try {
        recordTerminalContinuityEvent({
          name: 'renderer-recovery-attempted',
          ptyId: ptyIdRef.current ?? undefined,
          details: {
            attempt: webglRecoveryAttemptsRef.current + 1,
            maxAttempts: MAX_WEBGL_RECOVERY_ATTEMPTS,
            isRecovery,
            renderer: 'webgl'
          }
        })
        const webglAddon = new WebglAddon()
        const scrollRepair = webglScrollRepairRef.current
        if (scrollRepair) {
          webglAddon.onAddTextureAtlasCanvas(() => {
            scrollRepair.markAtlasDirty()
          })
          webglAddon.onRemoveTextureAtlasCanvas(() => {
            // Shared atlas pages merged. Refresh this renderer after idle;
            // never clearTextureAtlas — that leaves sibling terminals with
            // stale UVs on the shared cache.
            scrollRepair.noteAtlasMerged()
          })
        }
        webglAddon.onContextLoss(() => {
          webglAddon.dispose()
          webglAddonRef.current = null
          // Mark context as lost for recovery decisions
          webglContextLostRef.current = true
          if (!isVisibleRef.current || !shouldUseWebglRenderer(rendererPreferenceRef.current)) {
            webglContextLostRef.current = false
            return
          }
          // Increment recovery counter BEFORE scheduling recovery
          webglRecoveryAttemptsRef.current++
          // Clear any pending recovery timeout
          if (webglRecoveryTimeoutRef.current) {
            clearTimeout(webglRecoveryTimeoutRef.current)
          }
          // Delay before recovery to avoid rapid-fire loops
          webglRecoveryTimeoutRef.current = setTimeout(() => {
            webglRecoveryTimeoutRef.current = null
            loadWebglAddon(term, true)
          }, WEBGL_CONTEXT_LOSS_RECOVERY_DELAY_MS)
        })
        term.loadAddon(webglAddon)
        webglAddonRef.current = webglAddon
        // Clear context lost flag on successful load
        webglContextLostRef.current = false
        if (!isRecovery) {
          webglRecoveryAttemptsRef.current = 0
        }
        recordTerminalContinuityEvent({
          name: 'renderer-recovery-succeeded',
          ptyId: ptyIdRef.current ?? undefined,
          details: {
            attempt: webglRecoveryAttemptsRef.current + 1,
            isRecovery,
            renderer: 'webgl'
          }
        })
        try {
          term.refresh(0, Math.max(0, term.rows - 1))
        } catch (error) {
          void logFrontendError({
            level: 'warn',
            source: 'ConnectedTerminal:webgl-load',
            message: `refresh after WebGL load failed: ${
              error instanceof Error ? error.message : String(error)
            }`,
            stack: error instanceof Error ? error.stack : undefined
          })
        }
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error)
        console.warn('WebGL addon failed to load, falling back to DOM renderer:', error)
        webglAddonRef.current = null
        webglRecoveryAttemptsRef.current++
        recordTerminalContinuityEvent({
          name: 'renderer-recovery-failed',
          ptyId: ptyIdRef.current ?? undefined,
          details: {
            error: message,
            attempt: webglRecoveryAttemptsRef.current,
            isRecovery,
            renderer: 'webgl'
          }
        })
      }
    }

    const scrollRepair = createWebglScrollRepair({
      getTerminal: () => terminalRef.current,
      // The addon ref is the live answer: it is nulled while the tab is hidden
      // and whenever WebGL fails to load and the DOM renderer takes over, and
      // clearing a DOM renderer's model is a visible black frame rather than a
      // cheap rebuild.
      //
      // Skipped entirely on the alternate screen. The residue this discards the
      // model for is a normal-buffer effect — zsh-syntax-highlighting redrawing
      // with `\b` relative positioning leaves stale rows behind. A full-screen
      // TUI repaints with absolute positioning, which is idempotent, so there is
      // nothing there for a model rebuild to fix. The cost, meanwhile, peaks
      // exactly there: such an app writes continuously (its own scrolling is a
      // full repaint), so the write burst's leading edge fires constantly and
      // every cell would be re-uploaded each time. `refresh` still runs, so the
      // screen is still repainted — only the model is kept.
      rebuildSurface: (terminal) => {
        if (terminalRef.current?.buffer.active.type === 'alternate') return
        clearWebglRenderModel(terminal, !!webglAddonRef.current)
      }
    })
    webglScrollRepairRef.current = scrollRepair
    const scrollDisposable = terminal.onScroll(() => {
      scrollRepair.onScroll()
    })

    if (isVisibleRef.current && shouldUseWebglRenderer(rendererPreferenceRef.current)) {
      loadWebglAddon(terminal)
    }
    // Store reference for recovery handlers to use
    loadWebglAddonRef.current = loadWebglAddon

    // Defer initial fit to next animation frame so the WebGL renderer has time
    // to fully initialize its internal _renderer.value before we call dimensions.
    // Calling fit() synchronously after loadWebglAddon() causes an uncaught
    // "Cannot read properties of undefined (reading 'dimensions')" from xterm.
    requestAnimationFrame(() => {
      performFit(true)
    })

    if (autoFocus) {
      terminal.focus()
    }

    // Set up resize observer

    // macOS never delivers Cmd+V to the webview. The native Edit menu's
    // predefined Paste item owns that key equivalent, and AppKit resolves key
    // equivalents before the event reaches the key window's first responder —
    // the same preemption documented for Cmd+A in `src-tauri/src/lib.rs`, where
    // it cost us Select All. So `case 'v'` in the key handler above is dead code
    // on macOS; what actually runs is AppKit's `paste:`, whose DOM `paste` event
    // xterm answers by reading `text/plain` and nothing else
    // (`@xterm/xterm` Clipboard.ts `handlePasteEvent`).
    //
    // Copy an image out of an app that puts only image flavours on the
    // pasteboard — Lark/Feishu offers PNG, TIFF, JPEG, GIF, AVIF and a file URL,
    // and no text at all — and that read returns the empty string. Cmd+V then
    // does nothing whatsoever, while the right-click menu works, because it
    // calls `pasteFromClipboard` directly and that one asks the OS clipboard
    // about images.
    //
    // Trigger on "the event carries no text", not on "the event carries an
    // image": whether WebKit populates `clipboardData.items` with image types
    // for a paste into a plain textarea is not something this code can verify,
    // and the empty-text case is precisely the one that does nothing today.
    // Capture phase so it lands ahead of xterm's own listeners, which sit on the
    // textarea and on `terminal.element` inside this container.
    const pasteContainer = containerRef.current
    const handlePasteCapture = (event: ClipboardEvent): void => {
      if (event.clipboardData?.getData('text/plain')) return
      // stopPropagation as well as preventDefault: xterm's handler would
      // otherwise still run and emit a bracketed paste wrapped around nothing.
      event.preventDefault()
      event.stopPropagation()
      void pasteFromClipboardRef.current()
    }
    pasteContainer?.addEventListener('paste', handlePasteCapture, { capture: true })

    // Listen for input from xterm
    const dataDisposable = terminal.onData(handleTerminalData)

    // The live-write gate has to open before the attach round trip (F2/DOD-2:
    // it flips in the same tick as the capture gate, or both write the same
    // bytes), but the detached interval is only known after it. Chunks that
    // arrive in between are therefore newer than a transcript that has not been
    // written yet — painting them straight through puts them on screen ahead of
    // it. Worse, PTY read boundaries are not escape-sequence boundaries: a
    // transcript ending on `\x1b[3` would have its `1m` printed literally first
    // and then swallow the head of the next live chunk into a CSI parameter
    // list. Holding them until the replay has been written keeps the halves
    // adjacent, and xterm's parser is stateful across `write()` calls, so it
    // stitches them back together.
    //
    // `null` means "not holding" — the ordinary write-through state.
    let replayHold: Uint8Array[] | null = null
    let replayHoldBytes = 0
    let replayHoldOverflowed = false

    const beginReplayHold = (): void => {
      replayHold = []
      replayHoldBytes = 0
      replayHoldOverflowed = false
    }

    /**
     * Drain the hold in arrival order and return to write-through. Idempotent,
     * so every exit path from the replay can call it without coordinating.
     */
    const flushReplayHold = (): void => {
      const held = replayHold
      replayHold = null
      replayHoldBytes = 0
      if (!held || held.length === 0) return
      for (const chunk of held) {
        try {
          writeToTerminal(terminal, chunk)
        } catch (error) {
          // A disposed terminal is the realistic cause, and the remaining
          // chunks would fail the same way. Report once and stop rather than
          // let a throw escape into the caller's finally.
          console.error(
            '[Terminal Replay Flush Failed]',
            error instanceof Error ? error.message : String(error)
          )
          return
        }
      }
    }

    // Set up IPC listeners BEFORE spawning to avoid missing data
    // Cache ptyId -> terminalId mapping to avoid repeated store lookups
    let cachedTerminalId: string | null = null
    const handleTerminalOutput = (id: string, data: Uint8Array): void => {
      if (id === ptyIdRef.current && terminalRef.current) {
        if (replayHold) {
          replayHold.push(data)
          replayHoldBytes += data.length
          if (replayHoldBytes >= MAX_REPLAY_HOLD_BYTES) {
            replayHoldOverflowed = true
            flushReplayHold()
          }
        } else {
          writeToTerminal(terminalRef.current, data)
        }
        // Resolve terminal record ID (cached to avoid linear scan)
        if (!cachedTerminalId) {
          const terminalRecord = useTerminalStore.getState().findTerminalByPtyId(id)
          if (terminalRecord) {
            cachedTerminalId = terminalRecord.id
          }
        }
        if (cachedTerminalId) {
          const now = Date.now()
          const timeSinceLastUpdate = now - lastActivityUpdateRef.current

          // If enough time has passed since last update, update immediately
          if (timeSinceLastUpdate >= ACTIVITY_DEBOUNCE_MS) {
            useTerminalStore.getState().updateTerminalActivityBatch(cachedTerminalId, true, now)
            lastActivityUpdateRef.current = now
          } else {
            // Otherwise, store pending update for later
            pendingActivityUpdateRef.current = { id: cachedTerminalId }
          }

          // Clear existing activity timeout and set new one
          if (activityTimeoutRef.current) {
            clearTimeout(activityTimeoutRef.current)
          }
          const termId = cachedTerminalId
          activityTimeoutRef.current = setTimeout(() => {
            // Flush any pending activity update
            if (pendingActivityUpdateRef.current) {
              useTerminalStore
                .getState()
                .updateTerminalActivityBatch(pendingActivityUpdateRef.current.id, false, Date.now())
              pendingActivityUpdateRef.current = null
            } else {
              // Clear activity after 2 seconds of inactivity
              useTerminalStore.getState().updateTerminalActivityBatch(termId, false, Date.now())
            }
            activityTimeoutRef.current = null
            lastActivityUpdateRef.current = 0
          }, 2000)
        }
      }
    }
    // This renderer is the single live writer for its PTY. Ownership is claimed
    // up front and the id bound once known: the spawn path only learns its id
    // after the IPC round trip, and observing every PTY to filter by id would
    // reintroduce the possibility of a second writer.
    const primaryDataHandle = registerPrimaryTerminalData((data) => {
      const ptyId = ptyIdRef.current
      if (ptyId) handleTerminalOutput(ptyId, data)
    })
    primaryDataHandleRef.current = primaryDataHandle
    if (externalTerminalId) primaryDataHandle.bind(externalTerminalId)
    cleanupDataListenerRef.current = () => {
      primaryDataHandleRef.current = null
      primaryDataHandle.dispose()
    }

    cleanupExitListenerRef.current = terminalApi.onExit(
      (id: string, exitCode: number, signal?: number) => {
        if (id === ptyIdRef.current && onExitRef.current) {
          onExitRef.current(exitCode, signal)
        }
      }
    )

    // Spawn terminal if no external ID provided and auto-spawn enabled
    const initTerminal = async (): Promise<void> => {
      const spawnDebugId = `${instanceId}-spawn-${Date.now().toString().slice(-6)}`
      const recordReplayEvent = (
        name:
          | 'restore-replay-attempted'
          | 'restore-replay-succeeded'
          | 'restore-replay-failed'
          | 'restore-replay-skipped',
        details?: Record<string, unknown>,
        terminalEventId?: string,
        ptyId?: string
      ): void => {
        const projectId = continuityProjectIdRef.current
        recordTerminalContinuityEvent({
          name,
          correlationId: getOrCreateProjectContinuityCorrelation(projectId),
          projectId,
          terminalId: terminalEventId,
          ptyId,
          details
        })
      }

      devLog(`[ConnectedTerminal.initTerminal] START [${spawnDebugId}]`, {
        externalTerminalId,
        autoSpawn,
        spawnInFlight: spawnInFlightRef.current,
        hasPtyId: !!ptyIdRef.current
      })

      // Fit to get real dimensions BEFORE spawning
      performFit(true)
      const spawnCols = terminal.cols || 80
      const spawnRows = terminal.rows || 24

      if (!externalTerminalId) {
        if (!autoSpawn) {
          devLog(`[ConnectedTerminal.initTerminal] SKIP [${spawnDebugId}]: autoSpawn is false`)
          return
        }
        if (spawnInFlightRef.current || ptyIdRef.current) {
          devLog(
            `[ConnectedTerminal.initTerminal] SKIP [${spawnDebugId}]: already spawning or has PTY`
          )
          return
        }
      } else if (isTerminalPendingPtyAssignment(externalTerminalId)) {
        devLog(`SKIP autoSpawn: terminal ${externalTerminalId} pending PTY assignment from restore`)
        return
      }

      if (!externalTerminalId) {
        spawnInFlightRef.current = true
        devLog(`[ConnectedTerminal.initTerminal] SPAWNING [${spawnDebugId}]`, {
          cols: spawnCols,
          rows: spawnRows,
          spawnOpts: memoizedSpawnOptions
        })

        try {
          const spawnOpts = {
            ...memoizedSpawnOptions,
            // Ensure empty shell string is treated as undefined so Rust uses default
            shell: memoizedSpawnOptions?.shell || undefined,
            cols: spawnCols,
            rows: spawnRows
          }
          const result = await terminalApi.spawn(spawnOpts)
          devLog(`[ConnectedTerminal.initTerminal] SPAWN RESULT [${spawnDebugId}]`, {
            success: result.success,
            code: result.success ? undefined : result.code,
            ptyId: result.success ? result.data.id : 'FAILED'
          })

          if (result.success) {
            // Update ref immediately so listener can start processing data
            ptyIdRef.current = result.data.id
            // Claim the live-writer slot now that spawn has produced the id.
            primaryDataHandleRef.current?.bind(result.data.id)
            claimRendererGate(result.data.id)
            // No await sits between this gate and the replay below, so the hold
            // is empty on this path today. It is armed anyway so the ordering
            // invariant is a property of the code rather than of whoever next
            // inserts an await here.
            beginReplayHold()
            void addRendererRef(result.data.id, instanceIdRef.current)
            // If tab was visible before PTY was ready, flush deferred fit+resize now
            if (needsResizeOnReadyRef.current) {
              needsResizeOnReadyRef.current = false
              performFit(true)
              if (
                companionGeometryRef.current?.surfaceActive &&
                companionGeometryRef.current.preferredMode === 'phone' &&
                terminalApi.setDisplayMode
              ) {
                terminalApi
                  .setDisplayMode(result.data.id, 'phone', {
                    cols: terminal.cols,
                    rows: terminal.rows
                  })
                  .catch(() => {})
              } else if (!isMobileWebShell) {
                terminalApi.resize(result.data.id, terminal.cols, terminal.rows).catch(() => {})
              }
            }
            forceResizeFitRef.current()
            // Register terminal for scrollback persistence
            registerTerminal(result.data.id, terminal)
            const terminalStoreState = useTerminalStore.getState()
            const transcript = terminalStoreState.peekTranscript(result.data.id)
            const transcriptLooksPartial =
              transcript.includes('\u001b[?1049h') || transcript.includes('\u001b[?47h')
            recordReplayEvent(
              'restore-replay-attempted',
              {
                mode: transcript ? 'transcript' : initialScrollback?.length ? 'scrollback' : 'none',
                transcriptLength: transcript.length,
                initialScrollbackLineCount: initialScrollback?.length ?? 0,
                source: 'spawned-terminal',
                alternateScreenDetected: transcriptLooksPartial
              },
              storeTerminalId,
              result.data.id
            )
            try {
              if (transcript) {
                if (transcriptLooksPartial) {
                  // R3: replay the full captured DEC mode set (alt-screen + bracketed-paste
                  // + cursor + mouse), not just alt-screen — a partial/trimmed transcript
                  // may miss the initial mode sequences. Idempotent with modes in the stream.
                  writeToTerminal(terminal, buildRehydrateSequences(initialModesRef.current))
                  writeToTerminal(terminal, transcript)
                  writeToTerminal(
                    terminal,
                    `\x1b[33m\r\n[${tRef.current('restore.partialNote')}]\x1b[0m\r\n`
                  )
                } else {
                  writeToTerminal(terminal, buildRehydrateSequences(initialModesRef.current))
                  writeToTerminal(terminal, transcript)
                }
                terminalStoreState.consumeTranscript(result.data.id)
                recordReplayEvent(
                  'restore-replay-succeeded',
                  {
                    mode: 'transcript',
                    transcriptLength: transcript.length,
                    source: 'spawned-terminal',
                    fullFidelity: !transcriptLooksPartial,
                    restoreLimitation: transcriptLooksPartial
                      ? 'alternate-screen-or-in-place-redraw'
                      : undefined
                  },
                  storeTerminalId,
                  result.data.id
                )
              } else if (initialScrollback && initialScrollback.length > 0) {
                const payload = buildScrollbackRestorePayload(initialScrollback, initialModes)
                if (payload) writeToTerminal(terminal, payload)
                recordReplayEvent(
                  'restore-replay-succeeded',
                  {
                    mode: 'scrollback',
                    initialScrollbackLineCount: initialScrollback.length,
                    // R3: whether DEC mode rehydrate sequences were emitted.
                    modesReplayed: Boolean(initialModes),
                    source: 'spawned-terminal'
                  },
                  storeTerminalId,
                  result.data.id
                )
              } else {
                recordReplayEvent(
                  'restore-replay-skipped',
                  {
                    reason: 'no-persisted-history',
                    source: 'spawned-terminal'
                  },
                  storeTerminalId,
                  result.data.id
                )
              }
            } catch (error) {
              const replayError = error instanceof Error ? error.message : String(error)
              recordReplayEvent(
                'restore-replay-failed',
                {
                  mode: transcript
                    ? 'transcript'
                    : initialScrollback?.length
                      ? 'scrollback'
                      : 'none',
                  error: replayError,
                  source: 'spawned-terminal'
                },
                storeTerminalId,
                result.data.id
              )
              console.error('[Terminal Replay Failed]', replayError)
              if (onErrorRef.current) onErrorRef.current(replayError)
            } finally {
              // In `finally`, not after the write: a throwing replay must not
              // strand live output in the hold for the rest of the session.
              flushReplayHold()
            }
            // Write one-time info line if project env vars were applied
            if (memoizedSpawnOptions?.env && Object.keys(memoizedSpawnOptions.env).length > 0) {
              const envCount = Object.keys(memoizedSpawnOptions.env).length
              writeToTerminal(
                terminal,
                `\x1b[36m\r\n[${tRef.current('environment.applied', {
                  count: envCount,
                  formattedCount: envCount.toLocaleString()
                })}]\x1b[0m\r\n`
              )
            }
            // Restore scroll position if cached from previous pane
            restoreScrollPosition(result.data.id, terminal)
            if (onSpawnedRef.current) {
              onSpawnedRef.current(result.data.id)
            }
            if (onBoundToStoreTerminalRef.current) {
              onBoundToStoreTerminalRef.current(result.data.id)
            }
            // CAP-3: capture the issued lease into the terminal store
            // (in-memory only). Runs after onBoundToStoreTerminal so the
            // store record's ptyId is set and the linear scan finds it.
            if (result.data.claim) {
              useTerminalStore.getState().setTerminalClaim(result.data.id, result.data.claim)
            }
          } else {
            const cleanupFailure = recordTerminalCleanupFailure(result)
            if (cleanupFailure) setCleanupRecoveryId(cleanupFailure.terminalId)
            const errorMsg = cleanupFailure
              ? tRef.current('cleanup.quarantined')
              : result.error || tRef.current('errors.unknownSpawn')
            console.error('[Terminal Spawn Failed]', {
              code: result.code,
              cleanupStage: cleanupFailure?.cleanupStage
            })
            writeToTerminal(
              terminal,
              `\x1b[31m\r\n${tRef.current('errors.spawnProcessFailed')}:\r\n${errorMsg}\x1b[0m\r\n`
            )
            if (onErrorRef.current) onErrorRef.current(errorMsg)
          }
        } catch (err) {
          const errorMsg = err instanceof Error ? err.message : tRef.current('errors.spawnFailed')
          console.error('[Terminal Spawn Exception]', errorMsg)
          writeToTerminal(
            terminal,
            `\x1b[31m\r\n${tRef.current('errors.spawnException')}:\r\n${errorMsg}\x1b[0m\r\n`
          )
          if (onErrorRef.current) onErrorRef.current(errorMsg)
        } finally {
          spawnInFlightRef.current = false
        }
      } else {
        // External terminal ID provided - register and restore scrollback
        devLog(`[ConnectedTerminal.initTerminal] EXTERNAL PTY [${spawnDebugId}]`, {
          externalTerminalId
        })
        // Set ptyIdRef so that resize/recovery operations (performFit, terminalApi.resize)
        // work for external terminals just like spawned ones. Without this, the TUI app
        // never receives SIGWINCH on project-switch restore and can't redraw.
        // F2/DOD-2: these two statements must stay adjacent and synchronous. The
        // live-write gate in handleTerminalOutput and the transcript-capture
        // gate in use-terminal-detached-output must flip in the same tick, or
        // the awaited attach below opens a window in which both write the same
        // bytes — whole `ls`-class blocks duplicated on replay.
        ptyIdRef.current = externalTerminalId
        primaryDataHandleRef.current?.bind(externalTerminalId)
        claimRendererGate(externalTerminalId)
        // The gate above is now open and the replay below is one awaited IPC
        // round trip away, so hold live chunks until the detached interval has
        // been written rather than painting them ahead of it.
        beginReplayHold()
        // A cold renderer must complete the host-authorized resume path before
        // it registers a renderer reference. Missing/denied grants stay as
        // disconnected placeholders and never fall back to spawning.
        const { attached, stale, serverReplay } = await attachResumedTerminalRenderer(
          externalTerminalId,
          storeTerminalId,
          instanceIdRef.current
        )
        if (!attached) {
          // Without this the gate stays held for a terminal that never
          // attached, and the transcript stops capturing for a detached PTY.
          releaseRendererGate(externalTerminalId)
          if (!stale && !disposed && onErrorRef.current) {
            onErrorRef.current(tRef.current('resume.disconnectedTitle'))
          }
          return
        }
        if (disposed) {
          releaseRendererGate(externalTerminalId)
          void removeRendererRef(externalTerminalId, instanceIdRef.current)
          return
        }
        rendererRefAttachedRef.current = true
        registerTerminal(externalTerminalId, terminal)
        const terminalStoreState = useTerminalStore.getState()
        const transcript = terminalStoreState.peekTranscript(externalTerminalId)
        const transcriptLooksPartial =
          transcript.includes('\u001b[?1049h') || transcript.includes('\u001b[?47h')
        const transcriptTrimmed = Boolean(
          terminalStoreState.findTerminalByPtyId(externalTerminalId)?.transcriptTrimmed
        )
        // A gap means the host ring buffer could not cover the requested
        // cursor, and then the transcript is the only thing that can.
        const hostReplayCoveredTranscript = serverReplay !== null && !serverReplay.gap
        recordReplayEvent(
          'restore-replay-attempted',
          {
            mode: transcript ? 'transcript' : initialScrollback?.length ? 'scrollback' : 'none',
            transcriptLength: transcript.length,
            initialScrollbackLineCount: initialScrollback?.length ?? 0,
            source: 'external-terminal',
            alternateScreenDetected: transcriptLooksPartial
          },
          storeTerminalId,
          externalTerminalId
        )
        try {
          if (hostReplayCoveredTranscript) {
            // The host already streamed this interval back through the live
            // data path. The transcript covers the very same interval — the one
            // with no renderer mounted — so writing it too is what duplicated
            // whole `ls`-class blocks. Consume without writing.
            if (transcript) {
              terminalStoreState.consumeTranscript(externalTerminalId)
            }
            recordReplayEvent(
              'restore-replay-skipped',
              {
                reason: 'host-replay-authoritative',
                source: 'external-terminal',
                transcriptLength: transcript.length
              },
              storeTerminalId,
              externalTerminalId
            )
          } else if (transcript && !(cachedSession && transcriptTrimmed)) {
            if (cachedSession) {
              // The cached instance holds everything up to the moment it was
              // cached — but not the detached interval that followed, and the
              // arbitration above already established the host did not replay
              // that interval either. Consuming without writing dropped it
              // outright: real output loss, which is why a switched-back
              // terminal showed a genuinely old frame rather than a stale one.
              //
              // Raw continuation, deliberately without `buildRehydrateSequences`:
              // the live instance's DEC modes are already current, and
              // replaying a mount-time snapshot over them would fight the very
              // state the cache preserved.
              //
              // Only safe while the transcript is whole — see the trimmed
              // branch below for why.
              writeToTerminal(terminal, transcript)
            } else if (transcriptLooksPartial) {
              // R3: replay the full captured DEC mode set, not just alt-screen.
              writeToTerminal(terminal, buildRehydrateSequences(initialModesRef.current))
              writeToTerminal(terminal, transcript)
              writeToTerminal(
                terminal,
                `\x1b[33m\r\n[${tRef.current('restore.partialNote')}]\x1b[0m\r\n`
              )
            } else {
              writeToTerminal(terminal, buildRehydrateSequences(initialModesRef.current))
              writeToTerminal(terminal, transcript)
            }
            terminalStoreState.consumeTranscript(externalTerminalId)
            recordReplayEvent(
              'restore-replay-succeeded',
              {
                mode: 'transcript',
                transcriptLength: transcript.length,
                source: 'external-terminal',
                // A cached instance replays raw bytes onto live modes, so the
                // alt-screen heuristic (a cold restore's "we only have a tail"
                // problem) does not apply to it.
                fullFidelity: cachedSession ? true : !transcriptLooksPartial,
                restoreLimitation:
                  !cachedSession && transcriptLooksPartial
                    ? 'alternate-screen-or-in-place-redraw'
                    : undefined,
                // A trimmed transcript can be missing the mode transition that
                // would have taken this instance back out of alt-screen, and
                // the cached branch has nothing to reconcile it against. The
                // replay still happens; this is what makes the one silently
                // wrong case visible.
                ...(transcriptTrimmed ? { transcriptTrimmed: true } : {}),
                ...(replayHoldOverflowed ? { replayHoldOverflowed: true } : {})
              },
              storeTerminalId,
              externalTerminalId
            )
          } else if (cachedSession) {
            // Two cases land here. Either there was no detached output at all —
            // and then the persisted scrollback must stay unwritten, because the
            // reused instance already carries it — or the transcript exists but
            // was trimmed.
            //
            // A trimmed transcript lost its oldest bytes at an arbitrary offset.
            // `trimTranscriptToMaxChars` aligns to the next line break, but a
            // line break is not an escape-sequence boundary and carries none of
            // the preceding state, so the surviving tail can begin mid-sequence
            // or with colours, cursor position and DEC modes it never
            // established. Splicing that raw onto a live screen — which is
            // exactly what the cached path does, having deliberately skipped the
            // mode rehydrate — renders garbage.
            //
            // The span is already lost the moment it was trimmed; the only
            // choice left is how to fail. A coherent screen missing that span
            // beats a garbled one, and a still-running full-screen app repaints
            // over it on its next output anyway.
            if (transcript) {
              terminalStoreState.consumeTranscript(externalTerminalId)
            }
            recordReplayEvent(
              'restore-replay-skipped',
              {
                reason: transcript ? 'transcript-trimmed-unsafe-splice' : 'cached-terminal',
                source: 'external-terminal',
                ...(transcript ? { transcriptLength: transcript.length } : {})
              },
              storeTerminalId,
              externalTerminalId
            )
          } else if (initialScrollback && initialScrollback.length > 0) {
            const payload = buildScrollbackRestorePayload(initialScrollback, initialModes)
            if (payload) writeToTerminal(terminal, payload)
            recordReplayEvent(
              'restore-replay-succeeded',
              {
                mode: 'scrollback',
                initialScrollbackLineCount: initialScrollback.length,
                // R3: whether DEC mode rehydrate sequences were emitted.
                modesReplayed: Boolean(initialModes),
                source: 'external-terminal'
              },
              storeTerminalId,
              externalTerminalId
            )
          } else {
            recordReplayEvent(
              'restore-replay-skipped',
              {
                reason: 'no-persisted-history',
                source: 'external-terminal'
              },
              storeTerminalId,
              externalTerminalId
            )
          }
        } catch (error) {
          const replayError = error instanceof Error ? error.message : String(error)
          recordReplayEvent(
            'restore-replay-failed',
            {
              mode: transcript ? 'transcript' : initialScrollback?.length ? 'scrollback' : 'none',
              error: replayError,
              source: 'external-terminal'
            },
            storeTerminalId,
            externalTerminalId
          )
          console.error('[Terminal Replay Failed]', replayError)
          if (onErrorRef.current) onErrorRef.current(replayError)
        } finally {
          // In `finally`, not after the write: a throwing replay must not
          // strand live output in the hold for the rest of the session.
          flushReplayHold()
        }
        // Write one-time info line if project env vars were applied
        // (env should be passed via spawnOptions by the caller if this terminal was spawned with env vars)
        if (memoizedSpawnOptions?.env && Object.keys(memoizedSpawnOptions.env).length > 0) {
          const envCount = Object.keys(memoizedSpawnOptions.env).length
          writeToTerminal(
            terminal,
            `\x1b[36m\r\n[${tRef.current('environment.applied', {
              count: envCount,
              formattedCount: envCount.toLocaleString()
            })}]\x1b[0m\r\n`
          )
        }
        // Restore scroll position if cached from previous pane
        restoreScrollPosition(externalTerminalId, terminal)
        // Workspace spawn starts at 80x24. Push the fitted grid now so zsh
        // and p10k wrap against the same columns xterm is drawing.
        forceResizeFitRef.current()
        if (onBoundToStoreTerminalRef.current) {
          onBoundToStoreTerminalRef.current(externalTerminalId)
        }
      }
    }

    devLog(`[ConnectedTerminal] Calling initTerminal [${debugId}]`)
    // `finally` rather than the replay's own exit points: init has returns
    // before the replay is even reached (denied resume, unmount mid-attach) and
    // awaits that can reject outside its try. Any of those would otherwise
    // leave the hold armed with no writer for the rest of the mount. Rejections
    // stay unhandled exactly as they were before this wrapper.
    void initTerminal().finally(flushReplayHold)

    return () => {
      disposed = true
      devLog(`[ConnectedTerminal] UNMOUNT [${debugId}]`, {
        instanceId,
        ptyId: ptyIdRef.current,
        externalTerminalId
      })
      // Capture scroll position BEFORE unregistering for pane transitions
      const terminalId = ptyIdRef.current || externalTerminalId
      if (terminalId && terminalRef.current) {
        captureScrollPosition(terminalId)
        // Always release the renderer-side gate; the helper is idempotent, so
        // this is a no-op when it was never claimed. The HOST-side reference is
        // still only removed when it was actually added.
        releaseRendererGate(terminalId)
        if (!externalTerminalId || rendererRefAttachedRef.current) {
          void removeRendererRef(terminalId, instanceId)
        }
        rendererRefAttachedRef.current = false
      }

      // Unregister terminal from registry
      if (ptyIdRef.current) {
        unregisterTerminal(ptyIdRef.current)
      } else if (externalTerminalId) {
        unregisterTerminal(externalTerminalId)
      }

      pasteContainer?.removeEventListener('paste', handlePasteCapture, { capture: true })

      // PTY lifecycle is handled by explicit terminal close, not component unmount
      pixelScroll.dispose()
      pixelScrollRef.current = null
      scrollDisposable.dispose()
      webglScrollRepairRef.current?.dispose()
      webglScrollRepairRef.current = null
      dataDisposable.dispose()
      if (cleanupDataListenerRef.current) {
        cleanupDataListenerRef.current()
        cleanupDataListenerRef.current = null
      }
      if (cleanupExitListenerRef.current) {
        cleanupExitListenerRef.current()
        cleanupExitListenerRef.current = null
      }

      clearTerminalActivityOnUnmount()
      // Cursor cleanup: Disable cursor blink before WebGL disposal to prevent ghost cursors
      if (terminalRef.current) {
        terminalRef.current.options.cursorBlink = false
      }

      // Dispose WebGL addon BEFORE terminal disposal for proper cursor layer cleanup
      disposeWebglAddon()
      if (fileLinkProviderDisposableRef.current) {
        fileLinkProviderDisposableRef.current.dispose()
        fileLinkProviderDisposableRef.current = null
      }

      // Cache the terminal for reuse on project-switch-back instead of
      // disposing it. This preserves all xterm internal state (scrollback,
      // alt buffer, cursor position). Only cache if the terminal is still
      // alive in the store (not closed/exited) — otherwise dispose.
      const cacheKey = terminalId
      const terminalStillInStore =
        cacheKey && useTerminalStore.getState().findTerminalByPtyId(cacheKey)
      if (terminalStillInStore) {
        cacheTerminal(cacheKey, { terminal, fitAddon, searchAddon })
      } else {
        terminal.dispose()
      }
      terminalRef.current = null
      setTerminalInstance(null)
      fitAddonRef.current = null
      searchAddonRef.current = null
      ptyIdRef.current = null
      spawnInFlightRef.current = false
      // Reset init flag so a new terminal can be created if component remounts
      didInitRef.current = false
      initializedTerminalIdRef.current = undefined
      // Reset WebGL recovery state for next terminal creation
      webglRecoveryAttemptsRef.current = 0
      webglContextLostRef.current = false
      loadWebglAddonRef.current = null
    }
  }, [])

  // Update terminal font settings when app settings change (without recreating terminal)
  useEffect(() => {
    if (terminalRef.current) {
      terminalRef.current.options.fontFamily = fontFamily
      terminalRef.current.options.fontSize = fontSize
      forceResizeFit()
    }
  }, [fontFamily, fontSize, forceResizeFit])

  useEffect(() => {
    if (terminalRef.current) {
      terminalRef.current.options.screenReaderMode = screenReaderMode
    }
  }, [screenReaderMode])

  useEffect(() => {
    if (!shouldUseWebglRenderer(rendererPreference)) {
      disposeWebglAddon()
      webglRecoveryAttemptsRef.current = 0
      return
    }

    if (
      isVisibleRef.current &&
      terminalRef.current &&
      loadWebglAddonRef.current &&
      !webglAddonRef.current
    ) {
      webglRecoveryAttemptsRef.current = 0
      loadWebglAddonRef.current(terminalRef.current)
    }
  }, [disposeWebglAddon, rendererPreference])

  // Hidden tabs keep their xterm buffer and PTY attachment, but release the
  // scarce WebGL context. The DOM renderer continues maintaining terminal
  // state while hidden; WebGL is restored when the tab becomes visible.
  useEffect(() => {
    if (!isVisible) {
      // Snapshot where the user was reading BEFORE the renderer churn below.
      // Disposing the WebGL addon and re-fitting on the way back both leave
      // xterm's RenderService reporting stale dimensions for a moment, and
      // xterm 6's Viewport feeds those straight into `setScrollDimensions`,
      // which clamps the scrollable into `[0, scrollHeight - height]`. A clamp
      // against a zero-ish height pins the position to 0, and the clamp's own
      // scroll event arrives on a later animation frame — outside xterm's
      // `_suppressOnScrollHandler` window — so `_handleScroll` reads scrollTop 0
      // and drives `ydisp` to the first line of scrollback. The buffer is
      // intact; the user is just teleported to the top. We cannot stop the
      // clamp, so we re-assert the position once the show sequence settles.
      const scrollKey = ptyIdRef.current || externalTerminalIdRef.current
      if (scrollKey) captureScrollPosition(scrollKey)
      needsSurfaceRestoreRef.current = true
      disposeWebglAddon()
      pixelScrollRef.current?.setEnabled(false)
      return
    }

    pixelScrollRef.current?.setEnabled(true)
    if (
      shouldUseWebglRenderer(rendererPreferenceRef.current) &&
      terminalRef.current &&
      loadWebglAddonRef.current &&
      !webglAddonRef.current
    ) {
      webglRecoveryAttemptsRef.current = 0
      loadWebglAddonRef.current(terminalRef.current)
    }
  }, [disposeWebglAddon, isVisible])

  // Trigger fit + PTY resize when terminal becomes visible
  // Uses the two-stage resize pipeline via forceResizeFit,
  // which skips both debounces for immediate responsiveness.
  useEffect(() => {
    if (isVisible && fitAddonRef.current && terminalRef.current) {
      // Double RAF ensures DOM is fully rendered after pane transition
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          // Use forceResizeFit for immediate fit + PTY resize
          // This bypasses both debounce stages for visibility changes
          forceResizeFit()

          const terminal = terminalRef.current
          if (!terminal) return

          // Only focus if no interactive element (button, input, etc.) currently has focus.
          // This prevents stealing focus from TitleBar window controls when tab switch happens.
          const active = document.activeElement
          const isInteractiveElementFocused =
            active &&
            active !== document.body &&
            (active.tagName === 'BUTTON' ||
              active.tagName === 'INPUT' ||
              active.tagName === 'TEXTAREA' ||
              active.tagName === 'SELECT' ||
              active.tagName === 'A')
          if (!isInteractiveElementFocused) {
            terminal.focus()
          }

          if (needsSurfaceRestoreRef.current) {
            needsSurfaceRestoreRef.current = false
            restoreVisibleTerminalSurface({
              resetPixelOffset: () => pixelScrollRef.current?.reset(),
              repair: webglScrollRepairRef.current,
              terminal
            })
          }

          // Last, not before the surface repair: `restoreVisibleTerminalSurface`
          // repaints the terminal, which is itself a chance for the viewport to
          // be re-clamped. Asserting the position ahead of it would just be
          // overwritten.
          const ptyId = ptyIdRef.current
          if (ptyId) {
            restoreScrollPosition(ptyId, terminal)
          } else {
            // PTY not ready yet — defer resize until spawn completes
            needsResizeOnReadyRef.current = true
          }
        })
      })
    }
  }, [isVisible, forceResizeFit])

  // Shared terminal recovery logic - re-fit once layout is stable, then nudge
  // the compositor to re-present the canvas layer.
  const performTerminalRecovery = useCallback((): void => {
    if (!fitAddonRef.current || !terminalRef.current) return

    // Single-flight: if a recovery is already running (layout-wait poll or the
    // trailing visibility-flip RAF), skip duplicate triggers. On a window
    // restore both visibilitychange and focus typically fire close together.
    if (recoveryInProgressRef.current) return
    recoveryInProgressRef.current = true

    // Cancel any pending WebGL auto-recovery timeout to avoid double-creation
    // race with the genuine onContextLoss path.
    if (webglRecoveryTimeoutRef.current) {
      clearTimeout(webglRecoveryTimeoutRef.current)
      webglRecoveryTimeoutRef.current = null
    }

    // Root cause (verified via live forensics + xterm.js #4841 / #5357):
    //
    // After minimize→restore on Windows the webview reflows over several
    // frames. If fit() runs while the container height is still collapsed,
    // the terminal grid shrinks to 1-2 rows (PTY redraws tiny → "1-2 lines"
    // of text) until a later resize corrects it. The fit pipeline now guards
    // against collapsed dimensions (use-terminal-resize-v2), so an early fit
    // is a safe no-op rather than a destructive shrink.
    //
    // Additionally, the WebView2 compositor may not re-present the WebGL
    // canvas layer after restore (xterm 6.x has no DOM-row fallback; the
    // context itself stays healthy). A CSS visibility flip forces a
    // re-composite — the same mechanism that makes tab-switching work.
    //
    // Strategy: wait for the container to report a usable size (poll across a
    // few RAFs), then forceResizeFit + refresh, then flip visibility to
    // guarantee the layer re-composites.
    const termEl = terminalRef.current.element as HTMLElement | undefined
    const container = containerRef.current

    const MIN_USABLE = 40
    const MAX_LAYOUT_WAIT_FRAMES = 30 // ~0.5s at 60fps

    const runRecovery = (): void => {
      const terminal = terminalRef.current
      if (!terminal) {
        recoveryInProgressRef.current = false
        return
      }
      // Re-fit (guarded against collapsed dims) + rebuild the surface. A bare
      // refresh only redraws from the render model, and nothing recreates the
      // WebGL addon on this path — the isVisible-driven effect that does never
      // runs, because the tab stayed active the whole time the window was
      // away. So the model can still hold the pre-minimize frame and a refresh
      // faithfully redraws it.
      //
      // Safe here where it would not be inside a write callback: this runs
      // from a RAF, and `repairNow` itself never resizes (D-2).
      forceResizeFit()
      restoreVisibleTerminalSurface({ repair: webglScrollRepairRef.current, terminal })

      // Nudge the compositor to re-present the canvas layer. Clear the
      // single-flight guard only after the trailing refresh completes.
      if (termEl) {
        termEl.style.visibility = 'hidden'
        requestAnimationFrame(() => {
          termEl.style.visibility = ''
          const t = terminalRef.current
          if (t) {
            restoreVisibleTerminalSurface({ repair: webglScrollRepairRef.current, terminal: t })
          }
          recoveryInProgressRef.current = false
        })
      } else {
        recoveryInProgressRef.current = false
      }
    }

    // Wait until the container has reflowed to a usable size before fitting,
    // so we never collapse the grid. Bail out after MAX_LAYOUT_WAIT_FRAMES.
    let frames = 0
    const waitForStableLayout = (): void => {
      const rect = container?.getBoundingClientRect()
      const ready = !!rect && rect.width >= MIN_USABLE && rect.height >= MIN_USABLE
      if (ready || frames >= MAX_LAYOUT_WAIT_FRAMES) {
        runRecovery()
        return
      }
      frames += 1
      requestAnimationFrame(waitForStableLayout)
    }
    waitForStableLayout()
  }, [forceResizeFit])

  // Recovery handler for visibility change (app regains focus after idle)
  useEffect(() => {
    // Track timeout to prevent firing after unmount
    let recoveryTimeoutId: ReturnType<typeof setTimeout> | null = null

    const handleVisibilityChange = (): void => {
      if (document.visibilityState === 'visible' && isVisibleRef.current) {
        // Clear any pending timeout before scheduling new one
        if (recoveryTimeoutId) {
          clearTimeout(recoveryTimeoutId)
        }
        recoveryTimeoutId = setTimeout(() => {
          recoveryTimeoutId = null
          performTerminalRecovery()
        }, VISIBILITY_RECOVERY_DELAY_MS)
      }
    }

    document.addEventListener('visibilitychange', handleVisibilityChange)
    return () => {
      if (recoveryTimeoutId) {
        clearTimeout(recoveryTimeoutId)
      }
      document.removeEventListener('visibilitychange', handleVisibilityChange)
    }
  }, [performTerminalRecovery])

  // Recovery handler for window focus — critical for Tauri minimize/restore
  // on Windows where document.visibilitychange is unreliable.
  // The window 'focus' event reliably fires when the window is restored from
  // taskbar minimize. performTerminalRecovery re-fits the terminal to its
  // container and syncs PTY dimensions (SIGWINCH to the shell process).
  useEffect(() => {
    const handleWindowFocus = (): void => {
      // Skip recovery for terminals that are not the active tab in their pane
      // (isVisible is tab-active, not window-visible — see PaneContent.tsx).
      // Hidden instances recover via the isVisible-change useEffect instead.
      if (!isVisibleRef.current) return
      // Fire recovery immediately — the window is already visible when
      // 'focus' fires (unlike visibilitychange which needs DOM reflow time).
      // performTerminalRecovery internally waits for a stable layout before
      // fitting and is single-flight guarded, so this is safe to call eagerly.
      performTerminalRecovery()
    }

    window.addEventListener('focus', handleWindowFocus)
    return () => {
      window.removeEventListener('focus', handleWindowFocus)
    }
  }, [performTerminalRecovery])

  // Recovery handler for power resume (wake from sleep, screen unlock)
  useEffect(() => {
    // Track timeout to prevent firing after unmount
    let recoveryTimeoutId: ReturnType<typeof setTimeout> | null = null

    const cleanup = systemApi.onPowerResume(() => {
      if (!isVisibleRef.current) return
      // Clear any pending timeout before scheduling new one
      if (recoveryTimeoutId) {
        clearTimeout(recoveryTimeoutId)
      }
      recoveryTimeoutId = setTimeout(() => {
        recoveryTimeoutId = null
        performTerminalRecovery()
      }, POWER_RESUME_RECOVERY_DELAY_MS)
    })
    return () => {
      if (recoveryTimeoutId) {
        clearTimeout(recoveryTimeoutId)
      }
      cleanup()
    }
  }, [performTerminalRecovery])

  const handleContainerClick = useCallback((): void => {
    terminalRef.current?.focus()
  }, [])

  const handleSelectAll = useCallback((): void => {
    terminalRef.current?.selectAll()
  }, [])

  useImperativeHandle(searchRef, () => {
    const searchDecorations = {
      matchBackground: '#444444',
      activeMatchBackground: '#FFFF00',
      matchOverviewRuler: '#444444',
      activeMatchColorOverviewRuler: '#FFFF00'
    }

    return {
      findNext: (term: string) =>
        searchAddonRef.current?.findNext(term, { decorations: searchDecorations }) ?? false,
      findPrevious: (term: string) =>
        searchAddonRef.current?.findPrevious(term, { decorations: searchDecorations }) ?? false,
      clearDecorations: () => searchAddonRef.current?.clearDecorations(),
      writeText: (text: string) => {
        if (ptyIdRef.current) terminalApi.write(ptyIdRef.current, text)
      }
    }
  }, [])

  const shouldDebugLog = import.meta.env.DEV
  const devLog = (...args: unknown[]): void => {
    if (shouldDebugLog) console.log(...args)
  }

  const isCrashed = healthStatus === 'crashed'
  const cleanupStageLabel = cleanupRecovery
    ? t(`cleanup.stages.${cleanupRecovery.cleanupStage}`)
    : ''

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div className="relative isolate h-full w-full overflow-hidden group">
          {/* biome-ignore lint/a11y/noStaticElementInteractions lint/a11y/useKeyWithClickEvents: xterm owns the focusable textarea; this wrapper only forwards pointer focus. */}
          <div
            className={`w-full h-full bg-terminal-bg px-4 py-0.5 pb-1 ${className}`}
            onClick={handleContainerClick}
            onMouseDown={(e) => {
              // Prevent event from bubbling to window/parent handlers
              // that might steal focus back or interfere with UI
              e.stopPropagation()
              if (terminalRef.current) {
                terminalRef.current.focus()
              }
            }}
          >
            <div ref={containerRef} className="w-full h-full" />
          </div>
          {parkedByPhone && !isMobileWebShell ? (
            <div
              className="absolute inset-0 z-40 flex items-center justify-center bg-background/80 p-4 backdrop-blur-[2px]"
              role="status"
              aria-live="polite"
            >
              <div className="w-full max-w-sm rounded-md border border-border bg-card p-4 text-foreground shadow-lg">
                <h3 className="text-sm font-semibold tracking-[-0.01em]">
                  {t('parkedByPhone.title')}
                </h3>
                <p className="mt-1.5 text-xs leading-relaxed text-muted-foreground">
                  {t('parkedByPhone.description')}
                </p>
                <button
                  type="button"
                  className="mt-3 inline-flex h-8 items-center justify-center rounded-md bg-foreground px-3 text-xs font-medium text-background transition-colors hover:bg-foreground/88"
                  onClick={(event) => {
                    event.stopPropagation()
                    const ptyId = ptyIdRef.current
                    if (!ptyId || !terminalApi.setDisplayMode) return
                    void terminalApi.setDisplayMode(ptyId, 'desktop', { force: true })
                  }}
                >
                  {t('parkedByPhone.resume')}
                </button>
              </div>
            </div>
          ) : null}
          {cleanupRecovery && (
            <div className="absolute inset-x-3 top-3 z-[60]" role="alert" aria-live="polite">
              <div className="flex items-start gap-3 rounded-md border border-destructive/35 bg-card p-3 text-foreground shadow-[0_12px_36px_hsl(var(--background)/0.55),inset_0_1px_0_0_hsl(var(--foreground)/0.05)]">
                <AlertTriangle className="mt-0.5 shrink-0 text-destructive" size={16} />
                <div className="min-w-0 flex-1">
                  <h3 className="text-xs font-semibold tracking-[-0.01em]">{t('cleanup.title')}</h3>
                  <p className="mt-1 text-2xs text-muted-foreground">
                    {t('cleanup.description', {
                      terminalId: cleanupRecovery.terminalId,
                      stage: cleanupStageLabel
                    })}
                  </p>
                  {cleanupRecovery.retryFailed && (
                    <p className="mt-1.5 text-2xs text-destructive">{t('cleanup.retryFailed')}</p>
                  )}
                </div>
                <button
                  type="button"
                  disabled={cleanupRecovery.retrying}
                  aria-label={t('cleanup.retryTerminationFor', {
                    terminalId: cleanupRecovery.terminalId
                  })}
                  onClick={(event) => {
                    event.stopPropagation()
                    void retryTerminalCleanup(cleanupRecovery.terminalId)
                  }}
                  className="inline-flex h-7 shrink-0 items-center gap-1.5 rounded-md bg-destructive px-2.5 text-2xs font-medium text-destructive-foreground transition-colors hover:bg-destructive/90 focus:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-wait disabled:opacity-60"
                >
                  <RefreshCcw
                    size={13}
                    className={cleanupRecovery.retrying ? 'animate-spin' : undefined}
                  />
                  {cleanupRecovery.retrying
                    ? t('cleanup.retryInProgress')
                    : t('cleanup.retryTermination')}
                </button>
              </div>
            </div>
          )}
          {isCrashed && !cleanupRecovery && (
            <div className="absolute inset-0 z-50 flex animate-in items-center justify-center bg-background/78 p-4 text-foreground fade-in duration-150">
              <div className="w-full max-w-md rounded-md border border-destructive/35 bg-card p-4 shadow-[0_18px_60px_hsl(var(--background)/0.7),inset_0_1px_0_0_hsl(var(--foreground)/0.05)]">
                <div className="flex items-start gap-3">
                  <div className="flex size-8 shrink-0 items-center justify-center rounded-md bg-destructive/10">
                    <AlertTriangle className="text-destructive" size={16} />
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="mb-1 text-2xs font-medium text-destructive">
                      {t('crash.criticalError')} · {t('crash.paneException')}
                    </div>
                    <h3 className="text-sm font-semibold tracking-[-0.01em]">
                      {t('crash.sessionInterrupted')}
                    </h3>
                    <p className="mt-1.5 text-xs leading-relaxed text-muted-foreground">
                      {t('crash.description')}
                    </p>
                  </div>
                </div>
                <div className="mt-4 flex items-center gap-3">
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation()
                      if (targetId) void restartTerminalResource(targetId)
                    }}
                    className="inline-flex h-8 items-center justify-center gap-1.5 rounded-md bg-foreground px-3 text-xs font-medium text-background transition-colors hover:bg-foreground/88 focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                  >
                    <RefreshCcw size={14} /> {t('crash.reconnect')}
                  </button>
                  <div className="font-mono text-3xs text-muted-foreground/60">
                    REF::{targetId?.slice(0, 8)}
                  </div>
                </div>
              </div>
            </div>
          )}
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent className="w-40">
        <ContextMenuItem
          onSelect={copySelection}
          disabled={!hasSelection}
          className="cursor-pointer"
        >
          {t('contextMenu.copy')} <ContextMenuShortcut>{SHORTCUT_MOD}+C</ContextMenuShortcut>
        </ContextMenuItem>
        <ContextMenuItem onSelect={pasteFromClipboard} className="cursor-pointer">
          {t('contextMenu.paste')} <ContextMenuShortcut>{SHORTCUT_MOD}+V</ContextMenuShortcut>
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={handleSelectAll} className="cursor-pointer">
          {t('contextMenu.selectAll')} <ContextMenuShortcut>{SHORTCUT_MOD}+A</ContextMenuShortcut>
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem
          onSelect={() => {
            if (targetId) void restartTerminalResource(targetId)
          }}
          className="cursor-pointer text-primary focus:text-primary"
        >
          <RefreshCcw size={14} className="mr-2" /> {t('contextMenu.restart')}
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  )
}

export const ConnectedTerminal = memo(ConnectedTerminalComponent)
