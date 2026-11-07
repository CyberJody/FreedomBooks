"""Summarize exact SEC-FIX1 gate evidence, never substring-count MISMATCH as MATCH."""
from pathlib import Path
import json,re,subprocess,sys

root=Path(__file__).resolve().parents[1]
out=root.parents[1]/'staging/secfix1'
def read(name):
    p=out/name
    return p.read_text() if p.exists() else ''
gate=read('gate.log');harness=read('harness.log');smoke=read('smoke.log')
tests=re.findall(r'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored',gate)
matches=len(re.findall(r'^MATCH(?: |$)',harness,re.M))
scenarios=len(list((root/'crates/fb-harness/scenarios').glob('*.json')))
screens=len(json.loads((root/'crates/fb-screendiff/screens.json').read_text()))
summary={
    'commit':subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip(),
    'rust_test_executions_passed':sum(int(t[1]) for t in tests),
    'rust_test_executions_failed':sum(int(t[2]) for t in tests),
    'rust_test_executions_ignored':sum(int(t[3]) for t in tests),
    'gate_passed':bool(re.search(r'^gate: .*passed$',gate,re.M)),
    'screens_expected':screens,
    'screens_passed':f'SCREENS MATCH — {screens} screens, 0 with differences' in gate,
    'harness_expected':scenarios,'harness_matches':matches,
    'harness_mismatches':len(re.findall(r'^MISMATCH(?: |$)',harness,re.M)),
    'harness_finished':'database left from the last scenario:' in harness,
    'electron_unit_passed':bool(re.search(r'^# pass 9$',read('electron-tests.log'),re.M)),
    'packaged_fuses_passed':'FUSES VERIFIED' in read('package.log') and bool(re.search(r'pass 1$',read('package.log'),re.M)),
    'smoke_checks':re.findall(r'^SMOKE(?: D8)? OK: (\d+) checks$',smoke,re.M),
}
db_runs=list(re.finditer(r'^\s+Running unittests src/lib.rs \(target/debug/deps/fb_db-',gate,re.M))
if len(db_runs)>=2:
    split=db_runs[1].start()
    summary['workspace_tests_passed']=sum(int(n) for n in re.findall(r'test result: ok\. (\d+) passed;',gate[:split]))
    summary['json_feature_tests_passed']=sum(int(n) for n in re.findall(r'test result: ok\. (\d+) passed;',gate[split:]))
leak_path=out/'leak/summary.json'
if leak_path.exists():
    leak=json.loads(leak_path.read_text());summary['r11']=leak
    summary['r11_stored_checks']=sum(v['stored_texts'] for v in leak['vectors'].values())
    summary['r11_stored_recoveries']=sum(v['stored_recoveries'] for v in leak['vectors'].values())
    summary['r11_positive_controls']=sum(v['positive_input_recoveries'] for v in leak['vectors'].values())
print(json.dumps(summary,indent=2))
if '--progress' not in sys.argv:
    assert summary['gate_passed'] and summary['screens_passed']
    assert summary['rust_test_executions_failed']==0
    assert matches==scenarios and summary['harness_mismatches']==0 and summary['harness_finished']
    assert summary['electron_unit_passed'] and summary['packaged_fuses_passed']
    assert len(summary['smoke_checks'])==2
    assert summary['r11_stored_recoveries']==0 and summary['r11_positive_controls']>0
    (out/'verification.json').write_text(json.dumps(summary,indent=2)+'\n')
    print('SEC-FIX1 verification: all required gates passed')
