"""Keep valid-input parity fixtures within SEC-FIX1's free-text input contract.

Replace LF/CR only in explicit free-text fixture fields, on BOTH runners' input.
No dump/comparer/output normalization is involved. Dedicated security tests retain
LF/CR and assert refusal and no writes. Whitespace-only formset rows remain blank.
"""
from pathlib import Path
import json, re, sys
root=Path(__file__).resolve().parents[1]
fields={'name','display_name','business_name','billing_address','address','notes','memo','description','reason','payee_name','tax_label'}
files=sorted((root/'crates/fb-harness/scenarios').glob('*.json'))+sorted((root/'crates/fb-screendiff/scenarios').glob('*.json'))
for path in files:
    data=json.loads(path.read_text());edits=[]
    def visit(value,location=''):
        if isinstance(value,dict):
            for key,child in value.items():
                field=re.sub(r'^lines-\d+-','',key)
                if field in fields and isinstance(child,str) and any(c in child for c in '\r\n'):
                    edits.append(location+'/'+key)
                    value[key]=child.replace('\r\n','\t').replace('\n','\t').replace('\r','\t')
                else:visit(child,location+'/'+key)
        elif isinstance(value,list):
            for i,child in enumerate(value):visit(child,location+f'/{i}')
    visit(data)
    if edits:
        print(path.relative_to(root),len(edits),', '.join(edits[:3]))
        if '--write' in sys.argv:path.write_text(json.dumps(data,ensure_ascii=False,indent=2)+'\n')
