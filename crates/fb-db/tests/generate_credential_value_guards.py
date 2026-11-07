"""Append shared value guards for the 0160 opaque credential envelope table."""
import json
from collections import Counter
import generate_value_guards as base


def main():
    base.MIGRATION = "0161_credential_validators"
    _, inventory = base.schema()
    prior_path = base.HERE / "fb_value_guard_inventory.json"
    prior = json.loads(prior_path.read_text())
    # Preserve all prior classifications, including provider remote IDs as text.
    prior.pop("plugin_credential", None)
    assert set(inventory) - set(prior) == {"plugin_credential"}
    for table, columns in prior.items():
        assert set(columns) == set(inventory[table])
        inventory[table] = columns
    # Plugin IDs are names such as "simplefin", never model UUIDs.
    inventory["plugin_credential"]["plugin_id"] = "text"
    columns = inventory["plugin_credential"]
    assert columns == {"company_id": "uuid", "plugin_id": "text", "connection_id": "uuid"}
    out = ["-- Extend shared value validation for opaque credential envelope scope metadata."]
    for event in ("INSERT", "UPDATE"):
        out.append(f"CREATE TRIGGER fb_valid_plugin_credential_{event.lower()} BEFORE {event} ON plugin_credential BEGIN\n"
                   f" SELECT RAISE(ABORT,'{base.MESSAGE}') WHERE\n {base.condition('NEW.', columns)};\nEND;")
    out.append("DROP VIEW fb_value_invalid;")
    arms = [f"SELECT 'invalid stored value in {table}' AS problem FROM {table} WHERE\n {base.condition('', columns)}"
            for table, columns in inventory.items()]
    out.append("CREATE VIEW fb_value_invalid AS\n" + "\nUNION ALL\n".join(arms) + ";")
    (base.FBDB / "sql" / (base.MIGRATION + ".sql")).write_text("\n".join(out) + "\n")
    prior_path.write_text(json.dumps(inventory, indent=1, sort_keys=True) + "\n")
    print(len(inventory), sum(map(len, inventory.values())), dict(Counter(k for c in inventory.values() for k in c.values())))


if __name__ == "__main__":
    main()
