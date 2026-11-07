"""Collect machine-checkable D6 verification evidence; the worker's final message is the report."""
from pathlib import Path
import json
import re
import subprocess

ROOT=Path(__file__).resolve().parents[1]
OUT=Path('/mnt/Data/Projects/FreedomBooks/staging/d6')
screens=json.loads((ROOT/'crates/fb-screendiff/screens.json').read_text())
log=(OUT/'screens-final.log').read_text()
new=[name for name in screens if name.startswith('d6-')]
matched=re.findall(r'^MATCH (\S+) ',log,re.M)
differences=re.findall(r'^DIFF (\S+)',log,re.M)
assert all(name in matched for name in new),set(new)-set(matched)
assert differences==['journal-control-invalid'],differences
test_runs=[]
gate=(OUT/'gate.log').read_text()
for match in re.finditer(r'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored;',gate):
    test_runs.append(dict(status=match[1],passed=int(match[2]),failed=int(match[3]),ignored=int(match[4])))
smoke=(OUT/'smoke-final3.log').read_text()
smoke_match=re.search(r'SMOKE OK: (\d+) checks',smoke)
assert smoke_match,'Smoke has not completed successfully'
shots=json.loads((OUT/'screenshots.json').read_text())
assert len(shots)>=21
assert not subprocess.check_output(['git','diff','779adb1','--','crates/fb-core'],cwd=ROOT,text=True)
data=dict(base='779adb1',head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),ported_templates=18,
          screen_states=len(screens),new_states=len(new),new_matches=len(new),all_matches=len(matched),differences=differences,
          gate_test_runs=test_runs,gate_passed=sum(run['passed'] for run in test_runs),gate_failed=sum(run['failed'] for run in test_runs),
          gate_completed='SCREENS DIFFER' in gate,gate_blocker='Inherited journal-control-invalid wording mismatch; fb-core unchanged per COMMON rule 10',
          smoke_checks=int(smoke_match[1]),screenshot_count=len(shots),harness=(OUT/'harness-final.log').read_text().splitlines()[-2],
          clippy=(OUT/'clippy-final.log').read_text().splitlines()[-1])
(OUT/'verification.json').write_text(json.dumps(data,indent=2)+'\n')
print(json.dumps(data,indent=2))
