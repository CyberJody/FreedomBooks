"""Adopt SEC-FIX1's inherited fixture inputs while keeping every D7b appended step."""
from pathlib import Path
import json
root=Path(__file__).resolve().parents[1]
p=root/'crates/fb-screendiff/scenarios/screens-d7b.json'
scenario=json.loads(p.read_text())
base=json.loads((p.parent/'screens-dfix2.json').read_text())
start=next(i for i,s in enumerate(scenario['steps']) if s.get('ref')=='d7b-bank')
assert start==len(base['steps']),(start,len(base['steps']))
for old,new in zip(scenario['steps'][:start],base['steps']):
    assert old['op']==new['op'] and old.get('ref')==new.get('ref')
scenario['steps'][:start]=base['steps']
p.write_text(json.dumps(scenario,indent=2)+'\n')
print(f'Adopted {start} SEC-FIX1 fixture steps; kept {len(scenario["steps"])-start} D7b/FIX3 steps')
