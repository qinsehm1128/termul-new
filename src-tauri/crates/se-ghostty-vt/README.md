# se-ghostty-vt

A small MIT Rust wrapper around a pinned, real Ghostty VT C library. This is
**not a PTY integration** and must only be used in a **dedicated child process**.
The parent must impose process memory/CPU/time limits and a bounded IPC protocol.
No parser implemented in native code should be treated as an in-process sandbox.

## Features and build

Default features are empty. `NATIVE_AVAILABLE` is false and every terminal
operation returns `Error::Unavailable`; there is no fake empty-screen success.
Default builds need neither Zig nor libclang. The `native` feature compiles the
pinned MIT VT source and a small hand-written C shim; no bindgen is used.

From `src-tauri/`:

```sh
cargo test -p se-ghostty-vt
SE_GHOSTTY_VT_ZIG=/path/to/zig-0.16.0/zig \
  cargo test -p se-ghostty-vt --features native
```

Zig must be exactly **0.16.0**. The native build verifies the source archive's
SHA256 and safely extracts it to `OUT_DIR`. It runs:

```text
zig build -Demit-lib-vt -Demit-xcframework=false -Doptimize=ReleaseFast \
  -Dtarget=<Cargo target mapped to Zig> \
  -Dversion-string=1.3.2-HEAD-+44f2a44df --prefix <OUT_DIR>/native
```

Supported target mappings: macOS, GNU/musl Linux, MSVC Windows (only the host
platform actually tested should be considered verified). First-build Zig
package downloads can require network access; failures include complete Zig
stderr and must be resolved, never worked around with a stub engine.
`ZIG_LOCAL_CACHE_DIR` and `ZIG_GLOBAL_CACHE_DIR` can reuse downloaded packages and
build artifacts. All native controls are ignored when `native` is disabled.

An explicit locally trusted prebuild can avoid invoking Zig:
`SE_GHOSTTY_VT_PREBUILT_DIR=/absolute/prefix`. It must contain
`lib/libghostty-vt.a` (`ghostty-vt.lib` on MSVC) and a
`se-ghostty-vt.manifest` containing these exact `key=value` lines:

```text
source_sha256=af8cd2fbd25c17990b25c8bf8352263c35c5ec446134220de867236497311edd
version=1.3.2-HEAD-+44f2a44df
target=aarch64-apple-darwin
library_sha256=<SHA256 of the static library>
```

Use the actual Cargo target, not the example above. Source, target and binary
SHA256 are verified before copying the library into `OUT_DIR`; the shim also
checks the standalone VT ABI version (`0.1.0-dev` in this pinned source). Upstream
VT does **not** expose the Ghostty app commit/version through that API:
`-Dversion-string` configures the app, so source pin verification is performed by
build.rs, not inferred from this ABI string. This manifest is an explicit
local trust mechanism, not an upstream artifact signature. Always build that
library with the same pinned source and flags. Even prebuilt mode verifies and
extracts the pinned archive to obtain matching C headers.

## API and resource contract

- `Terminal::new(cols, rows)`, `write(bytes)`, `resize(cols, rows, cell_width_px,
  cell_height_px)` and `screen_tail(max_lines, max_bytes)` return `Result`.
- `ScreenSnapshot` contains plain UTF-8 text, viewport dimensions, alternate-screen
  status and **zero-based** cursor coordinates.
- Terminals are neither `Send` nor `Sync`; all methods require `&mut self` and no
  native handles or callbacks are exposed. No concurrent use of one instance.
- Maximum dimensions: 1024 columns, 1024 rows, 262,144 cells. Zero dimensions fail.
  Cell pixel dimensions are at most 4096 each (zero means unspecified).
- A single input chunk is at most 65,536 bytes. Split larger input in the caller;
  parser state, including incomplete escape sequences and UTF-8, survives writes.
- Output requests are at most 4096 lines / 1,048,576 bytes. Truncation keeps the
  newest text, never splits a UTF-8 character, and zero limits yield empty text
  while still reporting accurate metadata. Cursor coordinates describe the
  viewport, not the truncated text.
- Scrollback is explicitly disabled (zero bytes/lines), matching the approved
  observer protocol; `screen_tail` formats only the active screen. APC
  accumulation is limited to 4096 bytes and Kitty image storage is disabled.
  No WRITE_PTY, clipboard, bell, title or
  other host-effect callback is installed. Kitty file, temporary-file and shared
  memory media are all disabled; image storage is zero.
- Plain text formatting uses a bounded caller-owned buffer. An unexpected native
  formatting failure is an error, never an empty-screen fallback.

These bounds reduce ordinary resource exposure; they are **not** a hard total
RSS/CPU bound and do not replace child-process isolation or rate limiting. The
crate never opens a PTY or returns terminal query responses to an external host.

## Passive linkage

The shim has no C constructor or implicit initialization entrypoint. The VT
build root is upstream `src/lib_vt.zig`, not the application `src/main_c.zig`
(which contains a Windows app DllMain). Terminal state is allocated only by
explicit calls such as `Terminal::new`; static linkage does not call it. The
built aarch64 macOS static archive was checked with `xcrun otool -l` and
`xcrun nm -m`: no initializer/finalizer/ObjC load sections or global constructor
entrypoints were present. Linux/Windows artifact-level checks remain platform
verification work; this is not a claim of an OS permission sandbox.

## License and provenance

See `../../vendor/ghostty-vt/README.md` and `LICENSE` for pinned source identity,
archive checksum and the upstream MIT license. No herdr Rust binding is used.
