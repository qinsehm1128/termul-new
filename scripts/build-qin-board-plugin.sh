#!/usr/bin/env bash
# Build the qin-code board plugin (cdylib + sidecar) and pack qin-board.seplugin.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly crate=src-tauri/crates/qin-board-plugin
readonly target_dir="${CARGO_TARGET_DIR:-$PWD/src-tauri/target}"
readonly out="$target_dir/qin-board/qin-board.seplugin"
case "$(uname -s)" in
  Darwin) library=libqin_board_plugin.dylib ;;
  Linux) library=libqin_board_plugin.so ;;
  *) echo "unsupported platform: $(uname -s)" >&2; exit 1 ;;
esac
command cargo build --manifest-path src-tauri/Cargo.toml --target-dir "$target_dir" \
  -p qin-board-plugin --release --lib --bins
triple="$(command rustc -vV | awk '/^host:/ {print $2}')"
"$target_dir/release/qin-board-pack" \
  --manifest "$crate/src/manifest.json" \
  --target "$triple" \
  --native "$target_dir/release/$library" \
  --ui "$crate/ui" \
  --service "$target_dir/release/qin-board-service" \
  --out "$out"
