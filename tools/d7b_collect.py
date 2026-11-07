"""Copy bounded job evidence and summarize complete-prefix results, never substring MATCH counts."""
from pathlib import Path
import json, os, re, shutil
ROOT=Path(__file__).resolve().parents[1]
TMP=Path(os.environ['TMPDIR'])
OUT=Path('/mnt/Data/Projects/FreedomBooks/staging/d7b')
OUT.mkdir(parents=True,exist_ok=True)
for source,target in [('d7b-gate-final.log','gate.log'),('d7b-harness.log','harness.log'),('d7b-smoke-final.log','smoke.log'),('d7b-tests.log','d7b-tests.log'),('d7b-normalize-tests.log','normalize-tests.log'),('d7b-core-gap.log','core-gap.log'),('d7b-leak-verify.log','leak-verify.log'),('d7b-screen-final.log','screen-diff.log'),('d7b-paging-smoke.log','paging-smoke.log'),('d7b-clippy-final.log','clippy.log'),('d7b-clippy-balance.log','clippy-balance.log')]:
    if (TMP/source).exists():shutil.copyfile(TMP/source,OUT/target)
gate=(OUT/'gate.log').read_text()
tests=re.findall(r'test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;',gate)
harness=(OUT/'harness.log').read_text()
smoke=(OUT/'smoke.log').read_text()
result=dict(gate_passed_tests=sum(int(p) for p,f,i in tests),gate_failed_tests=sum(int(f) for p,f,i in tests),gate_test_suites=len(tests),
            harness_match=len(re.findall(r'^MATCH —',harness,re.M)),harness_mismatch=len(re.findall(r'^MISMATCH —',harness,re.M)),
            harness_scenarios_expected=len(list((ROOT/'crates/fb-harness/scenarios').glob('*.json'))),
            screen_summary=re.findall(r'^SCREENS (?:MATCH|DIFFER) —.*',gate,re.M),
            screen_differences=re.findall(r'^DIFF ([^\n]+)',gate,re.M),
            smoke_summary=re.findall(r'^SMOKE(?: D8)? OK:.*',smoke,re.M),
            smoke_failures=re.findall(r'^SMOKE.*FAILED.*',smoke,re.M),
            standalone_screens=re.findall(r'^SCREENS (?:MATCH|DIFFER) —.*',(OUT/'screen-diff.log').read_text(),re.M))
(OUT/'verification-summary.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result,indent=2))
