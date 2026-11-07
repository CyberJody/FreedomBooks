"""Run R11's independent generator/driver against this tree without editing its source evidence."""
from pathlib import Path
import shutil, subprocess, os, json, sys, collections, importlib.util
root=Path(__file__).resolve().parents[1]
source=root.parents[1]/'staging/r11-l4-bankfeeds-review'
out=root.parents[1]/'staging/secfix1/leak'
out.mkdir(parents=True,exist_ok=True)
for name in ['gen.py','gen11.py','verify.py','adjacent.json']:
    shutil.copy2(source/name,out/name)
for kind in ['baseline','vectors','control','joins']:
    subprocess.run(['python3',str(out/'gen11.py'),kind],check=True,stdout=subprocess.DEVNULL)
    if kind in ('baseline','vectors'):
        assert json.loads((out/f'{kind}.json').read_text())==json.loads((source/f'{kind}.json').read_text()),'R11 regeneration drift'
driver=root/'crates/fb-core/src/bankfeeds/secfix1_r11.rs'
tests=root/'crates/fb-core/src/bankfeeds/tests.rs'
original=tests.read_text()
driver.write_text((source/'mine.rs').read_text().replace(str(source),str(out)))
tests.write_text(original+'\n#[path="secfix1_r11.rs"]\nmod secfix1_r11;\n')
env=dict(os.environ,RUST_TEST_THREADS='1',CARGO_BUILD_JOBS='3')
try:
    result=subprocess.run(['cargo','test','-p','fb-core','--lib','--no-run','--message-format=json'],cwd=root,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
    (out/'build.log').write_text(result.stderr)
    if result.returncode:print('R11 build failed; see',out/'build.log');sys.exit(result.returncode)
    artifacts=[json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
    executable=next(a['executable'] for a in artifacts if a.get('reason')=='compiler-artifact' and a.get('executable') and a['target']['name']=='fb_core')
finally:
    tests.write_text(original)
    driver.unlink()
# Source is clean before the long run, so the ordinary gate can run independently.
print('R11 driver compiled; production source restored.',flush=True)
with (out/'driver.log').open('w') as log:
    result=subprocess.run([executable,'bankfeeds::tests::secfix1_r11::','--nocapture','--test-threads=1','--skip','r11_restore'],cwd=root,env=env,stdout=log,stderr=subprocess.STDOUT)
if result.returncode:print('R11 driver failed; see',out/'driver.log');sys.exit(result.returncode)
# The same independent stdlib decoders; print only aggregate counts.
spec=importlib.util.spec_from_file_location('r11verify',out/'verify.py');verify=importlib.util.module_from_spec(spec);spec.loader.exec_module(verify)
summary={}
for kind in ['baseline','vectors','control']:
    cases={c['id']:c for c in json.loads((out/f'{kind}.json').read_text())}
    modes=collections.defaultdict(collections.Counter)
    for line in (out/f'{kind}.out.jsonl').read_text().splitlines():
        run=json.loads(line);case=cases[run['id']];c=modes[run['mode']];c['runs']+=1
        secrets=[('pass',case['pass']),('user',case['user']),('user:pass',case['user']+':'+case['pass'])]
        for _,value in run['texts']+run['audits']:
            c['stored_texts']+=1;c['stored_recoveries']+=bool(verify.recovered(value,secrets))
        for value in run['returned']: c['returned_recoveries']+=bool(verify.recovered(value,secrets))
        c['rollback_failures']+=sum(not ok for _,ok in run['refused_rollback'])
        for value in verify.leaves(case['fields']):
            if verify.recovered(value,secrets): c['positive_input_recoveries']+=1
    summary[kind]={m:dict(v) for m,v in modes.items()}
(out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary,indent=2))
assert all(v['stored_recoveries']==0 and v['rollback_failures']==0 for kind in ('baseline','vectors') for v in summary[kind].values())
assert sum(v['positive_input_recoveries'] for v in summary['vectors'].values())>0
print('R11 PASS: zero stored-text recoveries and rollback failures; positive decoder controls fire.')
