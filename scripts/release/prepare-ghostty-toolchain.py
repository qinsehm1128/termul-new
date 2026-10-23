#!/usr/bin/env python3
"""Install the pinned Ghostty build toolchain into an explicitly owned cache.

Checksums were read from https://ziglang.org/download/index.json on 2026-10-23.
No latest-version lookup, global installation, or TLS bypass occurs at build time.
Requires tarfile data extraction filters (Python 3.12+ or a current Python 3.11).
"""

import argparse
import hashlib
import os
from pathlib import Path, PurePosixPath
import platform
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request
import zipfile

VERSION = "0.16.0"
RELEASES = {
    "x86_64-linux": (
        "tar.xz", 55478392,
        "70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00",
    ),
    "aarch64-macos": (
        "tar.xz", 52238004,
        "b23d70deaa879b5c2d486ed3316f7eaa53e84acf6fc9cc747de152450d401489",
    ),
    # This is a build host, not an additional desktop release target. It can
    # cross-compile the supported Apple Silicon artifact on an Intel runner.
    "x86_64-macos": (
        "tar.xz", 57396836,
        "0387557ed1877bc6a2e1802c8391953baddba76081876301c522f52977b52ba7",
    ),
    "x86_64-windows": (
        "zip", 97217739,
        "68659eb5f1e4eb1437a722f1dd889c5a322c9954607f5edcf337bc3684a75a7e",
    ),
}


def host_platform(system=None, machine=None):
    system = (system or platform.system()).lower()
    machine = (machine or platform.machine()).lower()
    system = {"darwin": "macos"}.get(system, system)
    machine = {"arm64": "aarch64", "amd64": "x86_64"}.get(machine, machine)
    key = f"{machine}-{system}"
    if key not in RELEASES:
        raise ValueError(f"unsupported Ghostty build host: {key}")
    return key


def release_info(key):
    extension, size, digest = RELEASES[key]
    root = f"zig-{key}-{VERSION}"
    name = f"{root}.{extension}"
    return root, name, size, digest


def verify_archive(path, size, digest):
    if path.stat().st_size != size:
        raise ValueError(f"toolchain archive has incorrect size: {path.name}")
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(chunk)
    if hasher.hexdigest() != digest:
        raise ValueError(f"toolchain archive SHA256 mismatch: {path.name}")


def download_archive(cache, name, size, digest):
    archive = cache / name
    if archive.exists():
        verify_archive(archive, size, digest)
        return archive
    url = f"https://ziglang.org/download/{VERSION}/{name}"
    # A partial download is never accepted as a cache hit. The temporary file
    # lives in this explicit cache and is removed even when networking fails.
    with tempfile.NamedTemporaryFile(dir=cache, prefix="zig-download-", delete=False) as temp:
        partial = Path(temp.name)
        try:
            with urllib.request.urlopen(url, timeout=120) as response:
                received = 0
                while chunk := response.read(1024 * 1024):
                    received += len(chunk)
                    if received > size:
                        raise ValueError("toolchain download exceeds pinned size")
                    temp.write(chunk)
            temp.close()
            verify_archive(partial, size, digest)
            os.replace(partial, archive)
        finally:
            # Windows cannot unlink an open NamedTemporaryFile. Network/read/
            # oversize errors can occur before the successful-path close above.
            temp.close()
            partial.unlink(missing_ok=True)
    return archive


def validate_member(name, root):
    # Reject platform-dependent backslashes as well as traversal before either
    # extractor sees the name. Only the pinned archive root may be installed.
    path = PurePosixPath(name)
    if "\\" in name or path.is_absolute() or ".." in path.parts:
        raise ValueError("unsafe toolchain archive path")
    if not path.parts or path.parts[0] != root:
        raise ValueError("unexpected toolchain archive root")


def extract_archive(archive, staging, root):
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as bundle:
            for member in bundle.infolist():
                validate_member(member.filename, root)
                if (member.external_attr >> 16) & 0o170000 == 0o120000:
                    raise ValueError("toolchain zip contains a symlink")
            bundle.extractall(staging)
    else:
        with tarfile.open(archive, "r:xz") as bundle:
            for member in bundle.getmembers():
                validate_member(member.name, root)
            bundle.extractall(staging, filter="data")


def verify_binary(binary):
    result = subprocess.run(
        [str(binary), "version"], check=True, capture_output=True, text=True, timeout=15,
    )
    if result.stdout.strip() != VERSION:
        raise ValueError(f"expected Zig {VERSION}, got {result.stdout.strip()!r}")


def require_safe_extraction():
    if not callable(getattr(tarfile, "data_filter", None)):
        raise RuntimeError(
            "Ghostty toolchain installation requires tarfile data extraction "
            "filters. Use Python 3.12+ or a current Python 3.11 interpreter."
        )


def install(cache, key):
    # Fail before creating directories or downloading a large archive.
    require_safe_extraction()
    cache = cache.expanduser().resolve()
    cache.mkdir(parents=True, exist_ok=True)
    root, name, size, digest = release_info(key)
    archive = download_archive(cache, name, size, digest)
    target = cache / root
    binary_name = "zig.exe" if key.endswith("-windows") else "zig"
    binary = target / binary_name
    if target.exists():
        # Refuse arbitrary pre-existing or half-extracted installations rather
        # than deleting them or trusting a PATH executable's version string.
        marker = target / ".se-archive-sha256"
        if not marker.is_file() or marker.read_text().strip() != digest:
            raise ValueError(f"unverified toolchain directory already exists: {target}")
    else:
        with tempfile.TemporaryDirectory(dir=cache, prefix="zig-extract-") as staging:
            extract_archive(archive, Path(staging), root)
            extracted = Path(staging) / root
            verify_binary(extracted / binary_name)
            (extracted / ".se-archive-sha256").write_text(digest + "\n")
            shutil.move(str(extracted), target)
    verify_binary(binary)
    return binary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache-dir", type=Path, required=True)
    parser.add_argument("--platform", choices=RELEASES)
    parser.add_argument("--github-env", type=Path)
    args = parser.parse_args()
    key = args.platform or host_platform()
    binary = install(args.cache_dir, key)
    environment = {
        "ZIG": str(binary),
        "ZIG_GLOBAL_CACHE_DIR": str(binary.parent.parent / f"global-cache-{key}"),
        "ZIG_LOCAL_CACHE_DIR": str(binary.parent.parent / f"local-cache-{key}"),
    }
    if args.github_env:
        if any("\n" in value or "\r" in value for value in environment.values()):
            raise ValueError("newline in toolchain cache path")
        with args.github_env.open("a", encoding="utf-8") as stream:
            stream.writelines(f"{key}={value}\n" for key, value in environment.items())
    print(f"Verified Ghostty build toolchain: Zig {VERSION} ({key})")
    for name, value in environment.items():
        print(f"{name}={value}")


if __name__ == "__main__":
    main()
