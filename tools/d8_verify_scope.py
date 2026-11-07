"""Check the parallel-wave fences and successful fixture inventory independently of HTML results."""
from pathlib import Path
import json
import subprocess
from d8_hand_fixes import ROOT, FILES

def original(name):
    return subprocess.check_output(['git','show','b0b216a:'+name],cwd=ROOT,text=True)

name='crates/fb-screendiff/screens.json'
old=json.loads(original(name));new=json.loads((ROOT/name).read_text())
assert all(new.get(k)==v for k,v in old.items()),'an inherited screen was removed or changed'
assert all(k.startswith('d8-') for k in new.keys()-old.keys())
assert (ROOT/'electron/scripts/smoke.cjs').read_text().startswith(original('electron/scripts/smoke.cjs')),'shared smoke must be append-only'
commands=next(line for line in original('electron/main/guards.cjs').splitlines() if line.startswith('const COMMANDS ='))
assert commands in (ROOT/'electron/main/guards.cjs').read_text(),'existing COMMANDS must remain intact'
base=json.loads((ROOT/'crates/fb-screendiff/scenarios/screens-dfix1.json').read_text())
scenario=json.loads((ROOT/'crates/fb-screendiff/scenarios/screens-d8.json').read_text())
assert scenario['steps'][:len(base['steps'])]==base['steps']
assert all(s.get('expect_error') is False for s in scenario['steps'])
assert {name.rsplit('/',1)[-1] for name in FILES}=={'_controls','_rows','profit_loss','profit_loss_body','balance_sheet','balance_sheet_body','general_ledger','general_ledger_body'}
for route in ['profit-and-loss','balance-sheet','general-ledger']:
    assert any(s['path'].split('?')[0].endswith('/reports/'+route+'/') and s.get('post')=={} and s.get('status')==405 for s in new.values()),route
print(f'D8 scope: {len(old)} inherited states unchanged; {len(new)-len(old)} appended; {len(scenario["steps"])} steps explicitly expect success; eight templates; shared smoke and COMMANDS append-only')
