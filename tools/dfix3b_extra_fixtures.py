"""Cover valid grammar reaching later refusals, and input-size edges."""
from pathlib import Path
from copy import deepcopy
import json
root = Path(__file__).resolve().parents[1]
p = root/'crates/fb-screendiff/screens.json'
s = json.loads(p.read_text())
base = s['dfix3b-begin-revision-before-account']
for i, value in enumerate(['1', '', '0', '1000000000', '+1', ' 1 ', '１', '0_1']):
    item=deepcopy(base)
    item['post']['revision']=value
    s[f'dfix3b-begin-valid-revision-{i}']=item
base = s['d7b-screen-invalid-shown']
for name, shown in [('count', ','.join(['1:0']*901))]:
    item=deepcopy(base)
    item['post']['shown']=shown
    s['dfix3b-shown-cap-'+name]=item
s={k:v for k,v in s.items() if not k.startswith('dfix3b-report-open-before-')}
p.write_text(json.dumps(s,indent=2,ensure_ascii=False)+'\n')
print(f'{len(s)} total fixtures')
