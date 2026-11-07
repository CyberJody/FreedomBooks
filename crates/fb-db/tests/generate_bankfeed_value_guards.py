"""Extend the final newly integrated bank-feed migration with the shared value-guard sweep.

The five bank-feed migrations append to main as 0133..0137; no main SQL is changed.
Provider remote_id is a provider text identifier, never a FreedomBooks UUID.
"""
import json
from collections import Counter
import generate_value_guards as base

MARKER = '-- INT-L4b shared value guards (generated).'

def main():
    base.MIGRATION = '0138_boundary'
    _, inventory = base.schema()
    for table, columns in inventory.items():
        if table.startswith('bankfeeds_') and 'remote_id' in columns:
            columns['remote_id'] = 'text'
    new = {t: c for t, c in inventory.items() if t.startswith('bankfeeds_')}
    assert len(new) == 7
    out = [MARKER]
    for table, columns in new.items():
        for event in ('INSERT', 'UPDATE'):
            out.append(f"CREATE TRIGGER fb_valid_{table}_{event.lower()} BEFORE {event} ON {table} BEGIN\n"
                       f" SELECT RAISE(ABORT,'{base.MESSAGE}') WHERE\n {base.condition('NEW.', columns)};\nEND;")
    out.append('DROP VIEW fb_value_invalid;')
    arms = [f"SELECT 'invalid stored value in {t}' AS problem FROM {t} WHERE\n {base.condition('', columns)}" for t, columns in inventory.items()]
    out.append('CREATE VIEW fb_value_invalid AS\n' + '\nUNION ALL\n'.join(arms) + ';')
    p = base.FBDB / 'sql/0137_bankfeeds_reconnect.sql'
    p.write_text(p.read_text().split(MARKER)[0] + '\n'.join(out) + '\n')
    (base.HERE / 'fb_value_guard_inventory.json').write_text(json.dumps(inventory, indent=1, sort_keys=True) + '\n')
    print(len(inventory), sum(map(len, inventory.values())), dict(Counter(k for c in inventory.values() for k in c.values())))

if __name__ == '__main__':
    main()
