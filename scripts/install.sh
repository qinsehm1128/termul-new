#!/usr/bin/env bash
set -euo pipefail

OWNER="qinsehm1128"
REPO="termul-new"
BASE_URL="https://github.com/${OWNER}/${REPO}"

# Distribution identity — the ONLY place this script spells the product out.
# `PRODUCT_NAME` mirrors `src-tauri/tauri.conf.json` -> productName, which is what
# Tauri turns into every bundle file name; `PACKAGE_NAME` mirrors `package.json`
# -> name, which is the Linux binary. Everything below composes from these three
# values instead of repeating them, so a rename cannot land here by halves.
# `scripts/tests/artifact-name-derivation.test.ts` reads both upstreams, recomputes
# what these definitions must be, and rejects any artifact name spelled out again.
PRODUCT_NAME="Se Manager"
PACKAGE_NAME="se-manager"
# Published release assets carry the dotted form: Tauri bundles under the
# spaced product name (`Se Manager_0.5.9_aarch64.dmg`) and
# `scripts/release/prepare-platform-artifacts.mjs` -> `releaseAssetName()`
# replaces the spaces with dots when it stages them for the GitHub release.
# This script downloads the published asset, so it needs the dotted form.
BUNDLE_STEM="${PRODUCT_NAME// /.}"

die() {
  printf '%s\n' "$*" >&2
  return 1
}

detect_os() {
  local os
  os="$(uname -s)"

  case "$os" in
    Darwin)
      printf '%s\n' "darwin"
      ;;
    Linux)
      printf '%s\n' "linux"
      ;;
    MINGW* | MSYS* | CYGWIN* | Windows_NT)
      die "Windows is not supported by the curl installer. Download the .exe or .msi from ${BASE_URL}/releases."
      ;;
    *)
      die "Unsupported operating system: ${os}"
      ;;
  esac
}

detect_arch() {
  local arch
  arch="$(uname -m)"

  case "$arch" in
    arm64 | aarch64)
      printf '%s\n' "aarch64"
      ;;
    x86_64 | amd64)
      printf '%s\n' "x86_64"
      ;;
    *)
      die "Unsupported architecture: ${arch}"
      ;;
  esac
}

require_tools() {
  local os="${1:-}"
  local missing=()
  local tool
  local common_tools=(curl mktemp awk)

  for tool in "${common_tools[@]}"; do
    if ! command -v "$tool" >/dev/null 2>&1; then
      missing+=("$tool")
    fi
  done

  if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then
    missing+=("sha256sum or shasum")
  fi

  case "$os" in
    darwin)
      for tool in hdiutil cp xattr; do
        if ! command -v "$tool" >/dev/null 2>&1; then
          missing+=("$tool")
        fi
      done
      ;;
    linux)
      for tool in cp chmod mkdir; do
        if ! command -v "$tool" >/dev/null 2>&1; then
          missing+=("$tool")
        fi
      done
      ;;
  esac

  if ((${#missing[@]} > 0)); then
    printf 'Missing required tools:' >&2
    printf ' %s' "${missing[@]}" >&2
    printf '\n' >&2
    return 1
  fi
}

resolve_version() {
  local effective_url
  local version

  effective_url="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "${BASE_URL}/releases/latest")"
  version="${effective_url##*/}"

  if [[ ! "$version" =~ ^v[0-9]+[.][0-9]+[.][0-9]+([-.][0-9A-Za-z.-]+)?$ ]]; then
    die "Could not resolve latest ${PRODUCT_NAME} version from ${BASE_URL}/releases/latest"
    return 1
  fi

  printf '%s\n' "$version"
}

fetch_sha256sums() {
  local version="$1"
  local output="${2:-}"

  if [[ -z "$output" ]]; then
    output="$(mktemp)"
  fi

  curl -fsSL "${BASE_URL}/releases/download/${version}/SHA256SUMS.txt" -o "$output"
  printf '%s\n' "$output"
}

asset_version() {
  local version="$1"

  printf '%s\n' "${version#v}"
}

hash_file() {
  local file="$1"

  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{print $1}'
  else
    shasum -a 256 "$file" | awk '{print $1}'
  fi
}

verify_sha256() {
  local file="$1"
  local asset_name="$2"
  local sums_file="$3"
  local expected=""
  local actual

  expected="$(awk -v asset="$asset_name" '$2 == asset { print $1; found = 1; exit } END { if (!found) exit 1 }' "$sums_file")" || {
    die "Integrity check failed, nothing was installed: checksum for ${asset_name} not found"
    return 1
  }

  actual="$(hash_file "$file")"
  if [[ "$actual" != "$expected" ]]; then
    die "Integrity check failed, nothing was installed: checksum for ${asset_name} did not match"
    return 1
  fi
}

confirm_install() {
  local prompt="$1"
  local reply

  if [[ "${SE_INSTALL_YES:-}" == "1" ]]; then
    printf 'SE_INSTALL_YES=1: %s\n' "$prompt"
    return 0
  fi

  if [[ ! -r /dev/tty ]]; then
    die "Interactive confirmation requires /dev/tty. Set SE_INSTALL_YES=1 to install non-interactively."
    return 1
  fi

  printf '%s [y/N] ' "$prompt" >/dev/tty
  if ! IFS= read -r reply </dev/tty; then
    die "Interactive confirmation requires /dev/tty. Set SE_INSTALL_YES=1 to install non-interactively."
    return 1
  fi

  case "$reply" in
    y | Y | yes | YES)
      ;;
    *)
      die "Install cancelled."
      ;;
  esac
}

download_asset() {
  local version="$1"
  local asset_name="$2"
  local output="$3"

  curl -fL "${BASE_URL}/releases/download/${version}/${asset_name}" -o "$output"
}

install_macos() {
  local version="$1"
  local arch="${2:-$(detect_arch)}"
  local sums_file="${3:-}"
  local suffix
  local normalized_version
  local asset_name
  local tmpdir
  local dmg_path
  local mount_dir
  local applications_dir="${SE_INSTALL_APPLICATIONS_DIR:-/Applications}"
  local app_source
  local app_target

  case "$arch" in
    aarch64)
      suffix="aarch64"
      ;;
    x86_64)
      suffix="x64"
      ;;
    *)
      die "Unsupported macOS architecture: ${arch}"
      return 1
      ;;
  esac

  normalized_version="$(asset_version "$version")"
  asset_name="${BUNDLE_STEM}_${normalized_version}_${suffix}.dmg"
  tmpdir="$(mktemp -d)"
  dmg_path="${tmpdir}/${asset_name}"
  mount_dir="${tmpdir}/mount"
  mkdir -p "$mount_dir"

  (
    set -euo pipefail

    se_macos_mounted=0
    se_macos_mount_dir="$mount_dir"
    se_macos_tmpdir="$tmpdir"
    trap 'if [[ "${se_macos_mounted:-0}" == "1" ]]; then hdiutil detach "${se_macos_mount_dir:-}" >/dev/null 2>&1 || true; fi; rm -rf "${se_macos_tmpdir:-}"' EXIT

    if [[ -z "$sums_file" ]]; then
      sums_file="$(fetch_sha256sums "$version" "${tmpdir}/SHA256SUMS.txt")" || exit 1
    fi

    download_asset "$version" "$asset_name" "$dmg_path" || exit 1
    verify_sha256 "$dmg_path" "$asset_name" "$sums_file" || exit 1

    hdiutil attach -nobrowse -mountpoint "$mount_dir" "$dmg_path" || exit 1
    se_macos_mounted=1

    app_source="${mount_dir}/${PRODUCT_NAME}.app"
    app_target="${applications_dir}/${PRODUCT_NAME}.app"
    if [[ -e "$app_target" ]]; then
      if ! rm -rf "$app_target"; then
        sudo rm -rf "$app_target" || exit 1
      fi
    fi

    if ! cp -R "$app_source" "$applications_dir/"; then
      sudo cp -R "$app_source" "$applications_dir/" || exit 1
    fi

    hdiutil detach "$mount_dir" || exit 1
    se_macos_mounted=0
    xattr -dr com.apple.quarantine "$app_target" 2>/dev/null || true
    printf 'Installed %s to %s\n' "$PRODUCT_NAME" "$app_target"
  )
}

install_linux() {
  local version="$1"
  local arch="${2:-$(detect_arch)}"
  local sums_file="${3:-}"
  local normalized_version
  local asset_name
  local tmpdir
  local appimage_path
  local bin_dir="${SE_INSTALL_BIN_DIR:-${HOME}/.local/bin}"
  local desktop_dir="${SE_INSTALL_DESKTOP_DIR:-${HOME}/.local/share/applications}"
  local target_path="${bin_dir}/${PACKAGE_NAME}"
  local desktop_path="${desktop_dir}/${PACKAGE_NAME}.desktop"

  if [[ "$arch" != "x86_64" ]]; then
    die "Unsupported Linux architecture: ${arch}"
    return 1
  fi

  normalized_version="$(asset_version "$version")"
  asset_name="${BUNDLE_STEM}_${normalized_version}_amd64.AppImage"
  tmpdir="$(mktemp -d)"
  appimage_path="${tmpdir}/${asset_name}"

  (
    set -euo pipefail

    se_linux_tmpdir="$tmpdir"
    trap 'rm -rf "${se_linux_tmpdir:-}"' EXIT

    if [[ -z "$sums_file" ]]; then
      sums_file="$(fetch_sha256sums "$version" "${tmpdir}/SHA256SUMS.txt")" || exit 1
    fi

    download_asset "$version" "$asset_name" "$appimage_path" || exit 1
    verify_sha256 "$appimage_path" "$asset_name" "$sums_file" || exit 1

    mkdir -p "$bin_dir" "$desktop_dir" || exit 1
    cp "$appimage_path" "$target_path" || exit 1
    chmod 755 "$target_path" || exit 1
    cat >"$desktop_path" <<DESKTOP
[Desktop Entry]
Type=Application
Name=${PRODUCT_NAME}
Exec=${target_path}
Terminal=false
Categories=Development;Utility;
DESKTOP

    case ":${PATH}:" in
      *":${bin_dir}:"*)
        ;;
      *)
        printf 'Warning: %s is not in PATH. Add it to run %s from your shell.\n' "$bin_dir" "$PACKAGE_NAME" >&2
        ;;
    esac

    printf 'Installed %s to %s\n' "$PRODUCT_NAME" "$target_path"
  )
}

main() {
  local os
  local arch
  local version
  local sums_file
  local tmpdir

  os="$(detect_os)" || return 1
  arch="$(detect_arch)" || return 1

  # Published desktop targets are Apple Silicon macOS and Windows x64. Windows
  # never reaches this script (rejected in require_tools), so darwin/aarch64 is
  # the only tuple with a release asset. Reject anything else here rather than
  # letting it download: without the asset the run dies at the checksum step,
  # which reads like a corrupt release instead of an unsupported platform.
  # `install_linux` and the Intel-macOS branch are left intact — restoring either
  # target is a release-matrix entry plus relaxing this guard.
  if [[ "$os" != "darwin" || "$arch" != "aarch64" ]]; then
    die "No published build for ${os}-${arch}. ${PRODUCT_NAME} ships macOS (Apple Silicon) and Windows x64; build from source for other platforms: ${BASE_URL}#-getting-started"
    return 1
  fi

  require_tools "$os" || return 1
  version="$(resolve_version)" || return 1

  case "$os" in
    darwin)
      confirm_install "Install ${PRODUCT_NAME} ${version} (${os}-${arch}) to ${SE_INSTALL_APPLICATIONS_DIR:-/Applications}?" || return 1
      ;;
    linux)
      confirm_install "Install ${PRODUCT_NAME} ${version} (${os}-${arch}) to ${SE_INSTALL_BIN_DIR:-${HOME}/.local/bin}?" || return 1
      ;;
    *)
      die "Unsupported operating system: ${os}"
      return 1
      ;;
  esac

  tmpdir="$(mktemp -d)"
  (
    set -euo pipefail

    se_main_tmpdir="$tmpdir"
    trap 'rm -rf "${se_main_tmpdir:-}"' EXIT

    sums_file="$(fetch_sha256sums "$version" "${tmpdir}/SHA256SUMS.txt")" || exit 1

    case "$os" in
      darwin)
        install_macos "$version" "$arch" "$sums_file" || exit 1
        ;;
      linux)
        install_linux "$version" "$arch" "$sums_file" || exit 1
        ;;
    esac
  )
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
