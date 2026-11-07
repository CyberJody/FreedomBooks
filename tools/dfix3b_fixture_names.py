"""Keep screen snapshot routes within the existing lowercase/hyphen grammar."""
from pathlib import Path
import json
import re
root=Path(__file__).resolve().parents[1]
p=root/'crates/fb-screendiff/screens.json'
s=json.loads(p.read_text())
renamed={k.replace('_','-') if k.startswith('dfix3b-') else k:v for k,v in s.items()}
assert len(renamed)==len(s)
assert all(re.fullmatch('[a-z0-9-]+',name) for name in renamed)
p.write_text(json.dumps(renamed,indent=2,ensure_ascii=False)+'\n')
out=root.parents[1]/'staging/dfix3b'
# Preserve the already-MATCHed render bytes under their corrected fixture names;
# gate.sh will independently regenerate and compare the final fixture catalogue.
for folder in [out/'screens/rust',out/'screens/django']:
    for source in folder.glob('dfix3b-*'):
        if '_' in source.name:
            source.rename(source.with_name(source.name.replace('_','-')))
p=root/'crates/fb-core/RECONCILE-VALIDATORS.md'
t=p.read_text()
for old in ['statement_date','ending_balance','service_charge','request_key','service_account','interest_account']:
    t=t.replace(old,old.replace('_','-'))
p.write_text(t)
print(f'{len(renamed)} fixture names now fit the existing snapshot route grammar')
