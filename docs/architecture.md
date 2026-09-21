# Se Manager - Architecture

**Date:** 2026-05-09
**Project Type:** Desktop Application
**Architecture Pattern:** Layered desktop application with renderer/runtime separation

## Executive Summary

Se Manager is a single-repository desktop application built around a **Tauri 2 + Rust runtime** and a **React 18 + TypeScript renderer**. The architecture emphasizes a clean boundary between UI concerns and native capabilities:

- the **renderer** owns user interaction, state, layout orchestration, editor/browser/terminal surfaces, and persistence adapters
- the **native runtime** owns PTY processes, browser child webviews, migration execution, shell detection, window integration, and OS-level operations

The app behaves like a workspace shell rather than a single-purpose terminal view. It combines project switching, pane-based layout management, editor tabs, browser tabs, annotations, snapshots, command history, and updater flows in one desktop window.

## Technology Stack

| Category | Technology | Notes |
| --- | --- | --- |
| Desktop framework | Tauri 2 | Windowing, commands, plugin model, updater |
| Native language | Rust | Runtime managers, trackers, PTY, browser webviews |
| Frontend | React 18 + TypeScript | Main renderer UI |
| State | Zustand | App and feature stores |
| Styling | Tailwind CSS + Radix/shadcn | Design system and primitives |
| Terminal UI | xterm.js | In-renderer terminal rendering |
| PTY backend | portable-pty + Se PtyManager | Native process-backed terminals, replay, and lifecycle |
| Build | Vite | Tauri dev/build integration |
| Testing | Vitest + Testing Library | Renderer validation |
| CI/CD | GitHub Actions | Validation, release, updater artifact publishing |

## High-Level Architecture

```text
User
  ↓
React Renderer (src/renderer)
  ├─ Layout shell
  ├─ Workspace panes/tabs
  ├─ Zustand stores
  ├─ Hooks/orchestration
  └─ Runtime adapter layer (src/renderer/lib)
        ↓
Tauri Command / Event Boundary
        ↓
Rust Runtime (src-tauri/src)
  ├─ PtyManager
  ├─ BrowserTabManager
  ├─ MigrationManager
  ├─ Trackers (cwd/git/exit code)
  └─ Window/menu/plugin integration
        ↓
OS / Filesystem / Shells / Child processes / Webviews
```

## Runtime Entry Points

### Renderer Entry Paths

There are two renderer bootstraps:

- `src/renderer/tauri-main.tsx` → always boots `TauriApp`
- `src/renderer/main.tsx` → chooses `TauriApp` or browser-safe `App` based on `__TAURI_INTERNALS__`

This preserves a browser/dev/test path while keeping the Tauri-specific app as the canonical desktop implementation.

### Native Entry Path

- `src-tauri/src/main.rs` initializes logging and delegates to `se_manager_lib::run()`
- `src-tauri/src/lib.rs` builds the Tauri app, plugins, menu, managed state, migrations, and invoke handlers

## Renderer Architecture

### 1. App Shell Layer

Core shell files:

- `WorkspaceLayout.tsx`
- `TitleBar.tsx`
- `StatusBar.tsx`
- route pages such as `ProjectSettings.tsx`, `AppPreferences.tsx`, and `WorkspaceSnapshots.tsx`

Responsibilities:

- application frame and navigation
- global keyboard shortcuts
- modal orchestration
- project switching
- unsaved-change handling
- pane area + file explorer composition

### 2. Workspace/Pane Layer

The pane system is a major architectural pillar:

- `workspace-store.ts` models the pane tree
- `PaneRenderer.tsx` recursively renders split or leaf nodes
- `PaneContent.tsx` maps a leaf pane to terminal/editor/browser tab surfaces
- `WorkspaceTabBar.tsx` manages tab UX

This enables a flexible workspace model more like a lightweight IDE than a simple terminal multiplexer.

### 3. Feature Surface Layer

Each workspace surface is specialized:

- **Terminal:** `ConnectedTerminal.tsx`, xterm integration, PTY data/event binding
- **Editor:** `EditorPanel.tsx`, `CodeEditor.tsx`, `MarkdownEditor.tsx`
- **Browser:** `BrowserPanel.tsx`, annotation UI, embedded browser controls
- **File Explorer:** `FileExplorer.tsx` and node/context-menu helpers

### 4. Store Layer

Zustand stores define the main state domains:

- project store
- terminal store
- workspace store
- editor store
- browser session store
- annotation store
- snapshot store
- updater store
- app settings store
- context bar settings store
- sidebar/file explorer/recent command support stores

This is a strongly store-driven architecture with hooks providing side-effect orchestration.

### 5. Hook / Orchestration Layer

Hooks such as:

- `use-projects-persistence`
- `use-terminal-restore`
- `useTerminalAutoSave`
- `use-file-watcher`
- `use-updater`
- `use-window-state`
- `use-command-history`
- `use-snapshots`

coordinate side effects, persistence, restore behavior, event subscriptions, and lifecycle policies.

### 6. Adapter / API Layer

`src/renderer/lib/` isolates platform integration behind typed adapters such as:

- `terminal-api.ts`
- `filesystem-api.ts`
- `persistence-api.ts`
- `browser-api.ts`
- `window-api.ts`
- `shell-api.ts`
- updater/session/migration adapters

This is a key architectural boundary: components are expected to use adapters, not raw Tauri APIs.

## Native Runtime Architecture

### 1. App Builder / Composition Root

`src-tauri/src/lib.rs` acts as the composition root. It:

- builds menus
- registers plugins
- creates managed singletons
- runs startup migrations
- registers invoke handlers
- handles app shutdown cleanup for PTYs and browser tabs

### 2. Tauri Commands Layer

`src-tauri/src/commands.rs` exposes a typed command surface for:

- terminal spawn/write/resize/kill/query operations
- browser tab creation/navigation/show/hide/destroy and annotation actions
- migration/version/history/rollback operations

The command layer standardizes responses using an `IpcResult<T>` pattern.

### 3. PTY Subsystem

Core files:

- `src-tauri/src/pty/manager.rs`
- `src-tauri/src/pty/windows.rs`
- `src-tauri/src/pty/mod.rs`

Responsibilities:

- terminal slot limiting
- process spawn/kill/resize/write
- terminal lifecycle tracking
- Windows-specific ConPTY handling
- renderer attachment bookkeeping
- transcript / output event streaming
- orphan detection policies

### 4. Tracker Subsystem

`src-tauri/src/trackers/` contains:

- `cwd_tracker.rs`
- `git_tracker.rs`
- `exit_code_tracker.rs`

These monitor terminal-associated metadata and emit updates back to the renderer. The git tracker includes Windows-specific command resolution logic to avoid problematic PATH selections.

### 5. Browser Webview Subsystem

`browser_tab_manager.rs` manages child webviews and browser-tab lifecycle:

- create/destroy/show/hide child webviews
- navigate/reload/back/forward
- inject URL/title/load polling scripts
- inject/remove annotation overlays
- maintain tab metadata and selection state

This is one of the most custom parts of the app architecture.

### 6. Migration Subsystem

`migrations.rs` implements migration versioning/history/rollback scaffolding backed by Tauri store. Startup runs can fail the app if migration integrity fails.

## Data and Persistence Architecture

### Renderer-Side Persistence

The renderer uses Tauri plugin-store adapters through versioned persistence APIs
for settings and lightweight application data. Desktop ACP chat transcripts are
an exception: they are owned by a Rust file store under Tauri app data, with a
lightweight index and one atomic JSON payload per session.

Primary persisted domains include:

- projects
- terminal layouts
- snapshots
- command history
- window state
- app settings
- Rust-backed desktop ACP chat history
- standalone ACP event-log session persistence

### Shared Persistence Contracts

`src/shared/types/persistence.types.ts` defines structures like:

- `PersistedProjectData`
- `PersistedTerminalLayout`
- `PersistedSnapshotList`
- `WindowState`

### Notable Persistence Patterns

- debounced writes with flush-on-close behavior
- version-wrapped persisted records
- atomic Rust replacement for desktop ACP payload/index files
- bounded renderer LRU for inactive full chat payloads; trimmed live sessions stay pinned
- verified, fail-closed legacy ACP import from `termul-data.json`
- desktop shared-live browser history reads durable files on demand instead of renderer clones
- transcript/scrollback persistence for restore scenarios
- session stores separated from general app data store

## State Management Patterns

### Project State
Tracks projects, active selection, archiving, colors, path, shell, and env vars.

### Terminal State
Tracks terminal records, PTY mapping, transcript accumulation, detached output, health, git/cwd/exit info, and hidden-state policies.

### Workspace State
Tracks the pane tree, active pane, active tabs, tab remapping, split/collapse behavior, and browser/editor/terminal tab helpers.

### Editor State
Tracks open file buffers, dirty state, view mode, cursor/scroll, save/reload transitions.

### Browser State
Tracks browser tabs, loading/title/nav state, annotation mode, and synchronization with runtime webviews.

## Browser Annotation Architecture

The annotation feature crosses multiple layers:

- browser session store for mode/state
- annotation store for persisted domain objects
- `BrowserPanel` and `AnnotationPanel` for UI
- `browser-api.ts` for IPC/event subscriptions
- `BrowserTabManager` for webview orchestration
- `src-tauri/resources/annotation-overlay.js` for in-page overlay injection

This is effectively a mini subsystem inside the app.

## Window / Menu / Updater Architecture

### Window Behavior
- custom title bar on Windows/Linux
- overlay-style native traffic lights on macOS
- hidden startup window shown once state is ready
- persisted window position/size/maximized state

### Menu Behavior
Native menu items include reload, zoom, full screen, updater trigger, and help link actions.

### Updater Behavior
The updater subsystem spans Tauri config, renderer store/hooks, and release workflow enforcement. Signed updater artifacts and manifest checks are required for stable releases.

## Testing Strategy

### Renderer
Vitest + Testing Library cover:

- components
- hooks
- stores
- adapter modules
- workspace interactions

### Native
CI runs:

- `cargo check --all-targets`
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`

## Deployment / Release Architecture

- Tauri bundles are built for Windows, Linux, and macOS
- release workflow validates version parity across package/config manifests
- updater artifacts (`latest.json`, `.sig`) are required for stable publish
- signing keys are managed through GitHub secrets and documented operationally

## Strengths of the Current Architecture

- strong separation between renderer UI and native runtime capabilities
- well-defined typed IPC/result patterns
- broad feature coverage for a desktop productivity shell
- store-driven composition with reusable hooks
- good CI discipline across JS and Rust stacks
- significant test surface in renderer code

## Web Terminal Transport

The renderer terminal seam is `terminal-api.ts`: Tauri uses typed commands and browser builds use a dedicated `/terminal/ws` socket. The terminal socket is intentionally separate from ACP `/ws`; it carries PTY requests, bounded scrollback replay/live output, and transport-neutral lifecycle/cwd/git/exit events. `ConnectedTerminal` remains the single xterm surface in both runtimes.

Standalone `se-server` owns its `PtyManager` and terminates those PTYs after graceful shutdown. Desktop shared-live mode passes the already-managed desktop `Arc<PtyManager>` into Axum, so stopping sharing detaches browser clients without killing desktop terminals. Output broadcast queues and replay scrollback remain bounded.

**Security boundary:** terminal authentication, authorization, TLS, and sandbox hardening are deferred. `/terminal/ws` must not be exposed to public or untrusted networks; existing server exposure controls are the only boundary in this version. Logs record lifecycle/request outcomes only and must never record terminal input, output, environment values, or secrets.

## Architectural Risks / Constraints

- terminal and pane rendering paths are performance-sensitive and complex
- browser annotation relies on injected scripts and webview timing behavior
- dual runtime/bootstrap paths require care when changing app initialization
- some legacy/transitional implementation surface remains in the renderer
- env var persistence includes a documented future security hardening gap for secrets

## Recommended Reading Order for Changes

### Terminal work
1. `src-tauri/src/core/terminal.rs` (independent Terminal Core ownership, local IPC, attach/replay)
2. `src/renderer/components/terminal/ConnectedTerminal.tsx`
3. `src/renderer/stores/terminal-store.ts`
4. `src/renderer/lib/tauri-terminal-api.ts`
5. `src-tauri/src/commands.rs`
6. `src-tauri/src/pty/manager.rs`

## Independent Core Processes (v1)

Desktop can run two independent long-lived roles from the packaged executable:

- `--terminal-core`: the sole owner of desktop PTYs, claims, trackers, output sequence/replay state, bounded scrollback, and terminal cleanup.
- `--acp-core`: the sole owner of the ACP manager, the Conversation durable writer (bootstrap-owned ordered persistence), the WS relay's durable admission, both rendezvous, scheduled tasks, and the memory index. The GUI proxies every command family over Core IPC (acp/history/conversation/scheduled-task/memory) and mirrors `acp:*` events verbatim; shared-live serves phone/browser history, conversations, tasks, and memory through the same Core-backed host.
- normal Tauri GUI: a client/launcher. It may adopt an existing Core endpoint and must not kill Core-owned PTYs during ordinary exit/relaunch. A supervisor watches both endpoints (3-miss detection, owned-child SIGTERM before respawn, in-place client reconnect with terminal stream restore); Windows named-pipe transport is compile-level only until validated on real Windows.

The local IPC layer is shared by both roles: length-delimited bounded frames, role/version handshake, per-profile endpoints, and OS-local permissions. Unix uses a user-owned runtime directory (0700) and socket (0600); Windows uses a current-user named-pipe ACL. This is one shared local policy, not two independent token systems. A live compatible endpoint is adopted; an incompatible handshake is not a silent adopt, and a still-live peer is never unlinked to make room for a replacement (see the crash/update matrix).

Terminal Core uses the existing terminal semantics: claim validation precedes attach replay; output carries sequence numbers and gap detection; the 256 KiB scrollback cap, `se-terminal-v2.binary`/`TML2` framing, watch subscriptions, and generic unauthorized claim errors remain unchanged. A GUI disconnect only removes a subscription; it does not terminate the PTY. Reconnect attaches to the existing terminal and replays retained output.

Crash / update matrix:

- **GUI quit/relaunch:** Cores stay alive; the GUI adopts the existing endpoint on return. Graceful exit reports `CORE_OWNED_SKIP` and does not kill Core-owned PTYs or ACP work.
- **Core crash:** supervisor 3-miss detection → SIGTERM-owned respawn → reconnect. ACP recovers from the durable repository. Terminal PTYs are unrecoverable in v1; `terminal:core_restarted` marks stale renderer terminals `exited` (users respawn).
- **App update:** GUI-only updater relaunch keeps Cores. When the app binary changes, Core respawn happens on the next GUI start because the adopted endpoint must speak the same protocol (incompatible handshake → best-effort wire shutdown only after HelloAck, then replace **only if the endpoint is gone**). A peer that rejected Hello is not assumed to process shutdown; a still-live endpoint is never unlinked and respawned. fd-passing handoff is deferred.
- **Explicit Core shutdown:** invokes the selected Core's shutdown operation and cleans only that Core's resources.

Standalone `se-server` remains an in-process, owning compatibility host and continues its existing drain-then-kill shutdown order. Browser/mobile keeps `/terminal/ws` and `/ws`; desktop shared-live remains non-owning and stops by detaching/draining clients. The browser and standalone surfaces therefore do not launch local Core processes.

Every Core-owned mutable resource must have one writer. GUI renderer stores are client caches; in Core-backed mode the GUI does not open ACP Conversation/history/scheduled-task/memory roots directly. ACP catalog/install state remains explicitly GUI-owned until a separate migration. Terminal and ACP Core failure domains are independent; shared-live ACP history/conversation/task/memory routes are Core-backed, while Windows desktop Core activation remains deferred pending real-machine validation.

### Unix `bun run dev:tauri` Terminal Core smoke

This is the supported manual development smoke for Unix/macOS. Start the desktop app with:

```sh
bun run dev:tauri
```

A Core-backed run must show both the role-process signal and the GUI connection signal:

```text
operation=core_ready role=terminal-core stable_code=READY
operation=desktop_terminal_core stable_code=READY
```

On a relaunch where the profile-scoped endpoint already exists, the first line is replaced by:

```text
operation=core_adopt role=terminal-core stable_code=ADOPTED
operation=desktop_terminal_core stable_code=READY
```

If startup logs `falling_back_in_process`, the application may still be usable, but that run is an in-process fallback and is not Terminal Core acceptance evidence. That fallback is admitted only when no live Core occupies the endpoint. A spawned or adopted Core whose GUI client then fails to connect logs `LIVE_CORE_UNREACHABLE` and refuses a second writer instead of bootstrapping GUI-side Conversation persistence. Confirming the Core process and its profile-scoped local endpoint is part of the smoke; a window opening alone is not proof of Core ownership.

For continuity testing, use a conversation-scoped terminal restored from `SessionWorkspace`, not a project-layout terminal. In the terminal, record a stable shell marker and keep the shell alive, for example:

```sh
printf 'CORE_SMOKE_PID=%s\\n' "$$"
sleep 600
```

Quit only the GUI; do not explicitly shut down the Core. The Core process and endpoint should remain available, the exit log should report `stable_code=CORE_OWNED_SKIP result=NOT_APPLICABLE`, and it must not report `PTY_CLEANUP_FAILED`. Relaunch with `bun run dev:tauri`, reopen the same conversation, and verify that the marker is replayed, the same terminal continues producing output, and a post-relaunch shell PID matches the pre-relaunch PID when the shell is resumed rather than replaced.

Project-layout restore adopts live Core PTYs by persisted `ptyId`; cold adopts bind the identity first and let the mounted terminal install replay/live ownership. Layouts without `ptyId` are legacy and fall back to heuristic match or re-spawn. Conversation SessionWorkspace restore remains the smoke for claim-issued replay. ACP desktop commands, Conversation persistence, and Core-backed shared-live history/conversation/task/memory routes use the ACP Core client.

The ACP Core owns Conversation lifecycle authority while Terminal Core owns PTYs. Suspend performs an operation-time scoped observation and never terminates PTYs. Delete performs observe → scoped idempotent terminate → re-observe before binding release or Conversation purge; unavailable, unknown, ownership-mismatched, or still-live terminal state is fail-closed and returns a live-resource/recovery outcome. Unix integration coverage starts both real Core processes, provisions a conversation-scoped terminal and workspace reference, then verifies ACP deletion removes the Terminal Core PTY before purging the Conversation. HTTP, WS, and Tauri lifecycle transports delegate to this same authority and only remove a requested workspace directory after an explicit `Deleted` outcome. Shared-live `/terminal/ws` uses the TerminalServiceHandle in Core-backed mode and the local manager only for in-process fallback. Windows named-pipe transport remains compile-level plumbing; desktop activation is deferred and logs `ACTIVATION_DEFERRED` for the explicit in-process fallback (not Core runtime acceptance). Packaged dual-role smoke remains outside this development acceptance path. Crash/update behavior is in the matrix above; fd-passing Core handoff is deferred.

### Browser/annotation work
1. `src/renderer/components/browser/BrowserPanel.tsx`
2. `src/renderer/stores/browser-session-store.ts`
3. `src/renderer/stores/annotation-store.ts`
4. `src/renderer/lib/browser-api.ts`
5. `src-tauri/src/browser_tab_manager.rs`
6. `src-tauri/resources/annotation-overlay.js`

### Workspace/layout work
1. `WorkspaceLayout.tsx`
2. `workspace-store.ts`
3. `PaneRenderer.tsx`
4. `PaneContent.tsx`
5. `WorkspaceTabBar.tsx`

---

_Generated using BMAD Method `document-project` workflow_
