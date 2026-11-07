"""Verify every final acceptance artifact and emit a small, secret-free report."""
from pathlib import Path
import json
import re
import subprocess
from PIL import Image

ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT.parents[1]/'staging/dfix3'
def git(*args):return subprocess.check_output(['git',*args],cwd=ROOT,text=True).strip()
def log(name):return (OUT/(name+'.log')).read_text()
assert git('branch','--show-current')=='dfix3'
subprocess.run(['git','merge-base','--is-ancestor','cc40505','HEAD'],cwd=ROOT,check=True)
assert not git('status','--porcelain'), 'commit the final source and tooling first'
assert git('diff','--name-only','cc40505','HEAD','--','crates/fb-db/sql')=='crates/fb-db/sql/0140_deposit_estimates.sql'
assert not git('diff','--name-only','98100e0','d7b','--','crates/fb-db/sql')
count=len(json.loads((ROOT/'crates/fb-screendiff/screens.json').read_text()))
gate=log('gate')
assert gate.rstrip().endswith('gate: clippy, check_json_features.sh (workspace tests + JSON feature unification), core isolation and screen diff passed')
assert f'SCREENS MATCH — {count} screens, 0 with differences' in gate
results=[tuple(map(int,m)) for m in re.findall(r'^test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored;',gate,re.M)]
assert results and sum(r[1] for r in results)==0 and not re.findall(r'^test .+ \.\.\. FAILED$',gate,re.M)
assert f'SCREENS MATCH — {count} screens, 0 with differences' in log('screens')
harness=log('harness')
assert len(re.findall(r'^MATCH —',harness,re.M))==128
assert not re.findall(r'^(?:FAIL|MISMATCH)',harness,re.M)
assert 'MATCH —' in log('core-gaps') and not re.findall(r'^(?:FAIL|MISMATCH)',log('core-gaps'),re.M)
assert 'MATCH —' in log('bank-rule-parity') and not re.findall(r'^(?:FAIL|MISMATCH)',log('bank-rule-parity'),re.M)
clock=log('clock')
assert f'CLOCK PROOF PASS: 23:30 and 00:30 local; {2*count} normalized files byte-identical' in clock
smokes={}
for name in ['smoke','smoke-secfix1']:
    text=log(name)
    assert 'SMOKE OK: 40 checks' in text and 'SMOKE D8 OK: 10 checks' in text and 'SMOKE FAILED' not in text
    assert 'D7b undo restores cleared marks' in text
    catalogue=json.loads((OUT/('screenshots-'+name)/'catalogue.json').read_text())
    smokes[name]={'checks':50,'catalogue_screens':catalogue['screens'],'production_csp':catalogue['productionCSP']}
    for p in (OUT/('screenshots-'+name)).glob('d7b-0[1-9]-*.png'):
        assert Image.open(p).size==(1440,1000)
assert 'FUSES VERIFIED linux/x64' in log('package') and 'pass 1' in log('package')
assert 'pass 9' in log('node-tests')
leak=json.loads((OUT/'leak/summary.json').read_text())
for mode in ['live','restore']:
    assert leak['vectors'][mode]['runs']==60
    assert leak['vectors'][mode]['stored_recoveries']==0 and leak['vectors'][mode]['rollback_failures']==0
leak_commit=(OUT/'leak/source-commit.txt').read_text().strip()
subprocess.run(['git','diff','--quiet',leak_commit,'HEAD','--','crates'],cwd=ROOT,check=True)
upgrades={}
for source in ['98100e0','cc40505']:
    text=(OUT/'upgrade'/(source+'-verify.log')).read_text()
    assert f'UPGRADE/LIFECYCLE PASS "{source}" -> FIX3; integrity and reopen clean' in text
    assert 'PASS backup and restore preserve every legacy row and the exact schema hash' in text
    schema=re.search(r'SCHEMA HASH MATCH .* -> FIX3: ([0-9a-f]{64})',text)[1]
    upgrades[source]={'prior_migrations':json.loads((OUT/'upgrade'/(source+'-books')/'fixture.json').read_text())['version'],'schema_sha256':schema}
assert len({v['schema_sha256'] for v in upgrades.values()})==1
assert 'PASS: all four FIX3 PostgreSQL databases dropped; absence verified' in log('cleanup')
report={'head':git('rev-parse','HEAD'),'branch':'dfix3','base':'cc40505','clean':True,'migration_added':'0140_deposit_estimates','d7b_migrations_added':0,'gate_exit':0,'rust_passed_executions':sum(r[0] for r in results),'rust_failed_executions':0,'rust_ignored_executions':sum(r[2] for r in results),'screens_match':count,'screen_differences':0,'clock_normalized_files':2*count,'harness_match':128,'smokes':smokes,'node_tests':9,'packaged_fuse_tests':1,'leak_vector_runs':120,'leak_stored_recoveries':0,'leak_positive_decoder_controls':sum(leak['vectors'][m]['positive_input_recoveries'] for m in ['live','restore']),'leak_source_commit':leak_commit,'upgrades':upgrades,'databases_absent':True}
(OUT/'verification.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report,indent=2))
