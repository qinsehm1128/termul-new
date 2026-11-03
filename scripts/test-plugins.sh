#!/usr/bin/env bash
set -euo pipefail

# Run from the repository root even when called from another directory.
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly target_dir=/Users/qs/project/me/termul/src-tauri/target
export SE_PLUGIN_TEST_ARTIFACTS="$target_dir/plugin-test-artifacts"

case "$(uname -s)" in
  Darwin) library=libse_plugin_example.dylib ;;
  Linux) library=libse_plugin_example.so ;;
  MINGW*|MSYS*|CYGWIN*) library=se_plugin_example.dll ;;
  *) printf 'Unsupported plugin test platform: %s\n' "$(uname -s)" >&2; exit 1 ;;
esac

# Cargo overwrites the same debug library for each variant. Preserve each one
# immediately after its build, before requesting the next feature set.
mkdir -p "$SE_PLUGIN_TEST_ARTIFACTS"/{normal,fault,upgrade}
command cargo build --manifest-path src-tauri/Cargo.toml --target-dir /Users/qs/project/me/termul/src-tauri/target -p se-plugin-example --lib --locked
cp "$target_dir/debug/$library" "$SE_PLUGIN_TEST_ARTIFACTS/normal/$library"
command cargo build --manifest-path src-tauri/Cargo.toml --target-dir /Users/qs/project/me/termul/src-tauri/target -p se-plugin-example --lib --locked --features test-hooks
cp "$target_dir/debug/$library" "$SE_PLUGIN_TEST_ARTIFACTS/fault/$library"
command cargo build --manifest-path src-tauri/Cargo.toml --target-dir /Users/qs/project/me/termul/src-tauri/target -p se-plugin-example --lib --locked --features upgrade-fixture
cp "$target_dir/debug/$library" "$SE_PLUGIN_TEST_ARTIFACTS/upgrade/$library"

command cargo test --manifest-path src-tauri/Cargo.toml --target-dir /Users/qs/project/me/termul/src-tauri/target -p se-plugin-host --test plugin_e2e --locked -- --nocapture

# P3/P7b owns the process-layer test; run it as soon as that file lands.
if [[ -f src-tauri/tests/plugin_core_smoke.rs ]]; then
  command cargo test --manifest-path src-tauri/Cargo.toml --target-dir /Users/qs/project/me/termul/src-tauri/target -p se-manager --test plugin_core_smoke --locked -- --nocapture
fi
