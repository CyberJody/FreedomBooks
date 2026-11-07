"""Reproduce the ten reviewed installed trigger/required-guard name inventories.

Runs the actual migration chains from git in empty SQLite databases. Stub UDFs
allow schema installation only; this does not test their accounting semantics.
The committed fixture lets cargo tests run without git or Python.
"""
import json
import re
import sqlite3
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
HEADS = dict(sales="6bde80e", payables="a91a9cf", checks="9a47557",
             prepayments="ba6c157", credits="ab7c5d8", banking="b0c7000",
             # INT-L4: main before layer 4, and the reviewed layer-4 heads with migrations (reports has none).
             main="9ce9933", backups="f8972b3", attachments="5060fc9", assistant="f34d3f1")


def source(ref, path):
    return subprocess.check_output(["git", "show", f"{ref}:{path}"], cwd=ROOT, text=True)


inventory = {}
for module, ref in HEADS.items():
    lib = source(ref, "crates/fb-db/src/lib.rs")
    migrations = re.findall(r'include_str!\("../sql/([^"]+)"\)', lib.split("pub const REQUIRED_TRIGGERS")[0])
    required = re.findall(r'"([^"]+)"', lib.split("pub const REQUIRED_TRIGGERS")[1].split("];")[0])
    sqls = [source(ref, "crates/fb-db/sql/" + name) for name in migrations]
    conn = sqlite3.connect(":memory:")
    for name in set(re.findall(r"\b(fb_\w+)\s*\(", "\n".join(sqls))):
        conn.create_function(name, -1, lambda *args: 1, deterministic=True)
    for name in set(re.findall(r"\bCOLLATE\s+(\w+)", "\n".join(sqls), re.I)):
        conn.create_collation(name, lambda a, b: (a > b) - (a < b))
    for sql in sqls:
        conn.executescript(sql)
    triggers = sorted(row[0] for row in conn.execute("SELECT name FROM sqlite_master WHERE type='trigger'"))
    inventory[module] = dict(head=ref, triggers=triggers, required=sorted(set(required)))
    print(module, ref, "installed", len(triggers), "required", len(set(required)))
    conn.close()
Path(__file__).with_name("reviewed_trigger_inventory.json").write_text(json.dumps(inventory, indent=2) + "\n")
