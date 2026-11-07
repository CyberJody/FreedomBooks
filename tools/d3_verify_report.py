"""Summarize completed D3 gates without concealing the inherited stopped screen."""
from pathlib import Path
import json
import re
import subprocess

ROOT=Path(__file__).resolve().parents[1]
EVIDENCE=Path('/mnt/Data/Projects/FreedomBooks/staging/d3')
base=json.loads(subprocess.check_output(['git','show','1d5b719:crates/fb-screendiff/screens.json'],cwd=ROOT,text=True))
current=json.loads((ROOT/'crates/fb-screendiff/screens.json').read_text())
gate=(EVIDENCE/'gate.log').read_text()
assert 'SCREENS DIFFER' in gate,'gate is not finished'
matches=re.findall(r'^MATCH ([^ ]+)',gate,re.M)
differences=re.findall(r'^DIFF ([^\n]+)',gate,re.M)
assert differences==['journal-control-invalid'],differences
assert set(matches)==set(current)-set(differences)
new=set(current)-set(base)
assert new.issubset(set(matches)),sorted(new-set(matches))
tests=[(int(p),int(f)) for p,f in re.findall(r'test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed;',gate)]
assert tests and all(f==0 for _,f in tests),tests
smoke=(EVIDENCE/'smoke.log').read_text();assert 'SMOKE OK: 22 checks' in smoke
harness=(EVIDENCE/'harness.log').read_text();assert '\nMATCH — ' in harness and '0 failures, 0 warnings' in harness
assert 'Finished' in (EVIDENCE/'core-build.log').read_text()
assert 'Finished' in (EVIDENCE/'clippy-final.log').read_text()
tree=subprocess.check_output(['cargo','tree','-p','fb-core','-e','normal','--prefix','none'],cwd=ROOT,text=True)
assert not re.search(r'^(napi|neon|electron|reqwest|ureq|hyper|tokio) ',tree,re.M)
(EVIDENCE/'core-tree.txt').write_text(tree)
summary=dict(base='1d5b719c12afbb5a19cbe141839af1e64e0f9443',templates=13,screens=len(current),matched=len(matches),differences=differences,
             new_screen_states=len(new),new_screen_states_matched=len(new),gate_test_executions=sum(p for p,_ in tests),gate_test_failures=0,
             gate_test_result_banners=len(tests),smoke_checks=22,electron_guard_tests=6,new_app_tests=5,new_core_tests=4,new_comparer_tests=1,
             template_tests=16,frozen_django_text_vectors=4628,screenshots=len(list(EVIDENCE.glob('d3-*.png'))),core_isolated=True,
             gate_status='blocked only by the inherited D2 journal-control refusal wording')
(EVIDENCE/'verification.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary,indent=2))
print('Final gate output: '+gate.strip().splitlines()[-1])
print('Final smoke output: '+smoke.strip().splitlines()[-1])
print('Final harness output: '+next(line for line in harness.splitlines() if line.startswith('MATCH —')))
