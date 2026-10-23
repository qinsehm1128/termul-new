#!/usr/bin/env python3
"""Read-only pre-main initializer audit for regular Ghostty VT static archives.

Python 3.11+, stdlib only. Supports Mach-O64, ELF32/64 and ordinary COFF
relocatable objects, not thin/fat archives, bitcode, COFF bigobj/import objects,
or executables. Unknown members fail closed. This is artifact evidence, not a
sandbox: it does not prove arbitrary code cannot initialize itself at runtime.
"""

import argparse
from pathlib import Path
import re
import struct
import sys

MAX_ARCHIVE_BYTES = 512 * 1024 * 1024


class AuditError(ValueError):
    pass


class Reader:
    def __init__(self, data):
        self.data = memoryview(data)

    def span(self, offset, size):
        if offset < 0 or size < 0 or offset > len(self.data) or size > len(self.data) - offset:
            raise AuditError(f"out-of-bounds range: offset={offset}, size={size}, length={len(self.data)}")
        return self.data[offset:offset + size]

    def unpack(self, fmt, offset):
        return struct.unpack(fmt, self.span(offset, struct.calcsize(fmt)))


def cstring(table, offset):
    if offset < 0 or offset >= len(table):
        raise AuditError(f"string offset out of bounds: {offset}")
    end = table.find(b"\0", offset)
    if end < 0:
        raise AuditError("unterminated string")
    return table[offset:end].decode("ascii", errors="backslashreplace")


def reject_section(name, size, kind=False):
    suspicious = (
        name in {"__mod_init_func", "__mod_term_func", "__objc_nlclslist", "__objc_nlcatlist", "__objc_init_func"}
        or name.startswith(".CRT$X")
        or any(name == base or name.startswith(base + ".") for base in
               (".init", ".fini", ".init_array", ".preinit_array", ".fini_array", ".ctors", ".dtors"))
    )
    if size and (kind or suspicious):
        raise AuditError(f"initializer/terminator section {name!r} is nonempty ({size} bytes)")


def reject_symbol(name):
    # Strip platform C prefixes, but retain MSVC's ??__E/??__F spelling.
    plain = name.lstrip("_")
    if (re.match(r"(?:DllMain(?:CRTStartup)?|CRT_INIT)(?:@\d+)?$", plain, re.IGNORECASE)
            or re.match(r"GLOBAL__(?:sub_[ID]_|[ID]_)", plain)
            or plain.startswith(("cxx_global_var_init", "static_initialization_and_destruction_0"))
            or re.match(r"\?DllMain(?:CRTStartup)?@@", name, re.IGNORECASE)
            or re.match(r"Z\d+_*static_initialization_and_destruction_0", plain)
            or name.startswith(("??__E", "??__F"))):
        raise AuditError(f"initializer/terminator symbol {name!r}")


def macho(r):
    magic = bytes(r.span(0, 4))
    endian = {b"\xcf\xfa\xed\xfe": "<", b"\xfe\xed\xfa\xcf": ">"}[magic]
    _, _, _, filetype, ncmds, cmdsize, _, _ = r.unpack(endian + "8I", 0)
    if filetype != 1:
        raise AuditError("unsupported Mach-O: expected MH_OBJECT")
    r.span(32, cmdsize)
    if ncmds > cmdsize // 8:
        raise AuditError("Mach-O command count exceeds command table")
    pos, end = 32, 32 + cmdsize
    for _ in range(ncmds):
        cmd, size = r.unpack(endian + "II", pos)
        if size < 8 or size % 8 or size > end - pos:
            raise AuditError("invalid Mach-O load command size")
        if cmd == 0x19:  # LC_SEGMENT_64
            if size < 72:
                raise AuditError("truncated Mach-O segment")
            _, _, _, _, _, off, length, _, _, count, _ = r.unpack(endian + "II16sQQQQIIII", pos)
            r.span(off, length)
            if count > (size - 72) // 80:
                raise AuditError("Mach-O section count exceeds segment")
            for i in range(count):
                fields = r.unpack(endian + "16s16sQQ8I", pos + 72 + i * 80)
                name = fields[0].split(b"\0", 1)[0].decode("ascii", errors="backslashreplace")
                length, off, reloff, nreloc, flags = fields[3], fields[4], fields[6], fields[7], fields[8]
                sectype = flags & 0xff
                reject_section(name, length, sectype in (9, 10, 0x16))
                if sectype not in (1, 0xc, 0x12):  # zero-fill sections have no file payload
                    r.span(off, length)
                r.span(reloff, nreloc * 8)
        elif cmd == 2:  # LC_SYMTAB
            if size != 24:
                raise AuditError("invalid Mach-O symbol command")
            _, _, off, count, stroff, strsize = r.unpack(endian + "6I", pos)
            r.span(off, count * 16)
            strings = bytes(r.span(stroff, strsize))
            for i in range(count):
                name, typ, _, desc, _ = r.unpack(endian + "IBBHQ", off + i * 16)
                text = cstring(strings, name)
                if not typ & 0xe0 and typ & 0x0e:  # defined, non-debug symbol
                    if desc & 0x100:  # N_SYMBOL_RESOLVER
                        raise AuditError(f"unsupported Mach-O initializer/symbol resolver {text!r}")
                    reject_symbol(text)
        elif cmd == 0x1a:  # LC_ROUTINES_64
            if size != 72:
                raise AuditError("invalid Mach-O routines command")
            if r.unpack(endian + "Q", pos + 8)[0]:
                raise AuditError("Mach-O LC_ROUTINES_64 initializer")
        pos += size
    if pos != end:
        raise AuditError("Mach-O command table size mismatch")
    return "Mach-O64"


def elf(r):
    ident = bytes(r.span(0, 16))
    if ident[4] not in (1, 2) or ident[5] not in (1, 2) or ident[6] != 1:
        raise AuditError("unsupported ELF class/encoding/version")
    wide = ident[4] == 2
    endian = "<" if ident[5] == 1 else ">"
    headerfmt = endian + ("HHIQQQIHHHHHH" if wide else "HHIIIIIHHHHHH")
    h = r.unpack(headerfmt, 16)
    if h[0] != 1 or h[2] != 1 or h[7] != (64 if wide else 52) or h[9] != 0:
        raise AuditError("unsupported ELF: expected relocatable object without program headers")
    shoff, entsize, count, names = h[5], h[10], h[11], h[12]
    sectionfmt = endian + ("IIQQQQIIQQ" if wide else "10I")
    if not shoff or entsize != struct.calcsize(sectionfmt):
        raise AuditError("invalid ELF section table")
    first = r.unpack(sectionfmt, shoff)
    count = count or first[5]
    names = first[6] if names == 0xffff else names
    if not count or names >= count or names == 0:
        raise AuditError("invalid ELF section/name count")
    r.span(shoff, count * entsize)
    sections = [r.unpack(sectionfmt, shoff + i * entsize) for i in range(count)]
    for s in sections:
        if s[1] not in (0, 8):
            r.span(s[4], s[5])
        if s[2] & 0x800:
            raise AuditError("unsupported compressed ELF section")
    ns = sections[names]
    if ns[1] != 3:
        raise AuditError("ELF section names are not a string table")
    strings = bytes(r.span(ns[4], ns[5]))
    for s in sections:
        reject_section(cstring(strings, s[0]), s[5], s[1] in (14, 15, 16))
        if s[1] in (2, 11):
            symfmt = endian + ("IBBHQQ" if wide else "IIIBBH")
            symsize = struct.calcsize(symfmt)
            if s[9] != symsize or s[5] % symsize or s[6] >= count or sections[s[6]][1] != 3:
                raise AuditError("invalid ELF symbol/string table")
            st = sections[s[6]]
            symbolnames = bytes(r.span(st[4], st[5]))
            for off in range(s[4], s[4] + s[5], symsize):
                sym = r.unpack(symfmt, off)
                index = sym[3] if wide else sym[5]
                if index == 0xffff:
                    raise AuditError("unsupported ELF extended symbol section index")
                if index and index < 0xff00 and index >= count:
                    raise AuditError("ELF symbol section index out of bounds")
                text = cstring(symbolnames, sym[0])
                if index:
                    info = sym[1] if wide else sym[3]
                    if info & 0xf == 10:  # STT_GNU_IFUNC: address relocation may run resolver pre-main
                        raise AuditError(f"unsupported ELF IFUNC initializer/resolver {text!r}")
                    reject_symbol(text)
    return "ELF64" if wide else "ELF32"


def coff(r):
    machine, count, _, symoff, nsyms, optional, _ = r.unpack("<HHIIIHH", 0)
    if machine not in (0x14c, 0x8664, 0xaa64) or optional or not count:
        raise AuditError("unsupported COFF: expected ordinary relocatable object (not bigobj/import/PE)")
    r.span(20, count * 40)
    strings = b""
    if symoff:
        r.span(symoff, nsyms * 18)
        start = symoff + nsyms * 18
        length = r.unpack("<I", start)[0]
        if length < 4:
            raise AuditError("invalid COFF string table size")
        strings = bytes(r.span(start, length))
    elif nsyms:
        raise AuditError("COFF symbol count without table")

    def name(raw, section=False):
        if section and raw.startswith(b"/"):
            number = raw.rstrip(b"\0")[1:]
            if not number.isdigit():
                raise AuditError("unsupported COFF long section name")
            offset = int(number)
        elif not section and raw[:4] == b"\0" * 4:
            offset = struct.unpack("<I", raw[4:])[0]
        else:
            return raw.split(b"\0", 1)[0].decode("ascii", errors="backslashreplace")
        if offset < 4:
            raise AuditError("invalid COFF string offset")
        return cstring(strings, offset)

    for i in range(count):
        raw, virtual, _, size, off, reloc, lines, nreloc, nlines, flags = r.unpack("<8s6I2HI", 20 + i * 40)
        reject_section(name(raw, True), max(virtual, size))
        if flags & 0x1000000:
            raise AuditError("unsupported COFF extended relocation count")
        if size and (off or not flags & 0x80):
            if not off:
                raise AuditError("COFF initialized section has no data offset")
            r.span(off, size)
        r.span(reloc, nreloc * 10)
        r.span(lines, nlines * 6)
    i = 0
    while i < nsyms:
        raw, _, section, _, _, aux = r.unpack("<8sIhHBB", symoff + i * 18)
        text = name(raw)
        if section > count or section < -2 or aux > nsyms - i - 1:
            raise AuditError("invalid COFF symbol section/auxiliary count")
        if section > 0 or section == -1:
            reject_symbol(text)
        i += 1 + aux
    return "COFF"


def audit_object(data):
    r = Reader(data)
    magic = bytes(r.span(0, min(4, len(r.data))))
    if magic in (b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xcf"):
        return macho(r)
    if magic == b"\x7fELF":
        return elf(r)
    if len(r.data) >= 2 and r.unpack("<H", 0)[0] in (0x14c, 0x8664, 0xaa64):
        return coff(r)
    raise AuditError(f"unsupported object format (magic {magic.hex()}); bitcode, bigobj/import and fat objects are not audited")


def audit_archive(path):
    path = Path(path)
    if path.suffix.lower() not in (".a", ".lib"):
        raise AuditError("library must be an explicit .a or .lib archive")
    if path.stat().st_size > MAX_ARCHIVE_BYTES:
        raise AuditError(f"archive exceeds {MAX_ARCHIVE_BYTES}-byte limit")
    with path.open("rb") as stream:
        data = stream.read(MAX_ARCHIVE_BYTES + 1)
    if len(data) > MAX_ARCHIVE_BYTES:
        raise AuditError("archive exceeds size limit")
    r = Reader(data)
    if bytes(r.span(0, 8)) != b"!<arch>\n":
        raise AuditError("unsupported archive: expected regular ar (not thin)")
    pos, objects, formats, longnames = 8, 0, set(), b""
    while pos < len(data):
        header = bytes(r.span(pos, 60))
        if header[58:] != b"`\n" or not header[48:58].strip().isdigit():
            raise AuditError("malformed ar member header/size")
        size = int(header[48:58])
        payload = r.span(pos + 60, size)
        rawname = header[:16].rstrip(b" ")
        if rawname == b"//":
            longnames = bytes(payload)
        elif rawname not in (b"/", b"/SYM64/"):
            if rawname.startswith(b"#1/"):
                if not rawname[3:].isdigit():
                    raise AuditError("malformed BSD extended name")
                length = int(rawname[3:])
                if length > size:
                    raise AuditError("BSD extended name exceeds member")
                name = bytes(payload[:length]).rstrip(b"\0")
                payload = payload[length:]
            elif rawname.startswith(b"/"):
                if not rawname[1:].isdigit():
                    raise AuditError("invalid GNU ar name reference")
                offset = int(rawname[1:])
                if offset >= len(longnames) or (offset and longnames[offset - 1] not in (0, 10)):
                    raise AuditError("ar name offset out of bounds/not an entry")
                # GNU uses /\n; Microsoft COFF archives use NUL terminators.
                ends = [end for end in (longnames.find(b"/\n", offset), longnames.find(b"\0", offset)) if end >= 0]
                if not ends:
                    raise AuditError("unterminated ar long name")
                name = longnames[offset:min(ends)]
            else:
                name = rawname.removesuffix(b"/")
            if name not in (b"__.SYMDEF", b"__.SYMDEF SORTED", b"__.SYMDEF_64", b"__.SYMDEF_64 SORTED"):
                if not name:
                    raise AuditError("empty ar member name")
                try:
                    formats.add(audit_object(payload))
                except AuditError as exc:
                    raise AuditError(f"{name.decode('ascii', errors='backslashreplace')}: {exc}") from exc
                objects += 1
        pos += 60 + size
        if size % 2:
            if bytes(r.span(pos, 1)) != b"\n":
                raise AuditError("missing/invalid ar alignment padding")
            pos += 1
    if not objects:
        raise AuditError("archive contains no auditable objects")
    return objects, sorted(formats)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--library", action="append", type=Path, default=[], help="explicit .a/.lib; repeatable")
    parser.add_argument("--cargo-target-dir", type=Path, help="also audit every out/native/lib Ghostty archive under this root")
    args = parser.parse_args(argv)
    try:
        libraries = list(args.library)
        if args.cargo_target_dir is not None:
            found = sorted(p for p in args.cargo_target_dir.rglob("*")
                           if p.name in ("libghostty-vt.a", "ghostty-vt-static.lib")
                           and p.parts[-4:-1] == ("out", "native", "lib") and p.is_file())
            if not found:
                raise AuditError(f"no Ghostty VT archives under {args.cargo_target_dir}/**/out/native/lib")
            libraries.extend(found)
        if not libraries:
            raise AuditError("provide --library or --cargo-target-dir")
        for path in dict.fromkeys(p.resolve() for p in libraries):
            objects, formats = audit_archive(path)
            print(f"PASS {path}: {objects} objects ({', '.join(formats)}); no initializer/terminator evidence")
        return 0
    except (AuditError, OSError) as exc:
        print(f"FAIL passive-link audit: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
