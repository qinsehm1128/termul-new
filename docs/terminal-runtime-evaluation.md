# Terminal Rendering and Runtime Evaluation

**Status:** Recorded analysis; no runtime migration approved\
**Last reviewed:** 2026-08-21\
**Scope:** Desktop Tauri, shared-live remote access, standalone server, browser/mobile client

## Executive summary

Se does **not** depend on or launch `tmux`. The few `tmux` strings in the
repository are examples used while testing restoration of DEC private modes for
full-screen TUI programs such as `vim`, `less`, and `tmux`.

The production terminal runtime is owned by Se:

```text
OS PTY / ConPTY
  -> Rust PtyManager
  -> 4 ms bounded output flusher
  -> Tauri binary Channel or negotiated binary/JSON WebSocket protocol
  -> ConnectedTerminal
  -> xterm.js 6.1 beta (WebGL with DOM fallback)
```

Replacing "tmux with RMUX" is therefore not a dependency swap. It would replace
large parts of Se's PTY ownership, terminal identity, output replay,
authorization, cleanup, and remote transport architecture with an RMUX daemon
and SDK adapter.

The recommendation is:

1. Keep the current Se runtime for now.
2. Simplify and harden the current renderer lifecycle before changing engines.
3. If persistent daemon-owned sessions are strategically desirable, build an
   optional RMUX backend spike behind the existing `TerminalApi` facade.
4. Do not make RMUX the default until parity, packaging, security, recovery, and
   Windows acceptance tests pass.

## Current renderer assessment

### Existing strengths

- Desktop output uses `Uint8Array` over a Tauri binary channel.
- Rust batches PTY output every 4 ms with a 4 MiB pending-output limit.
- Host replay is sequence-aware and bounded to 256 KiB per terminal.
- Renderer transcripts are bounded to 1.5 million characters.
- Resize uses an 8 ms fit debounce, a 256 ms PTY resize debounce, and a minimum
  40x40 layout guard.
- WebGL context-loss recovery, DOM fallback, scrollback restoration, DEC mode
  replay, and desktop/web terminal transport parity already exist.
- Project switching can retain xterm state, while detached terminals have a
  bounded transcript fallback.

### Work that is necessary

1. Remove the duplicate terminal initialization/cleanup path in
   `ConnectedTerminal.tsx`.
2. Correct terminal-cache ownership:
   - cache the full renderer/addon session rather than only `Terminal`;
   - dispose cached terminals on close and eviction;
   - ensure cache clearing calls `Terminal.dispose()`;
   - prevent Fit/Search/WebGL addons from accumulating after reattachment.
3. Bound GPU usage for invisible tabs. Inactive tabs remain mounted, continue
   receiving output, and can each retain a WebGL context.
4. Route output subscribers by PTY id instead of broadcasting every chunk to
   every mounted terminal renderer.
5. Remove or finish the unused renderer-pool/factory/XTerminal paths. The
   current renderer pool is not production-wired and its eviction path discards
   the serialized snapshot it just created.
6. Replace skipped JSDOM benchmarks with real Chromium/WebKit/Tauri tests.
7. Add coverage for terminal cache disposal, alternate-screen restoration,
   terminal search, scrolling, Unicode/CJK/emoji, IME, and accessibility.

### Implementation progress

Completed on 2026-08-21:

- removed the duplicate `ConnectedTerminal` initialization and teardown path;
- changed the cache to retain one terminal session with its Fit/Search addons,
  cap it at 20 sessions, and dispose sessions on replacement, eviction,
  explicit terminal close, and cache clearing;
- stopped allocating WebGL for initially hidden tabs and release/recreate the
  WebGL addon as tabs move between hidden and visible states;
- added PTY-scoped output subscriptions to both desktop and web adapters while
  retaining the global subscription only for detached transcript capture and
  the pre-spawn path;
- added cache ownership, hidden-renderer, remount, and transport-routing tests,
  and restored previously skipped lifecycle/output tests;
- deleted the unused renderer pool, dormant ring, terminal factory,
  `XTerminal`, `TauriTerminal`, and `use-xterm` implementations together with
  their dedicated dependencies and tests;
- added an opt-in screen-reader setting, defaulting off, that updates both new
  and already-mounted xterm instances;
- added an opt-in `se-terminal-v2.binary` WebSocket subprotocol. New clients
  receive live and replay PTY bytes in compact binary frames, while old clients
  and protocol-stripping proxies continue to receive the existing JSON
  `number[]` frames;
- added UTF-8 transport baselines for CJK, emoji/ZWJ, and combining characters.

Still measurement-gated:

- changing the Rust 4 ms flusher;
- selecting a stable xterm release;
- adding a Unicode addon. The transport now preserves representative Unicode
  byte sequences, but glyph width, IME, and grapheme behavior still require
  real browser/Tauri acceptance measurements before changing xterm's Unicode
  provider.

These remain gated because they need a real browser/Tauri baseline rather than
JSDOM timing. The existing benchmark description now reflects the pinned xterm
6.1 beta, but it is still intentionally skipped when canvas support is absent.

### Work that is not currently justified

- A periodic render watchdog that performs recovery on every healthy streaming
  terminal.
- A WebGPU, Ghostty, libghostty, or custom renderer rewrite.
- Blindly upgrading the xterm 6.1 beta line without compatibility measurements.
- Automatically switching WebGL and DOM renderers during normal operation.

The preferred strategy is subtraction and measurement, not another layer of
renderer abstractions.

## Does Se use tmux?

No production source, Cargo dependency, npm dependency, shell invocation, or
runtime configuration calls `tmux`.

Current Rust PTY support is provided by `portable-pty` and Se's own
`PtyManager`. References to `tmux` are limited to:

- comments describing full-screen alternate-buffer applications;
- test fixture names for mouse and DEC private-mode restoration;
- documentation comparisons with terminal multiplexers.

There is therefore no user-installed tmux prerequisite to remove.

### What currently provides tmux-like behavior

Se implements the relevant behavior itself:

| Capability | Current owner |
| --- | --- |
| Keep a process alive while switching projects or renderer views | In-process `PtyManager` and renderer-reference tracking |
| Reattach and catch up output | Claim-gated attach/resume, 256 KiB retained output, sequence cursor and gap flag |
| Split panes and tabs | React workspace and `workspace-store`, not a terminal multiplexer |
| Restore full-screen TUI modes after remount | DEC private-mode capture plus `buildRehydrateSequences()` |
| Reap abandoned terminals | Protected/resource references and configurable orphan detection |
| Shared desktop/browser terminal | Se's Tauri Channel and `/terminal/ws` transports |

Current continuity is not equivalent to a daemonized multiplexer across a full
Se process exit. Se can persist and visually reconstruct bounded
scrollback/transcript and respawn a shell, but it does not resurrect the exact
live process after its in-process host has exited. Persistent live processes
across application restarts are the clearest capability RMUX could add.

## RMUX evaluation

Evaluated project: [Helvesec/rmux](https://github.com/Helvesec/rmux), release
`0.10.0`.

### What RMUX provides

- A Rust daemon that owns PTYs, sessions, windows, panes, scrollback, layouts,
  and process lifecycle.
- Native Unix PTY and Windows ConPTY implementations.
- Owner-scoped Unix sockets or Windows named pipes.
- A typed Rust SDK with stable session/window/pane handles.
- Raw output streams with sequence and lag reporting.
- `recover_output()` with authoritative ANSI rebases after initial attach,
  resize, lag, history reset, parser expiry, or process-generation change.
- Structured pane surface snapshots and streams.
- Input, resize, spawn, respawn, close, snapshot, and state-event APIs.
- Ratatui rendering and a separate encrypted Web Share product.
- MIT or Apache-2.0 licensing.

### Important architectural fact

`rmux-sdk` is daemon-backed. `Rmux::builder().connect_or_start()` starts or
connects to a separate RMUX daemon over local IPC. The public SDK is explicitly
a peer of the daemon, not an in-process PTY engine.

Using RMUX would still be "implemented in Rust", but it would add these packaged
runtime components:

```text
Se
  -> rmux-sdk
  -> local socket / named pipe
  -> rmux daemon
  -> PTY / ConPTY
```

Release packages may additionally require the public dispatcher, full CLI
helper, and daemon to remain in their expected directory layout. RMUX is not a
single library that can replace `portable-pty` with one Cargo dependency.

### The lower-level `rmux-pty` option

RMUX also publishes `rmux-pty` as a separate crate. That crate can be embedded
in-process and exposes PTY allocation, native Unix PTYs, Windows ConPTY,
reader/writer handles, resize, child control, and process-group signaling.

This creates two different migration choices:

| Choice | Runtime model | What it replaces |
| --- | --- | --- |
| `rmux-sdk` | Separate RMUX daemon over local IPC | Se PTY manager, retained output, process/session lifetime, and part of recovery |
| `rmux-pty` | In-process Rust library | Only `portable-pty` and some platform-specific process-control code |

The second choice is much smaller, but it does **not** provide RMUX sessions,
windows, panes, daemon persistence, snapshots, or recoverable streams.

It also does not make Se “more Rust-native”: the current `portable-pty`
dependency is already an in-process Rust crate that selects Unix PTY or Windows
ConPTY implementations. Replacing it with `rmux-pty` is an API/backend
evaluation, not a migration from a non-Rust technology.

### What RMUX would not replace

- xterm.js, WebGL, DOM rendering, terminal fonts, selection, search, and
  accessibility in the Tauri/browser UI.
- Se's React pane/tab/file/editor/browser workspace.
- Conversation-to-project attachment and execution-target rules.
- Se's browser/mobile UI.
- Se's authorization and claim model unless it is redesigned around RMUX.

The Ratatui RMUX widget targets terminal applications, not a Tauri WebView, so
it is not a replacement for `ConnectedTerminal`.

### Potential benefits

- Sessions and processes can survive the Se application process without
  Se maintaining its own host lifetime machinery.
- RMUX already owns cross-platform PTY, stable pane identity, retained output,
  lag detection, snapshots, and authoritative recovery.
- `recover_output()` is suitable for feeding an xterm.js frontend because it
  begins with an ANSI rebase and then preserves raw output bytes.
- The typed SDK is safer than shelling out to tmux-compatible CLI commands.
- A mature RMUX backend could eventually remove a significant amount of custom
  Se PTY and replay code.

### Migration blockers and risks

1. **Two ownership models**
   - Se currently owns terminal ids, conversation binding, renderer refs,
     cleanup state, leases/claims, limits, and orphan handling.
   - RMUX would own process and pane identity in another daemon.
   - A durable one-to-one identity and authorization mapping would be required.

2. **Two layout models**
   - Se already owns split panes and tabs.
   - RMUX also owns sessions, windows, pane layout, borders, and constrained
     resize behavior.
   - Using both layout systems would produce competing resize and lifecycle
     semantics. Using one RMUX pane per Se terminal avoids that conflict but
     gains little from RMUX multiplexing.

3. **Transport duplication**
   - Se already has Tauri channels, WebSocket attach/replay, renderer
     claims, and shared-live browser support.
   - RMUX Web Share has a different protocol, security model, frontend, and
     access policy; it is not wire-compatible with Se.

4. **Packaging and updates**
   - Se would need to bundle, launch, monitor, upgrade, and eventually stop
     a sidecar daemon on every supported platform.
   - RMUX 0.10.0 is not wire-compatible with 0.9.x daemons and requires old
     daemons to be stopped during upgrade.

5. **Maturity and platform risk**
   - RMUX is young and currently at 0.10.0.
   - Open issues include Windows resize instability, ESC handling in lazygit,
     copy behavior differences, and attach backlog under large popup output.

6. **Behavioral parity**
   - Environment injection, project cwd validation, shell profiles, terminal
     close semantics, crash recovery, output gaps, Unicode/OSC behavior, SSH,
     remote access, mobile reconnect, and app shutdown all require parity
     testing.

7. **Latency**
   - Input and resize gain an additional local IPC hop.
   - The SDK resize path snapshots current dimensions and can send separate
     width and height operations; this differs from Se's direct PTY resize.

Any full RMUX adapter must preserve at least: trusted spawn with cwd/env,
write/resize/terminate, retained-output attach with lag reporting,
multi-client fan-out, claim rotation/revocation without killing the process,
renderer/protected/orphan references, Conversation scoping, and exit/cwd/git
tracker integration. Session creation alone is not sufficient parity.

### Is replacing only `portable-pty` reasonable?

Yes, as an isolated experiment, but there is no current evidence that
`portable-pty` is the source of Se's renderer, memory, or lifecycle
problems. Most identified issues are above the raw PTY layer:

- duplicated React/xterm initialization;
- WebGL and addon ownership;
- invisible renderer count;
- output subscriber fan-out;
- skipped renderer recovery tests.

An `rmux-pty` spike is only justified if it demonstrates a concrete advantage
in ConPTY behavior, process-group termination, cleanup reliability, or
maintenance cost. It should keep `PtyManager` and every existing Se
contract unchanged so the backend can be compared with the current
`portable-pty` implementation.

## Replacement decision

### Direct full replacement

Technically possible, but not currently justified. It is a subsystem migration,
not a cleanup. It should not be combined with the renderer-lifecycle work.

### Recommended experiment

Introduce an optional backend behind the existing `TerminalApi` contract:

```text
TerminalApi
  |- NativeSeBackend (current default)
  `- RmuxBackend (experimental)
```

The spike should:

1. Bundle a pinned RMUX daemon and connect through `rmux-sdk`.
2. Create one RMUX pane for one Se terminal without adopting RMUX layout.
3. Feed `recover_output()` events into the existing xterm.js renderer.
4. Map input, resize, close, exit, cwd, title, and reconnect behavior.
5. Keep Se's existing renderer claims and browser protocol at the outer
   boundary.
6. Compare startup latency, input latency, heavy-output throughput, idle CPU,
   memory, recovery fidelity, and package size with the native backend.
7. Exercise macOS, Linux, Windows, shared-live, standalone server, and mobile
   browser paths.

Only after that experiment should the project decide whether RMUX should remain
optional, replace the native backend, or not be adopted.

## External references

- [RMUX repository](https://github.com/Helvesec/rmux)
- [RMUX 0.10.0 release](https://github.com/Helvesec/rmux/releases/tag/v0.10.0)
- [RMUX SDK](https://github.com/Helvesec/rmux/tree/main/crates/rmux-sdk)
- [RMUX architecture](https://github.com/Helvesec/rmux/blob/main/docs/ARCHITECTURE.md)
- [RMUX Windows resize issue #214](https://github.com/Helvesec/rmux/issues/214)
- [RMUX attach backlog issue #216](https://github.com/Helvesec/rmux/issues/216)
- [xterm WebGL disposal issue #6068](https://github.com/xtermjs/xterm.js/issues/6068)
- [xterm IME issue #6089](https://github.com/xtermjs/xterm.js/issues/6089)
<!-- Upstream citations: these PRs exist in gnoviawan/termul, the project this codebase
     derives from. Repointing them at qinsehm1128/termul-new would produce dead links. -->
- [Termul terminal performance PR #152](https://github.com/gnoviawan/termul/pull/152)
- [Termul xterm 6.1 migration PR #135](https://github.com/gnoviawan/termul/pull/135)
- [Termul minimize/restore fix PR #185](https://github.com/gnoviawan/termul/pull/185)
