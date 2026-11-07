"""Verify fb-db's frozen category table against the frozen Python's own unicodedata (run with $FB_PY).

    "$FB_PY" crates/fb-harness/py/check_unicode_categories.py

The table (crates/fb-db/src/credits_number_categories.rs) lists every code point whose general category
starts with C or is Zl/Zp. `fb_db::check_numbers::char_allowed` and `fb_core::py::is_control_or_format`
both read it, so neither depends on the Rust crate's (newer) Unicode tables.
"""
import re
import sys
import unicodedata
from pathlib import Path

table = Path(__file__).resolve().parents[2] / "fb-db" / "src" / "credits_number_categories.rs"
ranges = [(int(a, 16), int(b, 16)) for a, b in re.findall(r"\(0x([0-9a-f]+), 0x([0-9a-f]+)\)", table.read_text())]
expected, start = [], None
for cp in range(0x110000):
    cat = unicodedata.category(chr(cp))
    hit = cat.startswith("C") or cat in ("Zl", "Zp")
    if hit and start is None:
        start = cp
    if not hit and start is not None:
        expected.append((start, cp - 1))
        start = None
if start is not None:
    expected.append((start, 0x10FFFF))
print(f"Python {sys.version.split()[0]}, Unicode {unicodedata.unidata_version}: {len(expected)} ranges; table {len(ranges)}")
if ranges != expected:
    raise SystemExit("MISMATCH: regenerate credits_number_categories.rs from this interpreter")
print("MATCH")
