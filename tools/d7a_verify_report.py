"""Summarize completed D7a gates without counting a MISMATCH as a MATCH."""
from pathlib import Path
import json,re,subprocess
from PIL import Image
ROOT=Path(__file__).resolve().parents[1]
OUT=Path('/mnt/Data/Projects/FreedomBooks/staging/d7a')
gate=(OUT/'gate.log').read_text()
assert 'gate: clippy, check_json_features.sh (workspace tests + JSON feature unification), core isolation and screen diff passed' in gate
tests=[(int(a),int(b)) for a,b in re.findall(r'test result: ok\. (\d+) passed; (\d+) failed;',gate)]
assert tests and not any(failed for _,failed in tests)
assert not re.search(r'^test result: FAILED',gate,re.M)
screens=json.loads((ROOT/'crates/fb-screendiff/screens.json').read_text())
matches=re.findall(r'^MATCH ([^ ]+) ',gate,re.M)
assert len(matches)==len(screens) and set(matches)==set(screens)
assert not re.search(r'^DIFF ',gate,re.M)
assert f'SCREENS MATCH — {len(screens)} screens, 0 with differences' in gate
smoke=(OUT/'smoke-final.log').read_text()
smoke_count=int(re.search(r'^SMOKE OK: (\d+) checks$',smoke,re.M)[1])
assert 'D7a check forms:' in smoke and 'D7a deposits:' in smoke
assert 'udpPackets":0' in smoke
catalogue=json.loads((OUT/'catalogue.json').read_text())
assert catalogue['navigation'] and catalogue['productionCSP']
visual=[k for k,v in screens.items() if not v.get('media') and not 300<=v.get('status',200)<400]
assert catalogue['screens']==len(visual)
wave=[k for k in visual if k.startswith('d7a-')]
for name in wave:
    path=OUT/f'catalogue-{name}.png'
    assert Image.open(path).size==(1440,1000),name
focused=(OUT/'d7a-tests.log').read_text()
assert 'test result: ok. 9 passed; 0 failed;' in focused
presentation=(OUT/'presentation-tests.log').read_text()
assert not re.search(r'^test result: FAILED',presentation,re.M)
assert 'snapshot_repr_preserves_python_quotes_and_nonprintable_escapes ... ok' in presentation
assert 'Finished' in (OUT/'clippy-final.log').read_text()
electron=(OUT/'electron-tests.log').read_text()
assert '# tests 6' in electron and '# pass 6' in electron and '# fail 0' in electron
result=dict(commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),gate_passed=sum(n for n,_ in tests),gate_failed=0,gate_test_binaries=len(tests),screens=len(screens),d7a_screens=len(wave),differences=0,fixture_steps=len(json.loads((ROOT/'crates/fb-screendiff/scenarios/screens-d7a.json').read_text())['steps']),smoke_checks=smoke_count,catalogue=catalogue,d7a_tests=9,electron_tests=6,core_services_added=[],core_mismatches_found=[],migrations_added=[])
(OUT/'verification.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result,indent=2))
