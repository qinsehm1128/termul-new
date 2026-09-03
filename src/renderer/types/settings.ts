import { acceptedBrandValues, brandCanonical } from '@shared/brand'

// Context bar visibility settings
export interface ContextBarSettings {
  showGitBranch: boolean
  showGitStatus: boolean
  showWorkingDirectory: boolean
  showExitCode: boolean
}

// Default settings with all elements visible
export const DEFAULT_CONTEXT_BAR_SETTINGS: ContextBarSettings = {
  showGitBranch: true,
  showGitStatus: true,
  showWorkingDirectory: true,
  showExitCode: true
}

// Persistence key for context bar settings
export const CONTEXT_BAR_SETTINGS_KEY = 'settings/context-bar'

// Table of contents panel settings
export interface TocSettings {
  isVisible: boolean
  maxHeadingLevel: number
  width: number
}

export const TOC_MIN_WIDTH = 150
export const TOC_MAX_WIDTH = 350

export const DEFAULT_TOC_SETTINGS: TocSettings = {
  isVisible: true,
  maxHeadingLevel: 3,
  width: 220
}

export const TOC_SETTINGS_KEY = 'settings/toc'

export type TerminalUrlOpenMode = 'system' | 'se'
export type UiLanguage = 'en' | 'zh-CN'
export type UiLanguagePreference = 'system' | UiLanguage

export function isUiLanguagePreference(value: unknown): value is UiLanguagePreference {
  return value === 'system' || value === 'en' || value === 'zh-CN'
}

/** Which interface the remote terminal HTTP server binds to when started. */
export type RemoteBindMode = 'localhost' | 'all'

// Application-wide settings
export interface AppSettings {
  terminalFontFamily: string
  /** Symbol/glyph font for the terminal: '' = auto Nerd Font fallback chain, 'none' = disabled. */
  terminalSymbolFontFamily: string
  terminalFontSize: number
  terminalBufferSize: number // Scrollback buffer size in lines
  terminalRenderer: 'auto' | 'webgl' | 'dom'
  /** Expose xterm's accessibility tree for NVDA/VoiceOver. Off by default for throughput. */
  terminalScreenReaderMode: boolean
  defaultShell: string
  defaultProjectColor: string // Default color for new projects (from PROJECT_COLORS)
  maxTerminalsPerProject: number // Maximum terminals allowed per project
  orphanDetectionEnabled: boolean // Enable automatic cleanup of inactive terminals
  orphanDetectionTimeout: number | null // Timeout in ms, null = disabled
  confirmTerminalClose: boolean // Show a confirmation dialog before closing a terminal
  terminalUrlOpenMode: TerminalUrlOpenMode // Controls how Ctrl/Cmd+Click terminal URLs are opened
  sidebarVisible: boolean
  fileExplorerVisible: boolean
  sshPanelVisible: boolean
  cliSessionPanelVisible: boolean
  terminalListPanelVisible: boolean
  /** Remote server bind: localhost (127.0.0.1) or all interfaces (0.0.0.0). */
  remoteBindMode: RemoteBindMode
  /** App-wide color theme family id (without `-light` suffix). */
  colorTheme: string
  /** Light, dark, or follow OS (maps to `{colorTheme}` / `{colorTheme}-light`). */
  appearanceMode: 'light' | 'dark'
  /**
   * Terminal color theme, independent of the UI theme.
   *
   * `null` means "follow the UI theme" and is the default — nullable rather
   * than a concrete id on purpose: a concrete default would pin every terminal
   * to one theme the moment the user changed the UI theme, which is the exact
   * behaviour this setting exists to make opt-in.
   */
  terminalColorTheme: string | null
  /** Whole-UI zoom factor (1.0 = 100%). Scales the entire window like VS Code's window zoom. */
  uiZoomLevel: number
  /** User-selected interface language, or follow the operating system. */
  uiLanguage: UiLanguagePreference
  /** ACP turn hard-cap timeout in seconds, or null = use the env var / Rust
   * default (unlimited by default). Set via App Preferences; pushed to the
   * Rust core. */
  acpTurnTimeoutSecs: number | null
  /** Automatically save dirty editor files after edits pause (GH-539). */
  editorAutoSave: boolean
  /** Idle delay in ms before a dirty editor file is auto-saved (GH-539). */
  editorAutoSaveDelayMs: number
  /** ACP per-turn idle timeout in seconds, or null = use the env var / Rust
   * default (unlimited by default). Set via App Preferences; pushed to the
   * Rust core. */
  acpTurnIdleTimeoutSecs: number | null
  /** ACP session/new timeout in seconds, or null = use the env var / Rust
   * default (60s). Set via App Preferences; pushed to the Rust core. */
  acpSessionNewTimeoutSecs: number | null
  /** ACP session reopen (load/resume) timeout in seconds, or null = use the
   * env var / Rust default (60s). Set via App Preferences; pushed to the
   * Rust core. */
  acpSessionReopenTimeoutSecs: number | null
  /** ACP first-prompt warmup timeout in seconds, or null = use the env var /
   * Rust default (45s); 0 disables the warmup entirely. Set via App
   * Preferences; pushed to the Rust core. */
  acpFirstPromptWarmupSecs: number | null
  /** When true (default), first `npx -y` agent launch installs the package
   * into Se's local prefix and later launches skip npx. When false,
   * always run through npx. Pushed to the Rust core. */
  acpPreferLocalNpmInstall: boolean
}

/** Whole-UI zoom bounds â€” match the native View menu semantics (0.5xâ€“3.0x, 10% steps). */
export const UI_ZOOM_DEFAULT = 1.0
export const UI_ZOOM_MIN = 0.5
export const UI_ZOOM_MAX = 3.0
export const UI_ZOOM_STEP = 0.1

export type AppPanelVisibilitySettingKey =
  | 'sidebarVisible'
  | 'fileExplorerVisible'
  | 'sshPanelVisible'
  | 'cliSessionPanelVisible'
  | 'terminalListPanelVisible'

export type AppSettingsUpdate = Partial<Omit<AppSettings, AppPanelVisibilitySettingKey>>

// Terminal buffer size options
export const BUFFER_SIZE_OPTIONS = [
  { value: 1000, label: '1,000 lines' },
  { value: 5000, label: '5,000 lines' },
  { value: 10000, label: '10,000 lines' },
  { value: 25000, label: '25,000 lines' },
  { value: 50000, label: '50,000 lines' }
]

// Font family options for terminal
export const FONT_FAMILY_OPTIONS = [
  { value: 'Menlo, Monaco, "Courier New", monospace', label: 'Menlo' },
  { value: 'Monaco, Menlo, "Courier New", monospace', label: 'Monaco' },
  { value: 'Consolas, "Courier New", monospace', label: 'Consolas' },
  { value: '"Courier New", Courier, monospace', label: 'Courier New' },
  { value: '"Source Code Pro", Menlo, monospace', label: 'Source Code Pro' },
  { value: '"JetBrains Mono", Menlo, monospace', label: 'JetBrains Mono' },
  { value: '"Fira Code", Menlo, monospace', label: 'Fira Code' },
  {
    value: '"MesloLGLDZ Nerd Font Mono", Menlo, monospace',
    label: 'MesloLGLDZ Nerd Font Mono'
  }
]

// Max terminals per project options
export const MAX_TERMINALS_OPTIONS = [
  { value: 5, label: '5 terminals' },
  { value: 10, label: '10 terminals' },
  { value: 15, label: '15 terminals' },
  { value: 20, label: '20 terminals' },
  { value: 50, label: '50 terminals' }
]

// Orphan detection timeout options
export const ORPHAN_TIMEOUT_OPTIONS = [
  { value: 60000, label: '1 minute' },
  { value: 300000, label: '5 minutes' },
  { value: 600000, label: '10 minutes' },
  { value: 1800000, label: '30 minutes' },
  { value: 3600000, label: '1 hour' }
]

// Editor auto-save delay options (GH-539)
export const EDITOR_AUTO_SAVE_DELAY_OPTIONS = [
  { value: 500, label: '0.5 seconds' },
  { value: 1000, label: '1 second' },
  { value: 2000, label: '2 seconds' },
  { value: 5000, label: '5 seconds' }
]

// Terminal renderer strategy options
export const TERMINAL_RENDERER_OPTIONS = [
  { value: 'auto', label: 'Auto (Prefer WebGL, DOM fallback)' },
  { value: 'webgl', label: 'WebGL' },
  { value: 'dom', label: 'DOM' }
] as const

// Terminal URL opening mode options
export const TERMINAL_URL_OPEN_MODE_OPTIONS: Array<{
  value: TerminalUrlOpenMode
  label: string
}> = [
  { value: 'system', label: 'System Default Browser' },
  { value: 'se', label: 'Se Browser' }
]

/**
 * The canonical spelling of a persisted `terminalUrlOpenMode`.
 *
 * A settings blob written before the rename still names the built-in browser
 * by its legacy enum member, and that member is deliberately absent from
 * {@link TERMINAL_URL_OPEN_MODE_OPTIONS} — nothing may write it any more. A
 * `<select>` bound straight to the persisted value would therefore match no
 * `<option>` and silently display the first entry ("System Default Browser"),
 * while `openTerminalUrl`'s compatibility read still opens links in the
 * built-in one. The settings UI would state the opposite of what the app does.
 *
 * The membership test is the same `acceptedBrandValues('urlOpenMode')` that
 * `openTerminalUrl` branches on, so display and behaviour cannot disagree.
 * Normalizing for display never writes: the persisted spelling stays on disk
 * until the user picks something, and what they pick is always canonical.
 */
export function normalizeTerminalUrlOpenMode(value: string): TerminalUrlOpenMode {
  return acceptedBrandValues('urlOpenMode').includes(value) ? 'se' : 'system'
}

export const REMOTE_BIND_MODE_OPTIONS: Array<{
  value: RemoteBindMode
  label: string
  description: string
}> = [
  {
    value: 'localhost',
    label: 'Localhost only (127.0.0.1)',
    description: 'Only this machine can connect directly. Safest default.'
  },
  {
    value: 'all',
    label: 'All interfaces (0.0.0.0)',
    description: 'Listen on every network interface. Other devices on your LAN can reach the port.'
  }
]

// ACP turn (hard-cap) timeout options for the App Preferences UI. `null` =
// follow the env var / Rust default (unlimited); a number is the override in
// seconds.
export const ACP_TURN_TIMEOUT_OPTIONS: Array<{
  value: number | null
  label: string
}> = [
  { value: null, label: 'Environment/default (unlimited)' },
  { value: 3600, label: '1 hour' },
  { value: 7200, label: '2 hours' },
  { value: 21600, label: '6 hours' },
  { value: 86400, label: '24 hours' },
  { value: 31536000, label: '1 year' }
]

// ACP turn idle-timeout options (the silent-turn window). `null` = follow the
// env var / Rust default (unlimited); a number is the override in seconds.
export const ACP_TURN_IDLE_TIMEOUT_OPTIONS: Array<{
  value: number | null
  label: string
}> = [
  { value: null, label: 'Environment/default (unlimited)' },
  { value: 300, label: '5 minutes' },
  { value: 600, label: '10 minutes' },
  { value: 900, label: '15 minutes' },
  { value: 1800, label: '30 minutes' },
  { value: 3600, label: '1 hour' }
]

// ACP session/new timeout options. `null` = follow the env var / Rust default
// (60 seconds); a number is the override in seconds.
export const ACP_SESSION_NEW_TIMEOUT_OPTIONS: Array<{
  value: number | null
  label: string
}> = [
  { value: null, label: 'Environment/default (60 seconds)' },
  { value: 30, label: '30 seconds' },
  { value: 60, label: '1 minute' },
  { value: 120, label: '2 minutes' },
  { value: 300, label: '5 minutes' }
]

// ACP session reopen (load/resume) timeout options. `null` = follow the env
// var / Rust default (60 seconds); a number is the override in seconds.
export const ACP_SESSION_REOPEN_TIMEOUT_OPTIONS: Array<{
  value: number | null
  label: string
}> = [
  { value: null, label: 'Environment/default (60 seconds)' },
  { value: 30, label: '30 seconds' },
  { value: 60, label: '1 minute' },
  { value: 120, label: '2 minutes' },
  { value: 300, label: '5 minutes' }
]

// ACP first-prompt warmup timeout options. `null` = follow the env var / Rust
// default (45 seconds); 0 disables the warmup entirely; a positive number is
// the override in seconds.
export const ACP_FIRST_PROMPT_WARMUP_OPTIONS: Array<{
  value: number | null
  label: string
}> = [
  { value: null, label: 'Environment/default (45 seconds)' },
  { value: 0, label: 'Disabled' },
  { value: 15, label: '15 seconds' },
  { value: 45, label: '45 seconds' },
  { value: 120, label: '2 minutes' }
]

// Default application settings
// Symbol font choices offered in App Preferences; values are CSS family lists.
export const SYMBOL_FONT_OPTIONS = [
  { value: '', label: 'Auto (Nerd Font fallback)' },
  { value: 'none', label: 'None' },
  {
    value: '"MesloLGLDZ Nerd Font Mono", "MesloLGLDZ Nerd Font"',
    label: 'MesloLGLDZ Nerd Font'
  },
  { value: '"MesloLGS NF", "MesloLGL NF", "MesloLGM NF"', label: 'MesloLGS NF' },
  {
    value: '"JetBrainsMono Nerd Font", "JetBrainsMono Nerd Font Mono"',
    label: 'JetBrainsMono Nerd Font'
  },
  { value: '"FiraCode Nerd Font", "FiraCode Nerd Font Mono"', label: 'FiraCode Nerd Font' },
  { value: '"Hack Nerd Font", "Hack Nerd Font Mono"', label: 'Hack Nerd Font' },
  { value: '"Symbols Nerd Font", "Symbols Nerd Font Mono"', label: 'Symbols Nerd Font' }
]

export const DEFAULT_APP_SETTINGS: AppSettings = {
  terminalFontFamily: 'Menlo, Monaco, "Courier New", monospace',
  terminalSymbolFontFamily: '',
  terminalFontSize: 14,
  terminalBufferSize: 10000,
  terminalRenderer: 'webgl',
  terminalScreenReaderMode: false,
  defaultShell: '',
  defaultProjectColor: 'blue',
  maxTerminalsPerProject: 10,
  orphanDetectionEnabled: true,
  orphanDetectionTimeout: 600000, // 10 minutes
  confirmTerminalClose: true,
  terminalUrlOpenMode: 'system',
  sidebarVisible: true,
  fileExplorerVisible: true,
  sshPanelVisible: true,
  cliSessionPanelVisible: false,
  terminalListPanelVisible: false,
  remoteBindMode: 'localhost',
  colorTheme: brandCanonical().themeId,
  appearanceMode: 'dark',
  terminalColorTheme: null,
  uiZoomLevel: UI_ZOOM_DEFAULT,
  uiLanguage: 'system',
  acpTurnTimeoutSecs: null,
  editorAutoSave: false,
  editorAutoSaveDelayMs: 1000,
  acpTurnIdleTimeoutSecs: null,
  acpSessionNewTimeoutSecs: null,
  acpSessionReopenTimeoutSecs: null,
  acpFirstPromptWarmupSecs: null,
  acpPreferLocalNpmInstall: true
}

// Persistence key for app settings
export const APP_SETTINGS_KEY = 'settings/app'

// Keyboard shortcut definition
export interface KeyboardShortcut {
  id: string
  label: string
  description: string
  defaultKey: string // Normalized format: "ctrl+k", "ctrl+shift+p"
  customKey?: string // User's custom binding, undefined = use default
}

// All keyboard shortcuts configuration
export type KeyboardShortcutsConfig = Record<string, KeyboardShortcut>

// Default keyboard shortcuts matching current WorkspaceDashboard handlers
export const DEFAULT_KEYBOARD_SHORTCUTS: KeyboardShortcutsConfig = {
  commandPalette: {
    id: 'commandPalette',
    label: 'Command Palette',
    description: 'Open the command palette for quick actions',
    defaultKey: 'ctrl+k'
  },
  commandPaletteAlt: {
    id: 'commandPaletteAlt',
    label: 'Command Palette (Alt)',
    description: 'Open command palette (VS Code style)',
    defaultKey: 'ctrl+shift+p'
  },
  terminalSearch: {
    id: 'terminalSearch',
    label: 'Terminal Search',
    description: 'Search within terminal output',
    defaultKey: 'ctrl+f'
  },
  commandHistory: {
    id: 'commandHistory',
    label: 'Command History',
    description: 'Search command history',
    defaultKey: 'ctrl+r'
  },
  newProject: {
    id: 'newProject',
    label: 'New Project',
    description: 'Create a new project',
    defaultKey: 'ctrl+n'
  },
  newTerminal: {
    id: 'newTerminal',
    label: 'Agent Launcher',
    description: 'Show the agent launcher prompt in the active pane',
    defaultKey: 'ctrl+t'
  },
  newBrowserTab: {
    id: 'newBrowserTab',
    label: 'New Browser Tab',
    description: 'Create a new browser tab',
    defaultKey: 'ctrl+shift+n'
  },
  nextTerminal: {
    id: 'nextTerminal',
    label: 'Next Tab',
    description: 'Switch to next tab (terminal or editor) using the Tauri-safe fallback',
    defaultKey: 'ctrl+pagedown'
  },
  prevTerminal: {
    id: 'prevTerminal',
    label: 'Previous Tab',
    description: 'Switch to previous tab (terminal or editor) using the Tauri-safe fallback',
    defaultKey: 'ctrl+pageup'
  },
  terminalSwitcher: {
    id: 'terminalSwitcher',
    label: 'Switch Terminal',
    description: 'Search every terminal by name or project and jump to it',
    defaultKey: 'ctrl+shift+e'
  },
  lastTerminal: {
    id: 'lastTerminal',
    label: 'Last Terminal',
    description: 'Jump back to the previously active terminal, wherever it lives',
    // Not ctrl+tab: on macOS the store aliases ctrl+X config to ⌘+X, and ⌘⇧Tab
    // is the OS app switcher. ctrl+shift+l is free on both surfaces and leaves
    // the shell's own Ctrl+L (clear) untouched.
    defaultKey: 'ctrl+shift+l'
  },
  zoomIn: {
    id: 'zoomIn',
    label: 'Zoom In',
    description: 'Zoom in the entire UI',
    defaultKey: 'ctrl+='
  },
  zoomOut: {
    id: 'zoomOut',
    label: 'Zoom Out',
    description: 'Zoom out the entire UI',
    defaultKey: 'ctrl+-'
  },
  zoomReset: {
    id: 'zoomReset',
    label: 'Reset Zoom',
    description: 'Reset the whole-UI zoom to 100%',
    defaultKey: 'ctrl+0'
  },
  sidebarToggle: {
    id: 'sidebarToggle',
    label: 'Toggle Sidebar',
    description: 'Show or hide the project sidebar',
    defaultKey: 'ctrl+shift+b'
  },
  closeTab: {
    id: 'closeTab',
    label: 'Close Tab',
    description: 'Close the active tab (terminal, editor, or browser)',
    defaultKey: 'ctrl+w'
  },
  saveFile: {
    id: 'saveFile',
    label: 'Save File',
    description: 'Save the current editor file',
    defaultKey: 'ctrl+s'
  },
  toggleFileExplorer: {
    id: 'toggleFileExplorer',
    label: 'Toggle File Explorer',
    description: 'Show or hide the file explorer panel',
    defaultKey: 'ctrl+b'
  },
  toggleCliSessionPanel: {
    id: 'toggleCliSessionPanel',
    label: 'Toggle CLI Sessions',
    description: 'Show or hide the CLI session vault panel',
    defaultKey: 'ctrl+shift+h'
  },
  fileExplorerRename: {
    id: 'fileExplorerRename',
    label: 'Rename File',
    description: 'Rename selected file',
    defaultKey: 'f2'
  },
  fileExplorerDelete: {
    id: 'fileExplorerDelete',
    label: 'Delete Files',
    description: 'Delete selected files',
    defaultKey: 'delete'
  },

  // Worktree shortcuts
  worktreeCreate: {
    id: 'worktreeCreate',
    label: 'Create Worktree',
    description: 'Open the new worktree creation modal',
    defaultKey: 'ctrl+shift+alt+n'
  },
  worktreeSwitchNext: {
    id: 'worktreeSwitchNext',
    label: 'Switch to Next Worktree',
    description: 'Cycle to the next worktree in the sidebar',
    defaultKey: 'ctrl+shift+downarrow'
  },
  worktreeSwitchPrev: {
    id: 'worktreeSwitchPrev',
    label: 'Switch to Previous Worktree',
    description: 'Cycle to the previous worktree in the sidebar',
    defaultKey: 'ctrl+shift+uparrow'
  },
  worktreeOpenTerminal: {
    id: 'worktreeOpenTerminal',
    label: 'Open Terminal in Worktree',
    description: 'Spawn a new terminal in the active worktree',
    defaultKey: 'ctrl+shift+alt+t'
  },
  worktreeMergeToMain: {
    id: 'worktreeMergeToMain',
    label: 'Merge Worktree to Main',
    description: 'Start merge workflow: worktree branch to main',
    defaultKey: 'ctrl+shift+m'
  },
  worktreeSyncMain: {
    id: 'worktreeSyncMain',
    label: 'Sync Main into Worktree',
    description: 'Start merge workflow: main into worktree branch',
    defaultKey: 'ctrl+shift+alt+s'
  },
  worktreeArchive: {
    id: 'worktreeArchive',
    label: 'Archive Active Worktree',
    description: 'Archive the current active worktree',
    defaultKey: 'ctrl+shift+a'
  },
  worktreeSwitchRoot: {
    id: 'worktreeSwitchRoot',
    label: 'Switch to Project Root',
    description: 'Switch active context to the project root directory',
    defaultKey: 'ctrl+shift+home'
  },
  colorThemePicker: {
    id: 'colorThemePicker',
    label: 'Change Color Theme',
    description: 'Open the color theme picker with live preview',
    defaultKey: 'ctrl+alt+t'
  }
}

// Persistence key for keyboard shortcuts
export const KEYBOARD_SHORTCUTS_KEY = 'settings/keyboard-shortcuts'
