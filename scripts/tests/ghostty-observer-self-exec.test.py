#!/usr/bin/env python3
"""Real native observer acceptance using an existing trusted host executable.

Runs the host's early child-only mode, never a mock parser. No PTY descriptor,
application config, network listener or renderer is needed for screen evidence.
"""

import argparse
import base64
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import time
import unittest

MAX_FRAME = 512 * 1024
BINARY = None


def frame(value):
    data = json.dumps(value, separators=(",", ":")).encode()
    return struct.pack(">I", len(data)) + data


def decode_frames(data):
    result = []
    while data:
        if len(data) < 4:
            raise AssertionError("partial observer response header")
        length = struct.unpack(">I", data[:4])[0]
        if not 0 < length <= MAX_FRAME or len(data) < length + 4:
            raise AssertionError("invalid observer response length")
        result.append(json.loads(data[4:4 + length]))
        data = data[4 + length:]
    return result


def output(seq, value):
    return {"type": "output", "eventSeq": seq,
            "dataBase64": base64.b64encode(value).decode()}


def run_child(requests=(), raw=None, extra_args=()):
    environment = {key: os.environ[key] for key in ["SystemRoot", "WINDIR"] if key in os.environ}
    started = time.perf_counter()
    result = subprocess.run(
        [str(BINARY), "--ghostty-vt-observer", *extra_args],
        input=raw if raw is not None else b"".join(map(frame, requests)),
        capture_output=True, timeout=5, env=environment,
    )
    # Private stdout is strictly protocol. Terminal output must not become
    # diagnostics, and no host service banner may run before child dispatch.
    responses = decode_frames(result.stdout)
    return result, responses, time.perf_counter() - started


class RealObserverTests(unittest.TestCase):
    def assert_success(self, requests):
        process, replies, _ = run_child(requests)
        self.assertEqual(process.returncode, 0, process.stderr.decode(errors="replace"))
        self.assertEqual(process.stderr, b"")
        self.assertEqual(replies[0]["type"], "hello")
        self.assertEqual(replies[0]["protocolVersion"], 1)
        self.assertEqual(replies[0]["ghosttyRevision"], "44f2a44")
        self.assertTrue(replies[0]["observerBuildId"])
        return replies

    def test_native_ansi_cursor_and_renderer_free_screen(self):
        replies = self.assert_success([
            {"type": "init", "cols": 80, "rows": 5},
            output(1, b"\x1b[31mprogress 10%\x1b[0m\rprogress 99%"),
            {"type": "read", "requestId": 1, "barrierSeq": 1, "maxLines": 5},
        ])
        self.assertEqual([reply["type"] for reply in replies], ["hello", "ack", "ack", "reply"])
        self.assertIn("progress 99%", replies[-1]["text"])
        self.assertNotIn("10%", replies[-1]["text"])
        self.assertNotIn("\x1b", replies[-1]["text"])
        self.assertEqual(replies[-1]["appliedSeq"], 1)

    def test_split_unicode_alternate_screen_and_resize_are_ordered(self):
        text = "中文".encode()
        replies = self.assert_success([
            {"type": "init", "cols": 80, "rows": 5},
            output(1, b"primary " + text[:2]), output(2, text[2:]),
            output(3, b"\x1b[?1049halternate"),
            {"type": "read", "requestId": 1, "barrierSeq": 3, "maxLines": 5},
            {"type": "resize", "eventSeq": 4, "cols": 100, "rows": 6},
            output(5, b"\x1b[?1049l"),
            {"type": "read", "requestId": 2, "barrierSeq": 5, "maxLines": 6},
        ])
        screens = [value for value in replies if value["type"] == "reply"]
        self.assertIn("alternate", screens[0]["text"])
        self.assertNotIn("primary", screens[0]["text"])
        self.assertIn("primary 中文", screens[1]["text"])
        self.assertNotIn("alternate", screens[1]["text"])
        self.assertEqual(screens[1]["appliedSeq"], 5)

    def test_terminal_queries_cannot_become_pty_writeback(self):
        replies = self.assert_success([
            {"type": "init", "cols": 80, "rows": 5},
            output(1, b"safe\x1b[c\x1b[6n\x1b]52;c;c2VjcmV0\x07\x07"),
            {"type": "read", "requestId": 1, "barrierSeq": 1, "maxLines": 5},
        ])
        self.assertEqual([reply["type"] for reply in replies], ["hello", "ack", "ack", "reply"])
        self.assertIn("safe", replies[-1]["text"])
        self.assertNotIn("secret", replies[-1]["text"])

    def test_malformed_helper_args_do_not_enter_normal_host(self):
        result, replies, _ = run_child(extra_args=("unexpected",))
        self.assertEqual(result.returncode, 64)
        self.assertEqual(replies, [])

    def test_frame_limit_sequence_and_read_barrier_fail_closed(self):
        for requests, raw in [
            ([], struct.pack(">I", MAX_FRAME + 1)),
            ([{"type": "init", "cols": 80, "rows": 5}, output(2, b"wrong")], None),
            ([{"type": "init", "cols": 80, "rows": 5},
              {"type": "read", "requestId": 1, "barrierSeq": 2, "maxLines": 5}], None),
            ([{"type": "init", "cols": 65535, "rows": 65535}], None),
        ]:
            with self.subTest(requests=requests, raw=raw):
                result, replies, _ = run_child(requests, raw=raw)
                self.assertEqual(result.returncode, 70)
                self.assertFalse(any(reply["type"] == "reply" for reply in replies))

    def test_stdout_text_and_line_budgets(self):
        requests = [{"type": "init", "cols": 1024, "rows": 256}]
        chunk = b"a" * 1000 + b"\r\n"
        requests.extend(output(i, chunk) for i in range(1, 301))
        requests.append({"type": "read", "requestId": 1, "barrierSeq": 300, "maxLines": 200})
        replies = self.assert_success(requests)
        self.assertLessEqual(len(replies[-1]["text"].encode()), 64 * 1024)
        self.assertTrue(replies[-1]["truncated"])


def main():
    global BINARY
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    args, remaining = parser.parse_known_args()
    BINARY = args.binary.expanduser().resolve(strict=True)
    unittest.main(argv=[sys.argv[0], *remaining])


if __name__ == "__main__":
    main()
