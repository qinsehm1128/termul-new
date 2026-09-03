SE_TEST_REPO_ROOT="$(cd "${BATS_TEST_DIRNAME:-$(dirname "${BASH_SOURCE[0]}")}/../.." && pwd)"

make_tmp() {
  SE_TEST_TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/termul-install-test.XXXXXX")"
  SE_TEST_STUB_BIN="$SE_TEST_TMP_DIR/bin"
  mkdir -p "$SE_TEST_STUB_BIN"

  SE_TEST_ORIGINAL_PATH="${SE_TEST_ORIGINAL_PATH:-$PATH}"
  PATH="$SE_TEST_STUB_BIN:$SE_TEST_ORIGINAL_PATH"

  export SE_TEST_TMP_DIR
  export SE_TEST_STUB_BIN
  export SE_TEST_ORIGINAL_PATH
  export PATH
}

cleanup_tmp() {
  if [[ -n "${SE_TEST_TMP_DIR:-}" && -d "$SE_TEST_TMP_DIR" ]]; then
    rm -rf "$SE_TEST_TMP_DIR"
  fi

  if [[ -n "${SE_TEST_ORIGINAL_PATH:-}" ]]; then
    PATH="$SE_TEST_ORIGINAL_PATH"
    export PATH
  fi

  unset SE_TEST_TMP_DIR
  unset SE_TEST_STUB_BIN
  unset SE_TEST_ORIGINAL_PATH
}

stub_cmd() {
  local name="$1"
  shift

  if [[ -z "${SE_TEST_STUB_BIN:-}" ]]; then
    make_tmp
  fi

  local stub_path="$SE_TEST_STUB_BIN/$name"
  {
    printf '#!/usr/bin/env bash\n'
    printf '%s\n' "$*"
  } >"$stub_path"
  chmod +x "$stub_path"
}

load_install() {
  source "$SE_TEST_REPO_ROOT/scripts/install.sh"
}
