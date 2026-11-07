"""Run FIX3c gates with dedicated evidence, databases, and temp files (FIX3b's adjustments, renamed resources)."""
from pathlib import Path
import os
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
source = (root / 'tools/dfix3_gates.py').read_text()
pairs = [
    ("staging/dfix3'", "staging/dfix3c'"),
    ("/'dfix3'", "/'dfix3c'"),
    ("'fbv2_core_df3ref'", "'fbv2_core_df3c_ref'"),
    ("'fbv2_core_df3'", "'fbv2_core_df3c'"),
    ("'fbv2_int_dfix1_df3ref'", "'fbv2_int_dfix1_df3c_ref'"),
    ("'fbv2_int_dfix1_df3screens'", "'fbv2_int_dfix1_df3c_screens'"),
    # Same as FIX3b: smoke reads gate.sh's screen artifacts too; npm builds native; skip normalized copies.
    ("assert 'SCREENS MATCH' in (OUT/'screens.log').read_text()",
     "assert any('SCREENS MATCH' in p.read_text() for p in [OUT/'screens.log', OUT/'gate.log'] if p.exists())"),
    ("'native':['node','scripts/build-native.cjs']", "'native':['npm','run','build:native']"),
    ("shutil.copytree(OUT/'screens/rust',fixture)",
     "shutil.copytree(OUT/'screens/rust',fixture,ignore=shutil.ignore_patterns('*.norm.html'))"),
    ('ROOT=Path(__file__).resolve().parents[1]', 'ROOT=Path(' + repr(str(root)) + ')'),
]
for old, new in pairs:
    assert source.count(old) == 1, old
    source = source.replace(old, new)
runner = root / 'target' / f'dfix3c_gate_runner-{sys.argv[1]}-{os.getpid()}.py'
runner.parent.mkdir(exist_ok=True)
runner.write_text(source)
sys.exit(subprocess.run(['python3', str(runner), *sys.argv[1:]], env=os.environ).returncode)
