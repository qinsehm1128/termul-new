#!/usr/bin/env bash
# Fail-closed acceptance for the optional host-side screen engine.
# Native tests are mandatory. A missing Zig toolchain or native library is a
# failure, not an excuse to silently run only the unavailable-engine stub.
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
export PYTHONDONTWRITEBYTECODE=1
# Acceptance must exercise the delivered source build, not a machine-local
# prebuilt shortcut (even though normal builds validate that shortcut).
unset SE_GHOSTTY_VT_PREBUILT_DIR
python_bin="${PYTHON:-python3}"
if [[ -z "${PYTHON:-}" ]] && command -v python3.12 >/dev/null 2>&1; then
  python_bin=python3.12
fi
ZIG="${ZIG:-${SE_GHOSTTY_VT_ZIG:-}}"

checkpoint() {
  printf 'QIN_CODE_CHECKPOINT {"message":"%s"}\n' "$1"
}

checkpoint 'Checksum-pinned build toolchain and failure handling'
"$python_bin" -c 'import tarfile; assert callable(getattr(tarfile, "data_filter", None)), "Use Python 3.12+ or a current Python 3.11"'
"$python_bin" scripts/release/prepare-ghostty-toolchain.test.py
if [[ -z "${ZIG:-}" ]]; then
  toolchain_cache="${GHOSTTY_TOOLCHAIN_CACHE:-}"
  if [[ -z "$toolchain_cache" ]]; then
    cache_root="${QIN_CODE_SCRATCH_DIR:-${RUNNER_TEMP:-}}"
    if [[ -z "$cache_root" ]]; then
      printf '%s\n' 'Set GHOSTTY_TOOLCHAIN_CACHE to an owned cache directory, or set ZIG to the verified Zig 0.16.0 executable.' >&2
      exit 1
    fi
    toolchain_cache="$cache_root/se-ghostty-toolchain"
  fi
  ZIG="$("$python_bin" scripts/release/prepare-ghostty-toolchain.py --cache-dir "$toolchain_cache" --print-executable)"
  export ZIG
  export ZIG_GLOBAL_CACHE_DIR="$toolchain_cache/global-cache-verify"
  export ZIG_LOCAL_CACHE_DIR="$toolchain_cache/local-cache-verify"
fi
export SE_GHOSTTY_VT_ZIG="$ZIG"

checkpoint 'TypeScript contracts and renderer type checking'
bun run typecheck

checkpoint 'Default build remains usable without native Ghostty'
cargo test --manifest-path src-tauri/Cargo.toml -p se-ghostty-vt

checkpoint 'Real Ghostty native VT behavior'
cargo test --manifest-path src-tauri/Cargo.toml -p se-ghostty-vt --features native

checkpoint 'Native library has no pre-main initialization or resolver hooks'
"$python_bin" scripts/release/verify-ghostty-passive-link.test.py
"$python_bin" scripts/release/verify-ghostty-passive-link.py \
  --cargo-target-dir "${CARGO_TARGET_DIR:-src-tauri/target}"

checkpoint 'PTY and Terminal Core default-path and isolation tests'
cargo test --manifest-path src-tauri/Cargo.toml -p se-pty -p se-terminal-core -p se-screen-protocol --lib

checkpoint 'Native production host entry points and read-only protocol'
export TERMUL_TERMINAL_CORE_BUILD_ID=ghostty-acceptance-v1
export TERMUL_SERVER_BUILD_ID=ghostty-acceptance-v1
cargo build --manifest-path src-tauri/Cargo.toml \
  --bin se-terminal-core --bin se-server --features standalone-server,ghostty-vt
target_dir="${CARGO_TARGET_DIR:-src-tauri/target}"
core_binary="$(cd "$target_dir/debug" && pwd)/se-terminal-core"
server_binary="$(cd "$target_dir/debug" && pwd)/se-server"
"$python_bin" scripts/tests/ghostty-observer-self-exec.test.py --binary "$core_binary"
"$python_bin" scripts/tests/ghostty-observer-self-exec.test.py --binary "$server_binary"
SE_TEST_OBSERVER_BINARY="$core_binary" SE_TEST_OBSERVER_BUILD_ID="$TERMUL_TERMINAL_CORE_BUILD_ID" \
  cargo test --manifest-path src-tauri/Cargo.toml -p se-pty \
    native_self_exec -- --ignored --nocapture

checkpoint 'Host configuration and WebSocket authorization'
cargo test --manifest-path src-tauri/Cargo.toml --lib terminal_screen_engine
cargo test --manifest-path src-tauri/Cargo.toml --lib screen_requests_require_live
cargo check --manifest-path src-tauri/Cargo.toml --lib --tests --bins --features standalone-server,ghostty-vt

checkpoint 'Project configuration, spawn, and Agent evidence regressions'
bun run test \
  src/renderer/pages/ProjectSettings.test.tsx \
  src/renderer/components/ProjectSidebar.test.tsx \
  src/renderer/lib/terminal-screen-engine.test.ts \
  src/renderer/lib/terminal-api.screen.test.ts \
  src/renderer/hooks/use-projects-persistence.test.ts \
  src/renderer/hooks/__tests__/use-projects-persistence-coalesce.test.ts \
  src/renderer/lib/terminal-spawn.test.ts \
  src/renderer/hooks/use-agent-terminal-state.test.ts \
  src/renderer/hooks/use-agent-terminal-state.host.test.tsx \
  src/renderer/stores/screen-observation-store.test.ts \
  src/renderer/components/terminal/TerminalScreenObservationStatus.test.tsx \
  src/renderer/lib/web-terminal-api.test.ts \
  src/renderer/lib/__tests__/tauri-terminal-api.test.ts \
  src/renderer/lib/agents/agent-terminal-state.test.ts \
  src/renderer/__tests__/renderer-root-parity.test.tsx \
  src/renderer/lib/__tests__/parity-checklist.test.ts

checkpoint 'Release identity and distribution regressions'
bun run test \
  scripts/release/ghostty-distribution.test.ts \
  scripts/release/component-build-inputs.test.ts \
  scripts/release/component-build-id.test.ts \
  scripts/release/component-identity-release.test.ts \
  scripts/release/prepare-server-artifacts.test.ts \
  scripts/release/prepare-platform-artifacts.test.ts

checkpoint 'Swift host-project and screen-contract model compatibility'
swift scripts/tests/ios-terminal-screen-engine.test.swift

checkpoint 'Native iOS simulator build (requires macOS and Xcode)'
if [[ "$(uname -s)" != Darwin ]] || ! command -v xcodebuild >/dev/null 2>&1; then
  printf '%s\n' 'Full Ghostty cross-surface acceptance requires macOS with Xcode for the iOS build gate.' >&2
  exit 1
fi
ios_cache_root="${QIN_CODE_SCRATCH_DIR:-${RUNNER_TEMP:-}}"
if [[ -z "$ios_cache_root" ]]; then
  printf '%s\n' 'Set QIN_CODE_SCRATCH_DIR or RUNNER_TEMP to an owned temporary directory for the iOS build.' >&2
  exit 1
fi
ios_derived_data="$(mktemp -d "$ios_cache_root/ghostty-ios-acceptance-XXXXXXXX")"
xcodebuild -project ios/SeRemote/SeRemote.xcodeproj \
  -scheme SeRemote -configuration Debug -sdk iphonesimulator \
  -destination 'generic/platform=iOS Simulator' \
  -derivedDataPath "$ios_derived_data" CODE_SIGNING_ALLOWED=NO build

checkpoint 'All Ghostty host screen mechanical acceptance gates passed'
