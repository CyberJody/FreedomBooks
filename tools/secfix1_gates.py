"""Run SEC-FIX1 gates in the foreground and retain bounded, durable summaries."""
from pathlib import Path
import os, subprocess, sys

ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT.parents[1]/'staging/secfix1'
OUT.mkdir(parents=True,exist_ok=True)
XV=ROOT.parent/'d1-desktop/electron/.tooling/xvfb/usr'
NODE=ROOT.parent/'d1-desktop/electron/.tooling/node-v24.22.2-linux-x64/bin'
env=dict(os.environ)
env['PATH']=f'{NODE}:{XV}/bin:'+env['PATH']
env['LD_LIBRARY_PATH']=f'{XV}/lib/x86_64-linux-gnu:{XV}/lib:'+env.get('LD_LIBRARY_PATH','')
env.update(CARGO_BUILD_JOBS='3',RUST_TEST_THREADS=env.get('RUST_TEST_THREADS','4'),
    FB_SCREENDIFF_TEMPLATE='fbv2_int_dfix1_secfix1_ref',FB_SCREENDIFF_DB='fbv2_int_dfix1_secfix1_screens',
    FB_SCREENDIFF_OUT=str(OUT/'screens'),FB_HARNESS_VAR=str(OUT/('reference-var-'+sys.argv[1])),
    FB_HARNESS_TEMPLATE='fbv2_core_secfix1_ref',FB_HARNESS_DB='fbv2_core_secfix1_run',
    FB_HARNESS_OUT=str(OUT/'harness'),FB_SMOKE_SHOTS=str(OUT/'screenshots'))
env.pop('CARGO_TARGET_DIR',None)
env.pop('FB_FIX6_CASES',None)  # The full gate always runs all four 1,280-case seeds.
commands={
    'gate':['bash','scripts/gate.sh'],
    'screens':['bash','crates/fb-screendiff/run_screendiff.sh'],
    'harness':['bash','crates/fb-harness/run_harness.sh'],
    'smoke':['xvfb-run','-a','-s','-screen 0 1440x1000x24','node','scripts/smoke-secfix1.cjs'],
    'smoke-d8':['xvfb-run','-a','-s','-screen 0 1440x1000x24','node','scripts/smoke-d8.cjs'],
    'package':['node','--test','fuses.test.cjs'],
    'native':['node','scripts/build-native.cjs'],
}
name=sys.argv[1]
cwd=ROOT/('electron' if name in ('smoke','smoke-d8','native') else 'packaging' if name=='package' else '.')
log=OUT/f'{name}.log'
with log.open('w') as output:
    result=subprocess.run(commands[name],cwd=cwd,env=env,stdout=output,stderr=subprocess.STDOUT)
print(f'{name}: exit {result.returncode}; log {log}',flush=True)
print('\n'.join(log.read_text().splitlines()[-18:]),flush=True)
sys.exit(result.returncode)
