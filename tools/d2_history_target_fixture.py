"""Use only the frozen begin() parameters; hide_later belongs to the later reconcile edit step."""
from pathlib import Path
import json

p=Path('crates/fb-screendiff/scenarios/screens-d2.json')
scenario=json.loads(p.read_text())
step=next(step for step in scenario['steps'] if step.get('ref')=='history-reconciliation')
assert step['values'].pop('hide_later') is True
p.write_text(json.dumps(scenario,indent=2)+'\n')
