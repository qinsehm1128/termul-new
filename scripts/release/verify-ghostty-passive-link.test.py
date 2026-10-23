#!/usr/bin/env python3
"""Offline stdlib tests; optional clang checks create real cross-target objects."""

from contextlib import redirect_stderr, redirect_stdout
import importlib.util
import io
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location("passive_link", Path(__file__).with_name("verify-ghostty-passive-link.py"))
audit = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(audit)


def ar(*members):
    result = b"!<arch>\n"
    for name, payload in members:
        header = f"{name:<16}{0:<12}{0:<6}{0:<6}{'100644':<8}{len(payload):<10}`\n".encode()
        assert len(header) == 60
        result += header + payload + (b"\n" if len(payload) % 2 else b"")
    return result


def macho(section=b"__text", flags=0, symbol=b"_passive", defined=True, endian="<"):
    strings = b"\0" + symbol + b"\0"
    header = struct.pack(endian + "8I", 0xfeedfacf, 0x100000c, 0, 1, 2, 176, 0, 0)
    segment = struct.pack(endian + "II16sQQQQIIII", 0x19, 152, b"", 0, 8, 208, 8, 7, 7, 1, 0)
    section = struct.pack(endian + "16s16sQQ8I", section, b"__DATA", 0, 8, 208, 3, 0, 0, flags, 0, 0, 0)
    symtab = struct.pack(endian + "6I", 2, 24, 216, 1, 232, len(strings))
    symbol = struct.pack(endian + "IBBHQ", 1, 0xf if defined else 1, 1 if defined else 0, 0, 0)
    return header + segment + section + symtab + b"\0" * 8 + symbol + strings


def elf(section=b".text", wide=True, endian="<", symbol=b"passive", defined=True, sectype=1):
    shfmt = endian + ("IIQQQQIIQQ" if wide else "10I")
    symfmt = endian + ("IBBHQQ" if wide else "IIIBBH")
    hsize, shsize, symsize = (64, 64, 24) if wide else (52, 40, 16)
    names = b"\0" + section + b"\0.shstrtab\0.strtab\0.symtab\0"
    strings = b"\0" + symbol + b"\0"
    payload = hsize + 5 * shsize
    nameoff, stroff = payload + 8, payload + 8 + len(names)
    symoff = stroff + len(strings)
    ident = b"\x7fELF" + bytes([2 if wide else 1, 1 if endian == "<" else 2, 1]) + b"\0" * 9
    header = struct.pack(endian + ("HHIQQQIHHHHHH" if wide else "HHIIIIIHHHHHH"),
                         1, 62 if wide else 3, 1, 0, 0, hsize, 0, hsize, 0, 0, shsize, 5, 2)
    sections = [(0,) * 10,
                (1, sectype, 0, 0, payload, 8, 0, 0, 1, 0),
                (len(section) + 2, 3, 0, 0, nameoff, len(names), 0, 0, 1, 0),
                (len(section) + 12, 3, 0, 0, stroff, len(strings), 0, 0, 1, 0),
                (len(section) + 20, 2, 0, 0, symoff, symsize, 3, 0, 1, symsize)]
    sym = (1, 0x12, 0, int(defined), 0, 0) if wide else (1, 0, 0, 0x12, 0, int(defined))
    return ident + header + b"".join(struct.pack(shfmt, *s) for s in sections) + b"\0" * 8 + names + strings + struct.pack(symfmt, *sym)


def coff(section=b".text", symbol=b"passive", defined=True):
    # Always use long-name references, exercising the COFF string table.
    strings = struct.pack("<I", 4 + len(section) + 1 + len(symbol) + 1) + section + b"\0" + symbol + b"\0"
    header = struct.pack("<HHIIIHH", 0x8664, 1, 0, 68, 1, 0, 0)
    section = struct.pack("<8s6I2HI", b"/4", 0, 0, 8, 60, 0, 0, 0, 0, 0x40)
    symname = struct.pack("<II", 0, 4 + len(strings[4:].split(b"\0", 1)[0]) + 1)
    sym = struct.pack("<8sIhHBB", symname, 0, int(defined), 0, 2, 0)
    return header + section + b"\0" * 8 + sym + strings


class PassiveLinkTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=os.environ.get("QIN_CODE_SCRATCH_DIR"))
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def library(self, data, name="libghostty-vt.a"):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        return path

    def cli(self, *args):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = audit.main([str(arg) for arg in args])
        return code, out.getvalue(), err.getvalue()

    def test_pass_all_formats_and_endianness(self):
        for obj, expected in [(macho(), "Mach-O64"), (macho(endian=">"), "Mach-O64"),
                              (elf(), "ELF64"), (elf(wide=False), "ELF32"),
                              (elf(endian=">"), "ELF64"), (elf(wide=False, endian=">"), "ELF32"),
                              (coff(), "COFF")]:
            with self.subTest(expected=expected, magic=obj[:4]):
                path = self.library(ar(("passive.o/", obj)))
                self.assertEqual(audit.audit_archive(path), (1, [expected]))

    def test_all_initializer_sections_and_type_flags(self):
        objects = [macho(section=name) for name in
                   (b"__mod_init_func", b"__mod_term_func", b"__objc_nlclslist", b"__objc_nlcatlist", b"__objc_init_func")]
        objects += [macho(flags=flag) for flag in (9, 10, 0x16)]
        objects += [elf(section=name, wide=wide) for name in
                    (b".init", b".fini", b".init_array", b".preinit_array", b".fini_array", b".ctors", b".dtors", b".init_array.101")
                    for wide in (False, True)]
        objects += [elf(sectype=typ) for typ in (14, 15, 16)]
        objects += [coff(section=name) for name in (b".CRT$XCU", b".CRT$XIA", b".CRT$XTZ")]
        for obj in objects:
            with self.subTest(magic=obj[:4], size=len(obj)):
                with self.assertRaisesRegex(audit.AuditError, "section.*nonempty"):
                    audit.audit_archive(self.library(ar(("bad.o/", obj))))

    def test_defined_ctor_symbols_rejected_but_undefined_references_allowed(self):
        for builder in (macho, elf, coff):
            for symbol in (b"DllMain", b"_DllMain@12", b"DllMainCRTStartup", b"_CRT_INIT",
                           b"_GLOBAL__sub_I_source.c", b"__GLOBAL__sub_D_source.c", b"_GLOBAL__I_x",
                           b"_Z41__static_initialization_and_destruction_0ii", b"?DllMain@@YAHPEAXI0@Z",
                           b"__cxx_global_var_init", b"__static_initialization_and_destruction_0", b"??__Eglobal@@YAXXZ", b"??__Fglobal@@YAXXZ"):
                with self.subTest(builder=builder.__name__, symbol=symbol):
                    with self.assertRaisesRegex(audit.AuditError, "symbol"):
                        audit.audit_object(builder(symbol=symbol))
                    audit.audit_object(builder(symbol=symbol, defined=False))

    def test_symbol_resolvers_fail_closed(self):
        for wide in (False, True):
            for endian in ("<", ">"):
                obj = bytearray(elf(wide=wide, endian=endian))
                symsize = 24 if wide else 16
                obj[-symsize + (4 if wide else 12)] = 0x1a
                with self.assertRaisesRegex(audit.AuditError, "unsupported ELF IFUNC"):
                    audit.audit_object(obj)
        obj = bytearray(macho())
        struct.pack_into("<H", obj, 222, 0x100)
        with self.assertRaisesRegex(audit.AuditError, "unsupported Mach-O.*resolver"):
            audit.audit_object(obj)

    def test_ar_indexes_bsd_and_gnu_names(self):
        bsdname = b"a-very-long-bsd-object.o\0\0"
        data = ar(("/", b"index"), ("/SYM64/", b"index"),
                  ("#1/20", b"__.SYMDEF SORTED\0\0\0\0\0" + b"index"),
                  ("//", b"long-object-name.o/\n"), ("/0", elf()),
                  (f"#1/{len(bsdname)}", bsdname + macho()))
        self.assertEqual(audit.audit_archive(self.library(data)), (2, ["ELF64", "Mach-O64"]))
        names = b"windows-long-object.obj\0second-long-object.obj\0"
        data = ar(("/", b"first index"), ("/", b"second index"), ("//", names),
                  ("/0", coff()), (f"/{names.index(b'second')}", coff()))
        self.assertEqual(audit.audit_archive(self.library(data, "ghostty-vt.lib")), (2, ["COFF"]))

    def test_empty_initializer_sections_are_allowed(self):
        obj = bytearray(macho(section=b"__mod_init_func"))
        struct.pack_into("<Q", obj, 144, 0)
        audit.audit_object(obj)
        obj = bytearray(elf(section=b".init_array"))
        struct.pack_into("<Q", obj, 64 + 64 + 32, 0)
        audit.audit_object(obj)
        obj = bytearray(coff(section=b".CRT$XCU"))
        struct.pack_into("<I", obj, 36, 0)
        audit.audit_object(obj)

    def test_unknown_and_empty_members_fail_closed(self):
        for obj in (b"", b"BC\xc0\xde", b"\0\0\xff\xff" + b"\0" * 40, b"\xca\xfe\xba\xbe", b"random text"):
            with self.subTest(obj=obj):
                with self.assertRaisesRegex(audit.AuditError, "unsupported object"):
                    audit.audit_archive(self.library(ar(("unknown.o/", obj))))
        for data in (b"!<arch>\n", ar(("/", b"index")), b"!<thin>\n"):
            with self.assertRaises(audit.AuditError):
                audit.audit_archive(self.library(data))

    def test_every_object_truncation_is_a_controlled_failure(self):
        for obj in (macho(), elf(), elf(wide=False), coff()):
            for end in range(len(obj)):
                with self.subTest(magic=obj[:4], end=end):
                    with self.assertRaises(audit.AuditError):
                        audit.audit_object(obj[:end])

    def test_oversized_offsets_counts_and_strings(self):
        # Mutate independent header/section/symbol fields, not just archive size.
        cases = [(macho(), "<I", 16, 0xffffffff), (macho(), "<I", 152, 0xffffffff),
                 (macho(), "<I", 160, 0xffffffff), (macho(), "<I", 216, 0xffffffff),
                 (elf(), "<Q", 40, 2**64 - 1), (elf(), "<Q", 64 + 64 + 24, 2**64 - 1),
                 (elf(), "<I", 64 + 4 * 64 + 40, 0xffffffff),
                 (elf(wide=False), "<I", 32, 0xffffffff),
                 (coff(), "<I", 8, 0xffffffff), (coff(), "<I", 12, 0xffffffff),
                 (coff(), "<I", 40, 0xffffffff), (coff(), "<I", 72, 0xffffffff),
                 (coff(), "<B", 85, 255)]
        for original, fmt, offset, value in cases:
            data = bytearray(original)
            struct.pack_into(fmt, data, offset, value)
            with self.subTest(magic=data[:4], offset=offset):
                with self.assertRaises(audit.AuditError):
                    audit.audit_object(data)

    def test_malformed_ar_and_name_offsets(self):
        good = ar(("x.o/", macho()))
        badsize = bytearray(good)
        badsize[56:58] = b"-1"
        for data in (good[:30], good[:-1], bytes(badsize), good[:66] + b"??" + good[68:],
                     ar(("#1/9999", b"short")), ar(("#1/no", b"short")),
                     ar(("//", b"long-name.o/\n"), ("/999", macho())),
                     ar(("//", b"long-name.o/\n"), ("/2", macho())),
                     ar(("//", b"unterminated"), ("/0", macho())),
                     ar(("/wat", macho())), ar(("x/", b"x"))[:-1]):
            with self.subTest(data=data[:70]):
                with self.assertRaises(audit.AuditError):
                    audit.audit_archive(self.library(data))

    def test_archive_limit_and_wrong_extension(self):
        path = self.library(ar(("x.o/", macho())))
        with patch.object(audit, "MAX_ARCHIVE_BYTES", 8):
            with self.assertRaisesRegex(audit.AuditError, "limit"):
                audit.audit_archive(path)
        with self.assertRaisesRegex(audit.AuditError, "explicit .a or .lib"):
            audit.audit_archive(self.library(b"", "library.so"))

    def test_cli_missing_inputs_and_no_discovery(self):
        for args in ((), ("--library", self.root / "missing.a"), ("--cargo-target-dir", self.root),
                     ("--cargo-target-dir", self.root / "missing")):
            with self.subTest(args=args):
                code, out, err = self.cli(*args)
                self.assertEqual(code, 1)
                self.assertEqual(out, "")
                self.assertIn("FAIL passive-link audit", err)

    def test_cli_discovers_all_and_does_not_mask_bad_archive(self):
        first = self.library(ar(("x.o/", macho())), "debug/build/pkg-one/out/native/lib/libghostty-vt.a")
        second = self.library(ar(("x.obj/", coff())), "x86_64-pc-windows-msvc/release/build/pkg-two/out/native/lib/ghostty-vt-static.lib")
        self.library(b"ignored", "elsewhere/libghostty-vt.a")
        code, out, err = self.cli("--cargo-target-dir", self.root, "--library", first)
        self.assertEqual((code, out.count("PASS"), err), (0, 2, ""))
        second.write_bytes(ar(("bad.obj/", coff(section=b".CRT$XCU"))))
        self.assertEqual(self.cli("--cargo-target-dir", self.root)[0], 1)
        self.assertEqual(self.cli("--library", first, "--cargo-target-dir", self.root / "absent")[0], 1)

    @unittest.skipUnless(shutil.which("clang"), "optional real-format check requires clang")
    def test_real_cross_target_clang_objects(self):
        # No headers, SDK, linker, ar, objdump or native execution required.
        source = self.root / "fixture.c"
        output = self.root / "fixture.o"
        for target in ("arm64-apple-macos11", "x86_64-unknown-linux-gnu", "i386-unknown-linux-gnu", "x86_64-pc-windows-msvc"):
            for constructor in (False, True):
                with self.subTest(target=target, constructor=constructor):
                    source.write_text("int passive(void) { return 42; }\n" +
                                      ("__attribute__((constructor)) static void start(void) { (void)passive(); }\n" if constructor else ""))
                    result = subprocess.run([shutil.which("clang"), "--target=" + target, "-c", str(source), "-o", str(output)],
                                            capture_output=True, text=True, check=False)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    path = self.library(ar(("fixture.o/", output.read_bytes())))
                    if constructor:
                        with self.assertRaisesRegex(audit.AuditError, "initializer/terminator"):
                            audit.audit_archive(path)
                    else:
                        self.assertEqual(audit.audit_archive(path)[0], 1)
        source.write_text('int value; static int implementation(void) { return value; }\n'
                          'static void *resolve(void) { value = 1; return implementation; }\n'
                          'int api(void) __attribute__((ifunc("resolve")));\n'
                          'int (*entry)(void) = api;\n')
        result = subprocess.run([shutil.which("clang"), "--target=x86_64-unknown-linux-gnu", "-c", str(source), "-o", str(output)],
                                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        with self.assertRaisesRegex(audit.AuditError, "unsupported ELF IFUNC"):
            audit.audit_archive(self.library(ar(("resolver.o/", output.read_bytes()))))


if __name__ == "__main__":
    unittest.main()
