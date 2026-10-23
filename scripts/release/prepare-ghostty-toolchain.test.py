#!/usr/bin/env python3
"""Offline tests for the checksum-pinned Ghostty toolchain installer."""

import hashlib
from contextlib import redirect_stdout
import importlib.util
import io
import os
import sys
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

sys.dont_write_bytecode = True

SPEC = importlib.util.spec_from_file_location(
    "ghostty_toolchain", Path(__file__).with_name("prepare-ghostty-toolchain.py")
)
toolchain = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(toolchain)


class ToolchainTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=os.environ.get("QIN_CODE_SCRATCH_DIR"))
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def test_supported_hosts_are_exact_and_versions_are_pinned(self):
        self.assertEqual(toolchain.host_platform("Darwin", "arm64"), "aarch64-macos")
        self.assertEqual(toolchain.host_platform("Darwin", "x86_64"), "x86_64-macos")
        self.assertEqual(toolchain.host_platform("Windows", "AMD64"), "x86_64-windows")
        self.assertEqual(toolchain.host_platform("Linux", "x86_64"), "x86_64-linux")
        with self.assertRaisesRegex(ValueError, "unsupported"):
            toolchain.host_platform("Linux", "aarch64")
        self.assertEqual(toolchain.VERSION, "0.16.0")
        for key in toolchain.RELEASES:
            root, name, size, digest = toolchain.release_info(key)
            self.assertTrue(root.endswith("-0.16.0"))
            self.assertTrue(name.startswith(root))
            self.assertGreater(size, 0)
            self.assertRegex(digest, r"^[0-9a-f]{64}$")

    def test_size_and_hash_are_both_required_for_cached_archives(self):
        archive = self.root / "archive"
        archive.write_bytes(b"fixed archive")
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        toolchain.verify_archive(archive, 13, digest)
        with self.assertRaisesRegex(ValueError, "size"):
            toolchain.verify_archive(archive, 12, digest)
        with self.assertRaisesRegex(ValueError, "SHA256"):
            toolchain.verify_archive(archive, 13, "0" * 64)

    def test_download_is_https_pinned_and_published_only_after_verification(self):
        payload = b"verified archive"
        digest = hashlib.sha256(payload).hexdigest()
        with patch.object(toolchain.urllib.request, "urlopen", return_value=io.BytesIO(payload)) as fetch:
            archive = toolchain.download_archive(self.root, "zig-test.zip", len(payload), digest)
        self.assertEqual(archive.read_bytes(), payload)
        self.assertEqual(fetch.call_args.args[0], "https://ziglang.org/download/0.16.0/zig-test.zip")
        self.assertEqual(list(self.root.glob("zig-download-*")), [])
        with patch.object(toolchain.urllib.request, "urlopen") as fetch:
            self.assertEqual(toolchain.download_archive(self.root, archive.name, len(payload), digest), archive)
            fetch.assert_not_called()

    def test_bad_download_leaves_no_archive_or_partial(self):
        with patch.object(toolchain.urllib.request, "urlopen", return_value=io.BytesIO(b"wrong")):
            with self.assertRaisesRegex(ValueError, "SHA256"):
                toolchain.download_archive(self.root, "bad.zip", 5, "0" * 64)
        self.assertEqual(list(self.root.iterdir()), [])

    def test_all_download_failures_close_handles_before_windows_unlink(self):
        original_temp = toolchain.tempfile.NamedTemporaryFile
        original_unlink = Path.unlink

        class BrokenRead(io.BytesIO):
            def read(self, _size=-1):
                raise OSError("interrupted read")

        for failure in ["network", "read", "oversize"]:
            with self.subTest(failure=failure):
                handles = []

                def record_temp(*args, **kwargs):
                    handle = original_temp(*args, **kwargs)
                    handles.append(handle)
                    return handle

                def windows_unlink(path, *args, **kwargs):
                    if path.name.startswith("zig-download-"):
                        self.assertTrue(handles[-1].closed, "Windows cannot unlink an open file")
                    return original_unlink(path, *args, **kwargs)

                fetch = {"side_effect": OSError("connection failed")} if failure == "network" else {
                    "return_value": BrokenRead() if failure == "read" else io.BytesIO(b"oversize")
                }
                with patch.object(toolchain.tempfile, "NamedTemporaryFile", side_effect=record_temp), \
                     patch.object(toolchain.urllib.request, "urlopen", **fetch), \
                     patch.object(Path, "unlink", new=windows_unlink):
                    with self.assertRaises((OSError, ValueError)):
                        toolchain.download_archive(self.root, "bad.zip", 1, "0" * 64)
                self.assertEqual(list(self.root.iterdir()), [])

    def test_unsupported_python_fails_before_download_or_cache_creation(self):
        cache = self.root / "must-not-exist"
        with patch.object(toolchain.tarfile, "data_filter", new=None, create=True), \
             patch.object(toolchain, "download_archive") as fetch:
            with self.assertRaisesRegex(RuntimeError, "Python 3.12"):
                toolchain.install(cache, "aarch64-macos")
            fetch.assert_not_called()
        self.assertFalse(cache.exists())

    def test_path_validation_rejects_traversal_and_foreign_roots(self):
        for name in ["../escape", "/absolute", "root/../escape", "root\\escape", "other/file", "C:/root/file"]:
            with self.subTest(name=name), self.assertRaises(ValueError):
                toolchain.validate_member(name, "root")
        toolchain.validate_member("root/lib/header.h", "root")

    def test_extracts_only_expected_archive_root(self):
        archive = self.root / "zig.tar.xz"
        with tarfile.open(archive, "w:xz") as bundle:
            member = tarfile.TarInfo("root/zig")
            member.size = 3
            bundle.addfile(member, io.BytesIO(b"zig"))
        output = self.root / "out"
        output.mkdir()
        toolchain.extract_archive(archive, output, "root")
        self.assertEqual((output / "root/zig").read_bytes(), b"zig")

    def test_zip_symlink_is_rejected(self):
        archive = self.root / "zig.zip"
        with zipfile.ZipFile(archive, "w") as bundle:
            member = zipfile.ZipInfo("root/zig")
            member.external_attr = 0o120777 << 16
            bundle.writestr(member, "/elsewhere")
        with self.assertRaisesRegex(ValueError, "symlink"):
            toolchain.extract_archive(archive, self.root / "out", "root")

    def test_existing_unverified_installation_is_never_deleted_or_executed(self):
        name, _, _, _ = toolchain.release_info("aarch64-macos")
        target = self.root / name
        target.mkdir()
        sentinel = target / "keep"
        sentinel.write_text("user data")
        with patch.object(toolchain, "download_archive"), patch.object(toolchain, "verify_binary") as execute:
            with self.assertRaisesRegex(ValueError, "unverified"):
                toolchain.install(self.root, "aarch64-macos")
            execute.assert_not_called()
        self.assertEqual(sentinel.read_text(), "user data")

    def test_cli_can_return_a_verified_path_without_shell_evaluation(self):
        binary = self.root / "path with spaces" / "zig"
        output = io.StringIO()
        argv = ["prepare", "--cache-dir", str(self.root), "--print-executable"]
        with patch.object(sys, "argv", argv), patch.object(toolchain, "install", return_value=binary), \
             redirect_stdout(output):
            toolchain.main()
        self.assertEqual(output.getvalue(), f"{binary}\n")

    def test_github_exports_native_wrapper_variable_and_private_caches(self):
        binary = self.root / "toolchain" / "zig"
        environment_file = self.root / "github-env"
        argv = ["prepare", "--cache-dir", str(self.root), "--github-env", str(environment_file)]
        with patch.object(sys, "argv", argv), patch.object(toolchain, "install", return_value=binary), \
             redirect_stdout(io.StringIO()):
            toolchain.main()
        environment = dict(line.split("=", 1) for line in environment_file.read_text().splitlines())
        self.assertEqual(environment["SE_GHOSTTY_VT_ZIG"], str(binary))
        self.assertEqual(environment["ZIG"], str(binary))
        self.assertTrue(environment["ZIG_GLOBAL_CACHE_DIR"].startswith(str(self.root)))


if __name__ == "__main__":
    unittest.main()
