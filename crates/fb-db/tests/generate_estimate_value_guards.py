"""Append guards for 0131's estimate tables without changing main's frozen 0130 migration."""
import json
import re
from collections import Counter
import generate_value_guards as base


def main():
    base.MIGRATION = '0132_estimate_validators'
    _, inventory = base.schema()
    prior_tables = set(re.findall(r'CREATE TRIGGER fb_valid_(\w+)_insert', (base.FBDB / 'sql/0130_shared_validators.sql').read_text()))
    new = {t: c for t, c in inventory.items() if t not in prior_tables}
    assert set(new) == {'estimate_seal', 'estimate_seal_due', 'sales_estimate', 'sales_estimaterevision', 'sales_estimateline', 'sales_estimateconversion'}
    out = ['-- Extend main 0130 guards for the appended 0131 estimates schema; no data changes.']
    for table, columns in new.items():
        for event in ('INSERT', 'UPDATE'):
            out.append(f"CREATE TRIGGER fb_valid_{table}_{event.lower()} BEFORE {event} ON {table} BEGIN\n"
                       f" SELECT RAISE(ABORT,'{base.MESSAGE}') WHERE\n {base.condition('NEW.', columns)};\nEND;")
    out.append('DROP VIEW fb_value_invalid;')
    arms = [f"SELECT 'invalid stored value in {t}' AS problem FROM {t} WHERE\n {base.condition('', columns)}" for t, columns in inventory.items()]
    out.append('CREATE VIEW fb_value_invalid AS\n' + '\nUNION ALL\n'.join(arms) + ';')
    (base.FBDB / 'sql/0132_estimate_validators.sql').write_text('\n'.join(out) + '\n')
    (base.HERE / 'fb_value_guard_inventory.json').write_text(json.dumps(inventory, indent=1, sort_keys=True) + '\n')
    print(len(inventory), sum(map(len, inventory.values())), dict(Counter(k for c in inventory.values() for k in c.values())))


if __name__ == '__main__':
    main()
