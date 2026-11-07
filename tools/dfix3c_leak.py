"""Replay review 11 on committed FIX3c, retaining separate evidence (FIX3b's runner, renamed paths)."""
from pathlib import Path
import os
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
source = (root / 'tools/dfix3_leak.py').read_text()
pairs = [
    ("root=Path(__file__).resolve().parents[1]", 'root=Path(' + repr(str(root)) + ')'),
    ('staging/dfix3/leak', 'staging/dfix3c/leak'),
    ('archive.unlink()', "archive.unlink()\nshutil.copy2(root/'Cargo.lock',scratch/'Cargo.lock')"),
]
for old, new in pairs:
    assert source.count(old) == 1, old
    source = source.replace(old, new)
runner = root / 'target/dfix3c_leak_runner.py'
runner.parent.mkdir(exist_ok=True)
runner.write_text(source)
env = dict(os.environ, CARGO_TARGET_DIR=str(root / 'target/dfix3c-r11'))
sys.exit(subprocess.run(['python3', str(runner)], env=env).returncode)
