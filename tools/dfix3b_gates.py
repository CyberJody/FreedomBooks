"""Run FIX3b gates with dedicated evidence, databases, and temp files."""
from pathlib import Path
import os
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
source = (root / 'tools/dfix3_gates.py').read_text()
source = source.replace("staging/dfix3", "staging/dfix3b").replace("/'dfix3'", "/'dfix3b'")
source = source.replace('fbv2_core_df3', 'fbv2_core_df3b')
source = source.replace('fbv2_int_dfix1_df3', 'fbv2_int_dfix1_df3b')
# Smoke consumes the screen artifacts produced by gate.sh as well as the standalone runner.
source = source.replace("assert 'SCREENS MATCH' in (OUT/'screens.log').read_text()", "assert any('SCREENS MATCH' in p.read_text() for p in [OUT/'screens.log', OUT/'gate.log'] if p.exists())")
source = source.replace("'native':['node','scripts/build-native.cjs']", "'native':['npm','run','build:native']")
source = source.replace("shutil.copytree(OUT/'screens/rust',fixture)", "shutil.copytree(OUT/'screens/rust',fixture,ignore=shutil.ignore_patterns('*.norm.html'))")
# Retain a real script on disk; no dynamic inline execution.
runner = root / 'target' / f'dfix3b_gate_runner-{sys.argv[1]}-{os.getpid()}.py'
runner.parent.mkdir(exist_ok=True)
runner.write_text(source.replace('ROOT=Path(__file__).resolve().parents[1]', 'ROOT=Path(' + repr(str(root)) + ')'))
sys.exit(subprocess.run(['python3', str(runner), *sys.argv[1:]], env=os.environ).returncode)
