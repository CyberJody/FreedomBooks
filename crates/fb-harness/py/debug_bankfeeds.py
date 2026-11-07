"""Debug helper: run the first N steps of a scenario through Python and print every outcome.

    FBV2_DB=fbv2_l4f_harness . crates/fb-harness/py/env.sh
    $FB_PY crates/fb-harness/py/debug_bankfeeds.py scenario.json N
"""
import json
import sys

import run_scenario

scenario = json.load(open(sys.argv[1]))
scenario['steps'] = scenario['steps'][:int(sys.argv[2])]
runner = run_scenario.Runner(scenario)
runner.run()
for o in runner.outcomes:
    if o['op'].startswith('bankfeeds_') or not o['ok']:
        print(o)
