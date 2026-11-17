"""Regenerate the generated tail of 0205_job_tickets.sql and the value-guard inventory entries.

    python3 crates/fb-db/tests/generate_job_ticket_guards.py

Installs every earlier migration listed in lib.rs (stub UDFs and collations, schema only), then the
hand-written part of 0205 (everything above the GENERATED marker). For each new jobs_* table it emits one
BEFORE INSERT and one BEFORE UPDATE value guard and appends one arm to the shared open-time view
fb_value_invalid, keeping the prior view text unchanged before the new arms. The classification is the
same naming rule as generate_value_guards.py; fb_value_guard_inventory.json gains the new tables.
"""
import json
import re
import sqlite3
from pathlib import Path

HERE = Path(__file__).resolve().parent
FBDB = HERE.parent
MIGRATION = "0205_job_tickets"
MARKER = "-- GENERATED BELOW by crates/fb-db/tests/generate_job_ticket_guards.py: value guards and fb_value_invalid.\n"
MESSAGE = "fb_valid: Invalid stored value"
FUNCTION = dict(uuid="fb_valid_uuid", date="fb_valid_date", timestamp="fb_valid_timestamp", text="fb_valid_text")


def classify(column):
    if column == "doc_id":
        return "text"
    if column == "id" or column.endswith("_id") or column == "request_key":
        return "uuid"
    if column == "date" or column.endswith("_date"):
        return "date"
    if column.endswith("_at"):
        return "timestamp"
    return "text"


def connect(sqls):
    conn = sqlite3.connect(":memory:")
    joined = "\n".join(sqls)
    for fn in set(re.findall(r"\b(fb_\w+)\s*\(", joined)):
        conn.create_function(fn, -1, lambda *args: 1, deterministic=True)
    for collation in set(re.findall(r"COLLATE\s+(\w+)", joined, re.I)) - {"NOCASE", "BINARY", "RTRIM", "nocase", "binary", "rtrim"}:
        conn.create_collation(collation, lambda a, b: (a > b) - (a < b))
    return conn


def text_columns(conn, table):
    cols = [(r[1], r[2].upper()) for r in conn.execute(f'PRAGMA table_xinfo("{table}")')]
    return {c: classify(c) for c, t in cols if t == "TEXT"}


def condition(prefix, columns):
    return " OR ".join(f"NOT {FUNCTION[kind]}({prefix}{col})" for col, kind in columns.items())


def main():
    lib = (FBDB / "src" / "lib.rs").read_text()
    registry = lib.split("pub const MIGRATIONS")[1].split("];")[0]
    entries = re.findall(r'\("(\d{4}_\w+)", include_str!\("\.\./sql/([^"]+)"\)\)', registry)
    names = [n for n, _ in entries]
    prior = [f for n, f in entries[: names.index(MIGRATION)]]
    sqls = [(FBDB / "sql" / f).read_text() for f in prior]
    path = FBDB / "sql" / f"{MIGRATION}.sql"
    head = path.read_text().split(MARKER)[0]
    conn = connect(sqls + [head])
    for sql in sqls:
        conn.executescript(sql)
    before = {r[0] for r in conn.execute("SELECT name FROM sqlite_schema WHERE type='table'")}
    view = conn.execute("SELECT sql FROM sqlite_schema WHERE name='fb_value_invalid'").fetchone()[0]
    conn.executescript(head)
    tables = sorted(r[0] for r in conn.execute("SELECT name FROM sqlite_schema WHERE type='table'") if r[0] not in before)
    out, arms, inventory = [], [], {}
    for table in tables:
        columns = text_columns(conn, table)
        inventory[table] = dict(sorted(columns.items()))
        for event in ("INSERT", "UPDATE"):
            out.append(f"CREATE TRIGGER fb_valid_{table}_{event.lower()} BEFORE {event} ON {table} WHEN {condition('NEW.', columns)} BEGIN SELECT RAISE(ABORT,'{MESSAGE}'); END;")
        arms.append(f"SELECT 'invalid stored value in {table}' AS problem FROM {table} WHERE\n {condition('', columns)}")
    out.append("DROP VIEW fb_value_invalid;")
    out.append(view.rstrip(";") + "\nUNION ALL\n" + "\nUNION ALL\n".join(arms) + ";")
    path.write_text(head + MARKER + "\n".join(out) + "\n")
    inv_path = HERE / "fb_value_guard_inventory.json"
    full = {t: c for t, c in json.loads(inv_path.read_text()).items() if t not in inventory}
    full.update(inventory)
    inv_path.write_text(json.dumps(full, indent=1) + "\n")
    print(f"{len(prior)} prior migrations; guarded {', '.join(tables)}")


if __name__ == "__main__":
    main()
