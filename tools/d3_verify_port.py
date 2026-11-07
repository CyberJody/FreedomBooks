"""All 13 D3 templates regenerate byte-for-byte; other waves remain unchanged."""
from pathlib import Path
import hashlib
import os
import subprocess
root=Path('crates/fb-templates/templates')
before={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*.html')}
subprocess.run(['python3','tools/d3_hand_fixes.py'],check=True,env={**os.environ,'PYTHONDONTWRITEBYTECODE':'1'})
after={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*.html')}
assert before==after,'template regeneration changed the checked-in output'
print('D3: 13 templates reproducible byte for byte; D1/D2 templates unchanged')
