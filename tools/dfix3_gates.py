"""Run each named gate in the foreground, with dedicated resources and durable logs."""
from pathlib import Path
import os
import shutil
import subprocess
import sys
ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT.parents[1]/'staging/dfix3'
OUT.mkdir(parents=True,exist_ok=True)
XV=ROOT.parent/'d1-desktop/electron/.tooling/xvfb/usr'
NODE=ROOT.parent/'d1-desktop/electron/.tooling/node-v24.22.2-linux-x64/bin'
env=dict(os.environ)
env['PATH']=f'{NODE}:{XV}/bin:'+env['PATH']
env['LD_LIBRARY_PATH']=f'{XV}/lib/x86_64-linux-gnu:{XV}/lib:'+env.get('LD_LIBRARY_PATH','')
name=sys.argv[1]
tmp=Path(env.get('TMPDIR',str(OUT/'tmp')))/'dfix3';tmp.mkdir(parents=True,exist_ok=True)
env.update(TMPDIR=str(tmp),CARGO_BUILD_JOBS='4',RUST_TEST_THREADS='8',
    FB_SCREENDIFF_TEMPLATE='fbv2_int_dfix1_df3ref',FB_SCREENDIFF_DB='fbv2_int_dfix1_df3screens',
    FB_SCREENDIFF_OUT=str(OUT/'screens'),FB_HARNESS_VAR=str(OUT/('reference-var-'+name)),
    FB_HARNESS_TEMPLATE='fbv2_core_df3ref',FB_HARNESS_DB='fbv2_core_df3',
    FB_HARNESS_OUT=str(OUT/'harness'),FB_SMOKE_SHOTS=str(OUT/('screenshots-'+name)),
    FB_FAKETIME_LIBRARY=str(ROOT.parents[1]/'staging/dfix2/faketime/usr-root/usr/lib/x86_64-linux-gnu/faketime/libfaketime.so.1'),
    FB_CLOCK_PROOF_OUT=str(OUT/'clock-proof'))
env.pop('CARGO_TARGET_DIR',None)
env.pop('FB_FIX6_CASES',None)
commands={
    'gate':['bash','scripts/gate.sh'],
    'screens':['bash','crates/fb-screendiff/run_screendiff.sh'],
    'harness':['bash','crates/fb-harness/run_harness.sh'],
    'core-gaps':['bash','crates/fb-harness/run_harness.sh','tools/fixtures/d7b-core-gaps.json'],
    'bank-rule-parity':['bash','crates/fb-harness/run_harness.sh','crates/fb-harness/scenarios/bankfeeds-random-500.json'],
    'smoke':['xvfb-run','-a','-s','-screen 0 1440x1000x24','node','scripts/smoke.cjs'],
    'smoke-secfix1':['xvfb-run','-a','-s','-screen 0 1440x1000x24','node','scripts/smoke-secfix1.cjs'],
    'package':['node','--test','fuses.test.cjs'],
    'native':['node','scripts/build-native.cjs'],
    'node-tests':['node','--test',*[str(p) for p in sorted((ROOT/'electron/tests').glob('*.test.cjs'))]],
    'clock':['bash','tools/dfix3_clock.sh'],
}
if name=='core-gaps':env['FB_HARNESS_OUT']=str(OUT/'core-gaps')
if name=='bank-rule-parity':env['FB_HARNESS_OUT']=str(OUT/'bank-rule-parity')
cwd=ROOT/('electron' if name in ('smoke','smoke-secfix1','native','node-tests') else 'packaging' if name=='package' else '.')
if name in ('smoke','smoke-secfix1'):
    assert 'SCREENS MATCH' in (OUT/'screens.log').read_text(), 'finish the standalone screen gate before smoke'
    fixture=OUT/(name+'-fixtures')
    if fixture.exists():shutil.rmtree(fixture)
    shutil.copytree(OUT/'screens/rust',fixture)
    env['FB_SMOKE_FIXTURE_DIR']=str(fixture)
log=OUT/(name+'.log')
print('START',name,flush=True)
with log.open('w') as output:
    result=subprocess.run(commands[name],cwd=cwd,env=env,stdout=output,stderr=subprocess.STDOUT)
print(f'{name}: exit {result.returncode}; log {log}',flush=True)
print('\n'.join(log.read_text().splitlines()[-20:]),flush=True)
sys.exit(result.returncode)
