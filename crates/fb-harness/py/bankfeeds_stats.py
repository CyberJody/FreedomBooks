"""Print ok / refused counts per op (and accepted kinds) for a bank-feed harness dump.

    python3 crates/fb-harness/py/bankfeeds_stats.py target/harness/bankfeeds-random-500.python.json
"""
import collections
import json
import sys

dump = json.load(open(sys.argv[1]))
counts = collections.Counter((o['op'], o['ok']) for o in dump['outcomes'])
for (op, ok), n in sorted(counts.items()):
    print(f'{op:28} {"ok" if ok else "refused":8} {n}')
print('statuses', collections.Counter((t['status'], t['accepted_as']) for t in dump['bankfeeds']['transactions']))
print('returned', sum(t['return_count'] for t in dump['bankfeeds']['transactions']))
