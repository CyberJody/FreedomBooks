"""BACKUPS-POLISH: the gates after gate.sh, one after another (stops at the first failure)."""
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
for step in ['harness', 'native', 'smoke', 'smoke-secfix1', 'npm-test', 'r11']:
    r = subprocess.run([sys.executable, str(ROOT / 'tools/backups_polish_gates.py'), step], cwd=ROOT)
    if r.returncode != 0:
        print(f'SEQUENCE STOPPED at {step}', flush=True)
        sys.exit(r.returncode)
print('SEQUENCE OK', flush=True)
