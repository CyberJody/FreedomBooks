"""Run part B's gates one after another (never in parallel); stop at the first failure.

Usage: setsid nohup python3 tools/inthotfix_b_sequence.py [first-step] > staging/int-hotfix/partB/sequence.out 2>&1 &
"""
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
OUT = Path('/mnt/Data/Projects/FreedomBooks/staging/int-hotfix/partB')
STEPS = ['gate', 'harness', 'native', 'smoke', 'smoke-secfix1', 'npm-test', 'r11']
start = STEPS.index(sys.argv[1]) if len(sys.argv) > 1 else 0
OUT.mkdir(parents=True, exist_ok=True)
for step in STEPS[start:]:
    began = time.time()
    print(f'== {step} start {time.strftime("%H:%M:%S")}', flush=True)
    code = subprocess.run(['python3', str(ROOT / 'tools/inthotfix_b_gates.py'), step], cwd=ROOT).returncode
    print(f'== {step} exit {code} after {int(time.time() - began)}s', flush=True)
    if code != 0:
        (OUT / 'sequence.exit').write_text(f'{step} {code}\n')
        sys.exit(code)
(OUT / 'sequence.exit').write_text('all 0\n')
print('== ALL GATES PASSED', flush=True)
