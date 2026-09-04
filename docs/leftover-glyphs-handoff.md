# Leftover / stacked glyphs handoff

**Status:** Unsolved. Do not treat the working tree as a fix.  
**Date:** 2026-08-24  
**App:** Termul 0.4.10 (`com.termul-manager.app`), Tauri 2 + WKWebView on macOS  
**Branch / HEAD:** `main` @ `30451fca` (dirty working tree; leftover work is uncommitted)  
**This chat:** [Leftover glyphs investigation](f8277c8d-e0e9-4207-a4b8-33dee52a8804)

A later model should start here instead of re-deriving the same dead ends.

---

## 1. What the user sees

Interactive zsh + Powerlevel10k + zsh-syntax-highlighting. As the user types, **previous glyphs stay on screen** and the new ones draw on top. Dragging the macOS window immediately clears it. Incremental `terminal.refresh()` does not.

Confirmed shots (all production installs, not `tauri dev`):

| Time | Binary (approx) | What is on screen | What the shell actually did |
| --- | --- | --- | --- |
| earlier | pre-geometry | `cclaudclaudee --dangerously-skip-permissions`, error line twice, current line `pppi` | unknown |
| after first leftover pass | | `cccclcclaude` + gray autosuggestion | unknown |
| TUI | | `pi update --extensions` garbled (`pi-ma8str658`, leftover `s` / `kages`) | same class as leftover redraw, not sidebar dups |
| 17:27 | after 17:17 geometry install | visual `pipppipi` (green `pi` + red leftover) | zsh ran **`pipi` once**; `command not found: pipi` printed **twice** |
| 17:40 | after 17:39 atlas-clear removal | `llls` listing printed as two identical 4-line blocks; input row is a dense `l`/`s` smudge with a red tail | unknown whether `llls` or `ll`/`ls` actually ran |
| **17:52** | **after 17:48:26 CSS + model-clear install** | prompt `qs` / `.../ns/ruby/ns_auth` / `17:52`, typed **`ppi`**: white `p` with a **red `p` ghost** slightly offset; cursor block on `i` still shows red+white remnants | still broken |

The 17:52 shot is the most useful: the leftover is the **same codepoint** (`p`) in a **different color** (highlighter turned the word red). That is an in-place ZLE recolor, not a wrap/SIGWINCH problem.

User quote that must drive the next investigation: **「我只要一拖动窗口就会好」**.

---

## 2. Environment the next model must keep in mind

- Surfaces that must stay in parity if behavior changes: desktop Tauri, desktop shared-live host, standalone `termul-server`, browser/phone. See `AGENTS.md`.
- Renderer: `src/renderer/components/terminal/ConnectedTerminal.tsx`. Hidden tabs stay mounted (`INACTIVE_TAB_PANE_CLASS` in `PaneContent.tsx`). Do not kill PTYs on project switch; xterm instances can be cached in `terminal-cache.ts`.
- Declared xterm range in `package.json`: `@xterm/xterm ^6.1.0-beta.216`, `@xterm/addon-webgl ^0.20.0-beta.215`.
- **Installed lockfile versions are newer:** `@xterm/xterm@6.1.0-beta.287`, `@xterm/addon-webgl@0.20.0-beta.286` (`bun.lock`). Do not analyze beta.216 sources.
- Prompt fonts: `ConnectedTerminal.tsx` appends a Nerd Font fallback chain + `"PingFang SC"` for CJK (`buildTerminalFontChain`).
- Cell options now forced in `terminal-config.ts`: `lineHeight: 1`, `letterSpacing: 0`, `smoothScrollDuration: 0`, `rescaleOverlappingGlyphs: true`, `scrollbar.showScrollbar: false`.
- Production install used during this thread:

```bash
bunx tauri build --config src-tauri/tauri.conf.prod.json --config '{"bundle":{"createUpdaterArtifacts":false}}'
ditto --rsrc --extattr "src-tauri/target/release/bundle/macos/Termul Manager.app" "/Applications/Termul Manager.app"
```

Last leftover-targeted binary mtime: **2026-08-24 17:48:26**. User reproduced at **17:52** on that build. Version string stays 0.4.10; distinguish builds by binary mtime. Fully quit the old process before testing.

---

## 3. What window drag actually does

Pane / window size changes go through `use-terminal-resize-v2.ts`:

1. `ResizeObserver` waits 8 ms.
2. `performFit(false)` runs only if **rounded container CSS pixels** changed.
3. `FitAddon.fit()` → `terminal.resize`.
4. If `cols`/`rows` are unchanged, xterm returns immediately (`CoreBrowserTerminal.resize`). No `refresh`, no WebGL `handleResize`, no atlas work.
5. `pushPtyResize` also no-ops on an unchanged grid. Rust `PtyManager::resize` now also no-ops same-size (and parks desktop size while phone-fit).

So:

- **Title-bar move (no size change):** no xterm resize, no PTY ioctl, no SIGWINCH. Only WKWebView recomposites the window. If this alone clears leftovers, the canvas/buffer is already correct and the displayed layer was stale — **or** something else in the window-server path invalidates WebGL.
- **Real size change that also changes cols/rows:** `WebglRenderer.handleResize` resizes the canvas bitmap (this **clears** it), `_clearModel(false)`, then a **synchronous** full redraw (`WebglRenderer.ts` ~192–226). That is a guaranteed visual reset.
- `forceFit` (visibility / attach / font / recovery) always forwards the grid; Rust suppresses a same-size ioctl.

A 1 px same-grid drag is the cheapest experiment that is still missing a recorded result (see §8).

---

## 4. Attempts in this thread (all failed to clear the 17:52 class of leftover)

Work landed in the dirty working tree. None of it is a successful fix. Later models should not repeat these as first actions.

### 4.1 Dual PTY write mute — does not explain live leftover

**Hypothesis:** spawn Channel + attach/watch Channel both `dispatchTerminalData`, so every chunk is `terminal.write` twice.

**Change:** `src/renderer/lib/tauri-terminal-api.ts`

- `spawnOutputGates` mutes the spawn flusher when attach/watch/resume starts (`beginLiveHandoff`).
- Handoff uses `LIVE_HANDOFF_LAST_SEQ = Number.MAX_SAFE_INTEGER` so attach does not replay history the spawn path already painted.

**Why it does not close this bug:** workspace terminals that already hold a spawn-issued `claim` and `healthStatus === 'running'` **never attach**. `resumeTerminalResource` returns immediately (`terminal-store.ts`). Those windows stay spawn-only, so the gate never mutes anything. Typical project tabs (the user’s `ns_auth` shots) are this path.

`pipi` running once already falsifies “keys written twice to the PTY”. Double `command not found` / double `ls` listing can still be (a) leftover paint of the same rows or (b) a remaining double-dispatch. Not proven either way.

### 4.2 Geometry sync — did not fix 17:27+

**Hypothesis:** xterm cols ≠ PTY cols, so zsh/p10k redraws at the wrong wrap.

**Change:**

- `src-tauri/src/pty/manager.rs`: same-size resize is a no-op; phone-fit **parks** the latest desktop size instead of dropping it. Tests: `same_size_resize_skips_ioctl`, `resize_is_ignored_while_phone_fit` updated.
- `src/renderer/hooks/use-terminal-resize-v2.ts`: removed the 256 ms PTY debounce; fit then resize immediately. `forceFit` always forwards the grid.
- `ConnectedTerminal.tsx`: `forceResizeFit` after spawn and after external attach; font changes also `forceResizeFit`.

User still saw leftovers after the 17:17 install. Drag-to-clear plus executed command `pipi` weakens “geometry still wrong” **if** a same-grid drag also clears (not recorded).

### 4.3 Write-idle `terminal.refresh` — ran, did not help

`createWebglScrollRepair` already refreshed on scroll idle (120 ms). `handleTerminalOutput` only did `terminal.write(data)`. Added `onWrite()` → same idle refresh.

Installed ~17:34. Leftovers persisted. Important: the user can stare at the line for seconds, so the 120 ms timer **did fire**. Another `refresh()` is not what window drag does.

### 4.4 Stop `clearTextureAtlas` — correct, but not sufficient

Read-only pass: [Trace leftover after drag](d8d87e9f-4aa8-431b-8a24-d55352af0f60).

xterm WebGL shares one atlas across matching terminals (`CharAtlasCache.ts`). In beta.286, `TextureAtlas.clearTexture()` does **not** set `_requestClearModel`. Only the renderer that called `clearTextureAtlas()` resets its model (`WebglRenderer.ts` ~335–339). Siblings keep stale UVs. Unchanged cells are skipped (`WebglRenderer.ts` ~570–575).

**Change:** `terminal-webgl-repair.ts` no longer calls `clearTextureAtlas`. Atlas add/remove only schedule refresh. Hide/show still disposes and recreates the addon.

Installed ~17:39. User reproduced again at 17:40 (`llls` double listing + stacked input).

Keep this change. Do not reintroduce routine atlas clears.

### 4.5 Remove canvas compositor promotion + local model clear — still broken at 17:52

**Hypothesis:** `transform: translateZ(0)` + `contain: layout paint` on `.xterm-screen canvas` / `.xterm-scrollable-element` made WKWebView cache a stale layer. Window move recomposites it. That matched “drag window → fixed” better than `refresh()`.

**Change:**

- Removed those rules from `src/renderer/index.css` (comment left so they are not put back).
- Write/scroll idle now calls `clearWebglRenderModel` → `terminal._core._renderService.clear()` (this renderer only; **not** the shared atlas), then `terminal.refresh(0, rows-1)`.

Installed **17:48:26**. User shot at **17:52** still shows white+red stacked `p`. So either:

1. Compositor promotion was not the (only) cause, or
2. The 17:52 window was still the old process (user was asked to fully quit; not independently verified), or
3. Model `clear()` + refresh still skip or miss the overlapping paint, or
4. Two surfaces are painting the same cell (WebGL + another canvas / pixel-scroll / DOM).

Treat (2) as a process check, not as a reason to retry the same CSS patch.

---

## 5. Files that belong to this investigation

### In-scope (leftover / resize / output)

| File | Role |
| --- | --- |
| `src/renderer/components/terminal/terminal-webgl-repair.ts` | Idle repair. `clearWebglRenderModel`, `createWebglScrollRepair`, `restoreVisibleTerminalSurface`. **No atlas clear.** |
| `src/renderer/components/terminal/terminal-webgl-repair.test.ts` | Unit tests for the above |
| `src/renderer/components/terminal/ConnectedTerminal.tsx` | Wires repair, `onWrite`, hide/show WebGL dispose/recreate, `forceResizeFit`, Nerd Font chain |
| `src/renderer/components/terminal/ConnectedTerminal.test.tsx` | Hide/show and atlas-merge tests now assert **no** `clearTextureAtlas` |
| `src/renderer/components/terminal/terminal-config.ts` | `lineHeight 1`, `letterSpacing 0`, `rescaleOverlappingGlyphs` |
| `src/renderer/components/terminal/terminal-pixel-scroll.ts` | Sub-row `translateY` on `.xterm-screen` (weak for horizontal leftover; still live) |
| `src/renderer/index.css` | Scrollbar hidden; **no** canvas `translateZ(0)` / `contain: paint` |
| `src/renderer/hooks/use-terminal-resize-v2.ts` | Immediate PTY resize after fit |
| `src/renderer/lib/tauri-terminal-api.ts` | Spawn/attach output handoff mute |
| `src-tauri/src/pty/manager.rs` | Same-size resize no-op; phone-fit parks desktop size |

### Same dirty tree, **not** leftover work

Do not fold these into a leftover PR without a human: theme/ANSI palette, ActivityRail, CommandPalette, MobileChatShell, locales, WorkspaceLayout, router, `terminal-spawn`, AppPreferences, `pty/env_refresh.rs` (UTF-8 locale + `FORCE_COLOR` / `CLICOLOR`). Those came from earlier turns in a long session.

Untracked leftover-related files: `terminal-webgl-repair.ts`, `terminal-pixel-scroll.ts` (+ tests).

---

## 6. Ranked remaining causes

After the failed passes, this is the honest order. None is proven.

### 6.1 WebGL incremental skip + color rewrite (still live)

ZLE/highlighter rewrites `p` in the same cell with a new fg. WebGL *should* see fg change and update (`WebglRenderer.ts` ~570–575). The 17:52 overlay (white `p` under red `p`, slightly offset) looks like **the old glyph was not erased from the GPU quad**, or a second quad was drawn at a sub-pixel offset.

`refresh()` does not rebuild the glyph renderer the way `handleResize` does (canvas width/height assign **clears** the drawing buffer). `renderService.clear()` calls `WebglRenderer.clear()` → `_clearModel(true)` + render-layer reset, **without** resizing the canvas bitmap. If leftovers live in the GL default framebuffer rather than the cell model, `clear()` is not equivalent to a window resize.

**Falsify:** log or screenshot `terminal.buffer.active` for that row. If the buffer is a single `ppi` and the canvas shows two `p`s, transport/shell are done. Switch that terminal to the **DOM** renderer and repeat `ppi`. If DOM is clean, it is WebGL/GPU.

### 6.2 Canvas not actually cleared on “drag”

If the user is grabbing the **edge** (resize) rather than the title bar (move), every “drag fixes it” is just `handleResize` clearing the bitmap. Then the missing API is “clear the WebGL drawing buffer without changing cols/rows”, not compositor CSS.

**Falsify:** record `terminal.cols` / `terminal.rows` and container `getBoundingClientRect` before/after a title-bar-only move. If they are unchanged and leftovers disappear, compositor/IOSurface. If leftovers only disappear when the grid or canvas pixel size changes, implement a same-grid canvas rebuild (see §8).

### 6.3 Powerline / wcwidth mismatch

Prompt uses Nerd Font + p10k pills. If zsh `wcwidth` ≠ xterm cell width, ZLE erases/recolors at the wrong column and leaves a ghost one cell (or a fraction of a cell) away. That matches a **slightly offset** red `p` under a white `p`. Window resize sends SIGWINCH / full p10k reprint and looks fixed.

Does **not** by itself explain a same-grid title-bar move clearing the GPU, unless that move is actually a 1 px resize.

**Falsify:** plain `%# ` prompt, no p10k, no syntax highlighting. If leftovers vanish, width/theme. If they remain, renderer.

### 6.4 Pixel-scroll `translateY` on `.xterm-screen`

`terminal-pixel-scroll.ts` applies `translateY` for sub-row wheel remainder and sets `will-change: transform`. Typing at the bottom should have offset `0`. Resize recomputes the old offset rather than resetting it.

Weak for horizontal `p` ghosts. Still a transform on the screen that can interact with WebKit layers after we removed canvas `translateZ`.

**Falsify:** during `ppi`, `getComputedStyle(.xterm-screen).transform` should be `none`. Temporarily disable `attachPixelSmoothScroll`.

### 6.5 Duplicate output (secondary)

Scoped subscribe vs global `onData`, spawn mute, single attach forwarder, store ownership — current code closes the known double-write routes for attach. Claim-present tabs never attach.

**Falsify:** log host `seq` once before `dispatchTerminalData`, compare to xterm buffer line count. Two buffer error lines ⇒ duplicated bytes. One buffer line + two visual lines ⇒ renderer.

### 6.6 Already falsified or weak

| Claim | Why not |
| --- | --- |
| Keys written twice to the PTY | `pipi` executed once |
| Parser does not mark CSI EL dirty | xterm `InputHandler` does; extra `refresh` after write already ran |
| Hidden-tab first paint | 17:52 is a focused project tab; show path already recreates WebGL |
| Routine `clearTextureAtlas` | Removed; 17:40 and 17:52 still fail |
| Write-idle `refresh` alone | Fired; user still sees leftovers |
| CSS `translateZ(0)` as the sole cause | Removed in 17:48:26; 17:52 still fails (unless old process) |

---

## 7. Suggested next work (do these, not another refresh wrapper)

1. **Process check.** Confirm `/Applications/Termul Manager.app/Contents/MacOS/termul-manager` mtime is 17:48:26 and `ps` has only one Termul after a full quit.
2. **Buffer vs pixels.** In the leftover state, dump the active buffer line and a screenshot. That splits “double write” from “GPU ghost” in one step.
3. **DOM renderer A/B.** Same `ppi` / `llls` with WebGL off. If DOM is clean, stay on WebGL internals / WKWebView, not PTY.
4. **Title-bar move vs edge resize.** Instrument cols/rows/rect. This decides compositor vs `handleResize` canvas clear.
5. **Same-grid canvas rebuild.** If (4) says resize-clear is the real cure, call the WebGL path that assigns `canvas.width` / `canvas.height` (or dispose+recreate the addon) **without** `terminal.resize` and **without** `clearTextureAtlas`. Do not send SIGWINCH.
6. **Plain prompt.** No p10k, no syntax highlighting, JetBrains Mono only.
7. **Disable pixel-scroll** for one install.
8. Do **not** add another SIGWINCH / `forceFit` / atlas-clear workaround unless a new measurement requires it.

---

## 8. Performance note (already asked)

Removing `translateZ(0)` / `contain: paint` only drops “don’t invalidate the sidebar on wheel” isolation. WKWebView already gives WebGL its own surface. Idle `clear()` + full-row `refresh` is one viewport rebuild after 120 ms of quiet output, not an atlas or GL-context rebuild. Not the current problem.

---

## 9. How to read the code quickly

```text
PTY reader → 4 ms flusher
  → spawn Channel (always, lifetime of PTY)
  → optional attach/watch Channel (muted spawn if handoff runs)
  → dispatchTerminalData
      → ConnectedTerminal.handleTerminalOutput → terminal.write + onWrite()
      → useTerminalDetachedOutput (transcript only when no renderer)
```

WebGL skip-unchanged: `node_modules/@xterm/addon-webgl/src/WebglRenderer.ts` ~570–575.  
Resize rebuild: same file ~192–226 (`canvas.width/height` assign + `_clearModel(false)` + sync redraw).  
`clear()` (what we call today): ~341–351 (`_clearModel(true)`, no canvas bitmap resize).  
Shared atlas: `node_modules/@xterm/addon-webgl/src/CharAtlasCache.ts`.

---

## 10. Related docs

- `AGENTS.md` — product rules and surfaces
- `docs/terminal-runtime-evaluation.md` — runtime/xterm overview (older; still useful)
- `docs/index.md` — doc index
