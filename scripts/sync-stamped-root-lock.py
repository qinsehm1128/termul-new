#!/usr/bin/env python3
"""Synchronize one stamped root Cargo.lock package version with Cargo.toml."""

from __future__ import annotations

import argparse
import errno
import os
from pathlib import Path
import re
import stat
import sys
import tempfile
import tomllib


PACKAGE_HEADER = re.compile(
    rb"(?m)^[ \t]*\[\[package\]\][ \t]*(?:#[^\r\n]*)?(?:\r?\n|$)"
)
VERSION_LINE = re.compile(
    rb'(?m)^(?P<prefix>[ \t]*version[ \t]*=[ \t]*)"(?P<value>[^"\r\n]*)"'
    rb"(?P<suffix>[ \t]*(?:#[^\r\n]*)?)(?P<newline>\r?\n|$)"
)


class SyncError(RuntimeError):
    """A stable, user-facing synchronization failure."""


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Synchronize a source-free root Cargo.lock package version"
    )
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--lockfile", required=True, type=Path)
    parser.add_argument("--package", required=True)
    return parser.parse_args(argv)


def manifest_package_version(path: Path) -> str:
    with path.open("rb") as handle:
        document = tomllib.load(handle)
    package = document.get("package")
    if not isinstance(package, dict):
        raise SyncError("manifest has no [package] table")
    version = package.get("version")
    if not isinstance(version, str) or not version or any(char in version for char in '"\r\n'):
        raise SyncError("manifest package.version is not a supported string")
    return version


def package_blocks(lock_bytes: bytes) -> list[tuple[int, int, dict[str, object]]]:
    headers = list(PACKAGE_HEADER.finditer(lock_bytes))
    blocks: list[tuple[int, int, dict[str, object]]] = []
    for index, header in enumerate(headers):
        start = header.start()
        end = headers[index + 1].start() if index + 1 < len(headers) else len(lock_bytes)
        block_bytes = lock_bytes[start:end]
        try:
            parsed = tomllib.loads(block_bytes.decode("utf-8"))
        except (UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
            raise SyncError(f"Cargo.lock package block {index + 1} is not valid TOML") from error
        packages = parsed.get("package")
        if not isinstance(packages, list) or len(packages) != 1 or not isinstance(packages[0], dict):
            raise SyncError(f"Cargo.lock package block {index + 1} is malformed")
        blocks.append((start, end, packages[0]))
    return blocks


def synchronized_bytes(lock_bytes: bytes, package_name: str, version: str) -> bytes:
    candidates = [
        (start, end, package)
        for start, end, package in package_blocks(lock_bytes)
        if package.get("name") == package_name and "source" not in package
    ]
    if len(candidates) != 1:
        raise SyncError(
            f"expected exactly one source-free [[package]] named {package_name!r}; found {len(candidates)}"
        )

    block_start, block_end, _package = candidates[0]
    block = lock_bytes[block_start:block_end]
    version_lines = list(VERSION_LINE.finditer(block))
    if len(version_lines) != 1:
        raise SyncError(
            f"source-free [[package]] named {package_name!r} must contain exactly one version line"
        )

    match = version_lines[0]
    line_start = block_start + match.start()
    line_end = block_start + match.end()
    replacement = (
        match.group("prefix")
        + b'"'
        + version.encode("utf-8")
        + b'"'
        + match.group("suffix")
        + match.group("newline")
    )
    updated = lock_bytes[:line_start] + replacement + lock_bytes[line_end:]

    updated_line_end = line_start + len(replacement)
    if updated[:line_start] != lock_bytes[:line_start] or updated[updated_line_end:] != lock_bytes[line_end:]:
        raise SyncError("byte-identity verification outside the root version line failed")
    if updated[line_start:updated_line_end] != replacement:
        raise SyncError("root version line replacement verification failed")
    return updated


ERROR_INVALID_FUNCTION = 1
ERROR_ACCESS_DENIED = 5
ERROR_NOT_SUPPORTED = 50
WINDOWS_DIRECTORY_FLUSH_OMIT_ERRORS = (
    ERROR_INVALID_FUNCTION,
    ERROR_ACCESS_DENIED,
    ERROR_NOT_SUPPORTED,
)


def _log(level: str, message: str) -> None:
    print(f"{level}: {message}", file=sys.stderr)


def _flush_windows_directory(directory: Path) -> bool:
    """Flush a directory handle on Windows. Return False when flush is omitted."""
    import ctypes
    from ctypes import wintypes

    generic_read = 0x80000000
    file_share_read = 0x00000001
    file_share_write = 0x00000002
    file_share_delete = 0x00000004
    open_existing = 3
    file_flag_backup_semantics = 0x02000000
    invalid_handle = wintypes.HANDLE(-1).value
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    create_file_w = kernel32.CreateFileW
    create_file_w.argtypes = [
        wintypes.LPCWSTR,
        wintypes.DWORD,
        wintypes.DWORD,
        wintypes.LPVOID,
        wintypes.DWORD,
        wintypes.DWORD,
        wintypes.HANDLE,
    ]
    create_file_w.restype = wintypes.HANDLE
    flush_file_buffers = kernel32.FlushFileBuffers
    flush_file_buffers.argtypes = [wintypes.HANDLE]
    flush_file_buffers.restype = wintypes.BOOL
    close_handle = kernel32.CloseHandle
    close_handle.argtypes = [wintypes.HANDLE]
    close_handle.restype = wintypes.BOOL
    handle = create_file_w(
        str(directory),
        generic_read,
        file_share_read | file_share_write | file_share_delete,
        None,
        open_existing,
        file_flag_backup_semantics,
        None,
    )
    if handle == invalid_handle:
        error = ctypes.get_last_error()
        if error in WINDOWS_DIRECTORY_FLUSH_OMIT_ERRORS:
            return False
        raise OSError(None, "CreateFileW failed for directory flush", directory.name, error)
    try:
        if not flush_file_buffers(handle):
            error = ctypes.get_last_error()
            if error in WINDOWS_DIRECTORY_FLUSH_OMIT_ERRORS:
                return False
            raise OSError(None, "FlushFileBuffers failed for directory", directory.name, error)
        return True
    finally:
        close_handle(handle)


def _is_unsupported_directory_flush(error: OSError) -> bool:
    winerror = getattr(error, "winerror", None)
    if winerror in WINDOWS_DIRECTORY_FLUSH_OMIT_ERRORS:
        return True
    if error.errno in (errno.EACCES, errno.EINVAL, errno.ENOTSUP) or error.errno is None:
        return True
    # POSIX hosts leave winerror unset; Windows codes arrive as OSError args[3].
    if len(error.args) >= 4 and error.args[3] in WINDOWS_DIRECTORY_FLUSH_OMIT_ERRORS:
        return True
    return False


def flush_parent_directory(directory: Path) -> str:
    """Flush the parent directory. Returns performed|omitted."""
    if os.name == "nt":
        try:
            performed = _flush_windows_directory(directory)
        except OSError as error:
            if _is_unsupported_directory_flush(error):
                _log("Warn", "unsupported Windows directory flush omitted after file fsync")
                return "omitted"
            raise
        if performed:
            return "performed"
        _log("Warn", "unsupported Windows directory flush omitted after file fsync")
        return "omitted"
    directory_fd = os.open(directory, os.O_RDONLY)
    try:
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)
    return "performed"


def atomic_replace(path: Path, data: bytes) -> str:
    original_mode = stat.S_IMODE(path.stat().st_mode)
    temporary_name: str | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="wb",
            dir=path.parent,
            prefix=f".{path.name}.",
            suffix=".tmp",
            delete=False,
        ) as handle:
            temporary_name = handle.name
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(temporary_name, original_mode)
        os.replace(temporary_name, path)
        temporary_name = None
        return flush_parent_directory(path.parent)
    finally:
        if temporary_name is not None:
            try:
                os.unlink(temporary_name)
            except FileNotFoundError:
                pass


def synchronize(manifest: Path, lockfile: Path, package_name: str) -> None:
    if not package_name or package_name != package_name.strip():
        raise SyncError("--package must be non-empty and trimmed")
    version = manifest_package_version(manifest)
    original = lockfile.read_bytes()
    updated = synchronized_bytes(original, package_name, version)
    directory_flush = "omitted"
    version_changed = updated != original
    if version_changed:
        directory_flush = atomic_replace(lockfile, updated)
    _log(
        "Info",
        f"platform={os.name} directory_flush={directory_flush} version_changed={str(version_changed).lower()}",
    )
    if lockfile.read_bytes() != updated:
        raise SyncError("post-replacement Cargo.lock verification failed")


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        synchronize(args.manifest, args.lockfile, args.package)
    except (OSError, SyncError, tomllib.TOMLDecodeError, UnicodeError) as error:
        print(f"sync-stamped-root-lock: {error}", file=sys.stderr)
        if isinstance(error, OSError):
            code = getattr(error, "winerror", None) or error.errno
            _log("Error", f"SYNC_REPLACE_FAILED code={code} root={args.lockfile.name}")
        else:
            _log("Error", "SYNC_FAILED")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
