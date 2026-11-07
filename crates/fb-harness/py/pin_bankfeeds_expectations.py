"""Pin every random-500 bank-feed step to the outcome the frozen Python app gives it.

    python3 crates/fb-harness/py/pin_bankfeeds_expectations.py <bankfeeds-random-500.python.json>

Reads the Python runner's outcomes for the UNPINNED scenario (delete bankfeeds-random-500.expect.json and
regenerate first) and writes `bankfeeds-random-500.expect.json`: one entry per step with `expect_error`, and
for a refusal its exact `expect_kind` and `expect_message`. The generator applies the pins; both runners then
assert them independently, so a step that fails on both sides in the same way still fails the harness.
"""
import json
import os
import sys
from collections import Counter

HERE = os.path.dirname(os.path.abspath(__file__))
outcomes = json.load(open(sys.argv[1]))['outcomes']
pins = []
for o in outcomes:
    pin = dict(op=o['op'], expect_error=not o['ok'])
    if not o['ok']:
        pin.update(expect_kind=o['kind'], expect_message=o['message'])
    pins.append(pin)
with open(os.path.join(HERE, 'bankfeeds-random-500.expect.json'), 'w') as handle:
    json.dump(pins, handle, indent=0, ensure_ascii=False)
    handle.write('\n')
refused = Counter((p['op'], p.get('expect_kind')) for p in pins if p['expect_error'])
print(len(pins), 'steps pinned;', sum(not p['expect_error'] for p in pins), 'succeed;', sum(refused.values()), 'refused:')
for (op, kind), n in sorted(refused.items()):
    print(f'  {n:4} {op} {kind}')
