#!/usr/bin/env bash
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly target_dir=/Users/qs/project/me/termul/src-tauri/target
export SE_PLUGIN_TEST_ARTIFACTS="${SE_PLUGIN_TEST_ARTIFACTS:-$target_dir/plugin-test-artifacts}"
case "$(uname -s)" in
  Darwin) library=libse_plugin_example.dylib ;;
  Linux) library=libse_plugin_example.so ;;
  MINGW*|MSYS*|CYGWIN*) library=se_plugin_example.dll ;;
  *) exit 1 ;;
esac
# Parent runner has preserved both native variants. This script is also usable
# standalone, after scripts/test-plugins.sh has generated those artifacts.
command cargo build --manifest-path src-tauri/Cargo.toml --target-dir /Users/qs/project/me/termul/src-tauri/target -p se-plugin-example --bins --locked
triple="$(command rustc -vV | awk '/^host:/ {print $2}')"
for variant in normal fault upgrade; do
  manifest=src-tauri/crates/se-plugin-example/src/manifest.json
  if [[ "$variant" == upgrade ]]; then manifest=src-tauri/crates/se-plugin-example/src/manifest-v2.json; fi
  "$target_dir/debug/se-plugin-pack" --manifest "$manifest" --target "$triple" --fixture "$variant" \
    --native "$SE_PLUGIN_TEST_ARTIFACTS/$variant/$library" \
    --ui src-tauri/crates/se-plugin-example/ui \
    --service "$target_dir/debug/se-plugin-example-service" \
    --out "$SE_PLUGIN_TEST_ARTIFACTS/$variant/activity-studio.seplugin"
done
command cargo test --manifest-path src-tauri/Cargo.toml --target-dir /Users/qs/project/me/termul/src-tauri/target -p se-plugin-example --bin se-plugin-pack --locked
command cargo test --manifest-path src-tauri/Cargo.toml --target-dir /Users/qs/project/me/termul/src-tauri/target -p se-plugin-host --test plugin_web_ui_e2e --locked -- --nocapture
