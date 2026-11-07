"""List and first-enable previews exercise Django's NFC-aware truncation."""
from pathlib import Path
import json
p=Path('crates/fb-screendiff/scenarios/screens-d3.json');s=json.loads(p.read_text())
for row in s['steps']:
    if row.get('ref')=='goods':row['values']['description']='Grass e\u0301 seed · '*14
p.write_text(json.dumps(s,indent=2)+'\n')
