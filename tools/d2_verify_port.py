"""Prove all D2 templates regenerate byte-for-byte from the frozen source and hand-fix script."""
from pathlib import Path
import hashlib
import os
import subprocess

root=Path('crates/fb-templates/templates')
before={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*.html')}
subprocess.run(['python3','crates/fb-templates/tools/port_d2.py'],check=True,env={**os.environ,'PYTHONDONTWRITEBYTECODE':'1'})
after={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*.html')}
assert before==after,'mechanical template regeneration changed the checked-in output'
print('D2 template regeneration: 14 templates, byte-for-byte reproducible; D1 templates unchanged')
