"""Run DESKTOP-JODY1 gates with dedicated evidence, databases and temp files (FIX3c's runner, renamed resources).

Screen-diff DB names must use an approved screen-diff prefix (fbv2_int_dfix1_*); the harness uses the
brief's fbv2_core_dj1_ref / fbv2_core_dj1. A standalone 'screens' run uses its own DBs and output so it can
run while gate.sh is busy with the workspace tests.
"""
from pathlib import Path
import os
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
source = (root / 'tools/dfix3_gates.py').read_text()
pairs = [
    ("staging/dfix3'", "staging/desktop-jody1'"),
    ("/'dfix3'", "/'desktop-jody1'"),
    ("'fbv2_core_df3ref'", "'fbv2_core_dj1_ref'"),
    ("'fbv2_core_df3'", "'fbv2_core_dj1'"),
    ("'fbv2_int_dfix1_df3ref'", "('fbv2_int_dfix1_dj1s_ref' if name=='screens' else 'fbv2_int_dfix1_dj1_ref')"),
    ("'fbv2_int_dfix1_df3screens'", "('fbv2_int_dfix1_dj1s_screens' if name=='screens' else 'fbv2_int_dfix1_dj1_screens')"),
    ("FB_SCREENDIFF_OUT=str(OUT/'screens')", "FB_SCREENDIFF_OUT=str(OUT/('screens' if name in ('screens','smoke','smoke-secfix1') else 'gate-screens'))"),
    ("assert 'SCREENS MATCH' in (OUT/'screens.log').read_text()",
     "assert 'SCREENS MATCH' in (OUT/'screens.log').read_text()"),
    ("'native':['node','scripts/build-native.cjs']", "'native':['npm','run','build:native']"),
    ("shutil.copytree(OUT/'screens/rust',fixture)",
     "shutil.copytree(OUT/'screens/rust',fixture,ignore=shutil.ignore_patterns('*.norm.html'))"),
    ("'node-tests':['node','--test',", "'npm-test':['npm','test'],\n    'node-tests':['node','--test',"),
    ("('smoke','smoke-secfix1','native','node-tests')", "('smoke','smoke-secfix1','native','node-tests','npm-test')"),
    ('ROOT=Path(__file__).resolve().parents[1]', 'ROOT=Path(' + repr(str(root)) + ')'),
]
for old, new in pairs:
    assert source.count(old) == 1, old
    source = source.replace(old, new)
# 'name' must be known before the env block uses it.
source = source.replace("name=sys.argv[1]\n", "")
source = source.replace("env=dict(os.environ)\n", "name=sys.argv[1]\nenv=dict(os.environ)\n", 1)
assert source.count("name=sys.argv[1]") == 1
runner = root / 'target' / f'djody1_gate_runner-{sys.argv[1]}-{os.getpid()}.py'
runner.parent.mkdir(exist_ok=True)
runner.write_text(source)
sys.exit(subprocess.run(['python3', str(runner), *sys.argv[1:]], env=os.environ).returncode)
