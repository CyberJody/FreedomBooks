"""Foreground PRINT checks, isolated build/test resources, durable final output."""
from pathlib import Path
import os
import subprocess
import sys
ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT.parents[1]/'staging/printfix1'
OUT.mkdir(parents=True,exist_ok=True)
XV=ROOT.parent/'d1-desktop/electron/.tooling/xvfb/usr'
NODE=ROOT.parent/'d1-desktop/electron/.tooling/node-v24.22.2-linux-x64/bin'
env=dict(os.environ)
env['PATH']=f'{NODE}:{XV}/bin:'+env['PATH']
env['LD_LIBRARY_PATH']=f'{XV}/lib/x86_64-linux-gnu:{XV}/lib:'+env.get('LD_LIBRARY_PATH','')
tmp=Path(env.get('TMPDIR',str(OUT/'tmp')))/'print'
tmp.mkdir(parents=True,exist_ok=True)
env.update(TMPDIR=str(tmp),CARGO_BUILD_JOBS='4',RUST_TEST_THREADS='8',
    FB_SCREENDIFF_TEMPLATE='fbv2_core_prfref',FB_SCREENDIFF_DB='fbv2_core_prf',
    FB_SCREENDIFF_OUT=str(OUT/'screens'),FB_HARNESS_VAR=str(OUT/'reference-var'),
    FB_HARNESS_TEMPLATE='fbv2_core_prfref',FB_HARNESS_DB='fbv2_core_prf',
    FB_HARNESS_OUT=str(OUT/'harness'),FB_SMOKE_SHOTS=str(OUT),
    FB_TIME_ZONE='America/New_York',
    FB_FAKETIME_LIBRARY=str(ROOT.parents[1]/'staging/dfix2/faketime/usr-root/usr/lib/x86_64-linux-gnu/faketime/libfaketime.so.1'),
    FB_CLOCK_PROOF_OUT=str(OUT/'clock-proof'))
env.pop('CARGO_TARGET_DIR',None)
name=sys.argv[1]
commands={
    'check':['cargo','check','-p','fb-app','-p','fb-screendiff'],
    'printing-tests':['cargo','test','-p','fb-app','-p','fb-screendiff','-p','fb-templates','print'],
    'gate':['bash','scripts/gate.sh'],
    'clock':['bash','tools/dfix3_clock.sh'],
    'screens':['bash','crates/fb-screendiff/run_screendiff.sh'],
    'harness':['bash','crates/fb-harness/run_harness.sh'],
    'native':['npm','run','build:native'],
    'smoke':['xvfb-run','-a','-s','-screen 0 1440x1000x24','node','scripts/smoke-secfix1.cjs'],
    'smoke-register':['xvfb-run','-a','-s','-screen 0 1440x1000x24','node','scripts/smoke-print-register.cjs'],

    'node-tests':['npm','test'],
    'save-path':['node','scripts/save-path-regression.cjs'],
    'save-live':['xvfb-run','-a','-s','-screen 0 1440x1000x24','node','scripts/save-live-regression.cjs'],
    'cleanup':[sys.executable,'tools/print_cleanup.py'],
}
if name=='rediff':
    commands[name]=['bash','tools/print_rediff.sh']
if name=='compare':
    commands[name]=['bash','tools/print_compare.sh']
cwd=ROOT/'electron' if name in ('native','smoke','smoke-register','node-tests','save-path','save-live') else ROOT
if name in ('smoke','smoke-print'):
    env['FB_SMOKE_FIXTURE_DIR']=str(OUT/'screens/rust')
log=OUT/(name+'.log')
print('START',name,flush=True)
with log.open('w') as output:
    result=subprocess.run(commands[name],cwd=cwd,env=env,stdout=output,stderr=subprocess.STDOUT)
print(f'{name}: exit {result.returncode}; log {log}',flush=True)
print('\n'.join(log.read_text().splitlines()[-25:]),flush=True)
sys.exit(result.returncode)
