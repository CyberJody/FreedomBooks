"""Regenerate shared value guards in the unmerged numbering migration."""
import json
from collections import Counter
import generate_value_guards as base

MARKER = "-- Shared value guards for the numbering settings."


def main():
    # Inventory the numbering table too; 0163 is only a schema-inventory cutoff.
    base.MIGRATION = "0163_numbering_inventory_cutoff"
    _, inventory = base.schema()
    fixture = base.HERE / "fb_value_guard_inventory.json"
    prior = json.loads(fixture.read_text())
    prior.pop("fb_numbering", None)
    assert set(inventory) - set(prior) == {"fb_numbering"}
    for table, columns in prior.items():
        assert set(columns) == set(inventory[table])
        inventory[table] = columns
    columns = inventory["fb_numbering"]
    assert columns == {"company_id": "uuid", "kind": "text", "scope": "text", "start": "text", "audit_id": "uuid"}
    out = [MARKER]
    for event in ("INSERT", "UPDATE"):
        out.append(f"CREATE TRIGGER fb_valid_fb_numbering_{event.lower()} BEFORE {event} ON fb_numbering BEGIN\n"
                   f" SELECT RAISE(ABORT,'{base.MESSAGE}') WHERE\n {base.condition('NEW.', columns)};\nEND;")
    out.append("DROP VIEW fb_value_invalid;")
    arms = [f"SELECT 'invalid stored value in {table}' AS problem FROM {table} WHERE\n {base.condition('', fields)}"
            for table, fields in inventory.items()]
    out.append("CREATE VIEW fb_value_invalid AS\n" + "\nUNION ALL\n".join(arms) + ";")
    migration = base.FBDB / "sql/0162_numbering.sql"
    prefix = migration.read_text().split(MARKER, 1)[0].rstrip()
    migration.write_text(prefix + "\n\n" + "\n".join(out) + "\n")
    fixture.write_text(json.dumps(inventory, indent=1, sort_keys=True) + "\n")
    counts = Counter(kind for fields in inventory.values() for kind in fields.values())
    print(len(inventory), sum(counts.values()), dict(counts))


if __name__ == "__main__":
    main()
