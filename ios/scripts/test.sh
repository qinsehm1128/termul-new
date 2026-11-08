#!/usr/bin/env bash
# Build and test the SeRemote iOS app.
#
# Runs the full test plan (unit + UI) on the first available iPhone simulator.
# When no iOS simulator runtime is installed, it falls back to
# build-for-testing so app and test sources still compile.
#
# Usage: ios/scripts/test.sh [--build-only]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROJECT="$ROOT/SeRemote/SeRemote.xcodeproj"
DERIVED="${SE_IOS_DERIVED_DATA:-${TMPDIR:-/tmp}/seremote-derived}"

device_id=""
if [[ "${1:-}" != "--build-only" ]]; then
  device_id="$(xcrun simctl list devices available -j 2>/dev/null |
    python3 -c 'import json,sys
d=json.load(sys.stdin)["devices"]
ids=[x["udid"] for k,v in d.items() if "iOS" in k for x in v if "iPhone" in x["name"]]
print(ids[0] if ids else "")' || true)"
fi

if [[ -z "$device_id" ]]; then
  echo "No iPhone simulator available; compiling app and tests only." >&2
  exec xcodebuild -project "$PROJECT" -scheme SeRemote \
    -destination 'generic/platform=iOS Simulator' \
    -derivedDataPath "$DERIVED" build-for-testing
fi

exec xcodebuild -project "$PROJECT" -scheme SeRemote \
  -destination "id=$device_id" -derivedDataPath "$DERIVED" test
