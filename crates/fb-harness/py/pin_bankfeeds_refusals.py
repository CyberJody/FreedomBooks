"""Pin every refusal of the hand-written bank-feed scenarios to its exact kind and message.

    python3 crates/fb-harness/py/pin_bankfeeds_refusals.py <harness output dir>

Reads `<scenario>.python.json` and `<scenario>.rust.json` (a harness run of the scenarios as generated) and
writes `bankfeeds-refusals.expect.json`: for each scenario, step index -> [kind, message] for every step
the scenario expects to be refused. The two sides must refuse with the same kind. Their messages must be
equal, except where the documented redaction divergence applies (L4-BANKFEEDS.md "Python bugs not
copied": Python keeps URL userinfo that v2 removes); there the v2 message is pinned, and the comparer
checks the Python side after applying v2's rule to its bank-feed messages. The generator applies the
pins and refuses to write a scenario with an unpinned refusal.
"""
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SCENARIOS = ['bankfeeds-connect-map', 'bankfeeds-pulls', 'bankfeeds-rules', 'bankfeeds-accept']
# Userinfo left in a Python message: anything but `***` between `://` and an `@` of the same run.
USERINFO = re.compile(r"://(?!\*\*\*@)[^\s<>\"'/]*@")
out_dir = sys.argv[1]
pins = {}
for name in SCENARIOS:
    steps = json.load(open(os.path.join(HERE, '..', 'scenarios', f'{name}.json')))['steps']
    py = {o['step']: o for o in json.load(open(os.path.join(out_dir, f'{name}.python.json')))['outcomes']}
    rs = {o['step']: o for o in json.load(open(os.path.join(out_dir, f'{name}.rust.json')))['outcomes']}
    pins[name] = {}
    for index, step in enumerate(steps):
        if step.get('expect_error') is not True:
            continue
        p, r = py[index], rs[index]
        assert not p['ok'] and not r['ok'], (name, index, 'not refused on both sides')
        assert p['kind'] == r['kind'], (name, index, p['kind'], r['kind'])
        if p['message'] != r['message']:
            assert USERINFO.search(p['message']) and not USERINFO.search(r['message']), (name, index, p['message'], r['message'])
            print(f'{name} step {index}: redaction divergence, pinned v2 {r["message"]!r} (python {p["message"]!r})')
        for k in ('expect_kind', 'expect_message'):
            if k in step:
                assert step[k] == r[k[len('expect_'):]], (name, index, k, step[k], r[k[len('expect_'):]])
        pins[name][str(index)] = [r['kind'], r['message']]
with open(os.path.join(HERE, 'bankfeeds-refusals.expect.json'), 'w') as handle:
    json.dump(pins, handle, indent=1, ensure_ascii=False)
    handle.write('\n')
print({name: len(v) for name, v in pins.items()}, 'refusals pinned')
