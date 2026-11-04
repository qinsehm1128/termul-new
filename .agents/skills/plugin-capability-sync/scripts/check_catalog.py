#!/usr/bin/env python3
"""Check that every registered Tauri command is classified in the plugin capability catalog.

Usage (from the repo root):
    python3 .agents/skills/plugin-capability-sync/scripts/check_catalog.py

Exit 0 when every command in the `generate_handler!` block of src-tauri/src/lib.rs
appears in a classified row (OPEN / OPEN-REDACTED / SECRET-EXCLUDED / INTERNAL)
of the catalog spec. Exit 1 and list the gaps otherwise.

Until the C3 Rust test `capability_adapter_catalog_is_complete` exists, this script
is the coverage gate for the catalog.
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[4]
LIB = ROOT / "src-tauri/src/lib.rs"
CATALOG = ROOT / ".qin-code/docs/specs/plugin-capability-catalog-spec.md"
EXPOSURE = re.compile(r"\| (OPEN-REDACTED|SECRET-EXCLUDED|INTERNAL|OPEN) \|")


def registered_commands() -> list[str]:
    text = LIB.read_text()
    start = text.index("invoke_handler(tauri::generate_handler![")
    end = text.index("])", start)
    names = set()
    for line in text[start:end].splitlines()[1:]:
        line = line.split("//", 1)[0].strip()
        if not line or line.startswith("#["):
            continue
        m = re.fullmatch(r"([A-Za-z_][A-Za-z_0-9:]*)\s*,?", line)
        if m:
            names.add(m.group(1).rsplit("::", 1)[-1])
    return sorted(names)


def classified_rows() -> list[str]:
    rows = []
    for line in CATALOG.read_text().splitlines():
        if line.startswith("| ") and EXPOSURE.search(line):
            rows.append(line)
            rows.append(line.replace(".", "_"))
    return rows


def main() -> int:
    if not CATALOG.exists():
        print(f"catalog missing: {CATALOG}")
        return 1
    commands = registered_commands()
    rows = "\n".join(classified_rows())
    missing = [c for c in commands if not re.search(rf"\b{re.escape(c)}\b", rows)]
    print(f"registered commands: {len(commands)}; unclassified: {len(missing)}")
    for name in missing:
        print(f"  UNCLASSIFIED {name}")
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main())
