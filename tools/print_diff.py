"""Append PRINT coverage and the narrowly-scoped desktop print capability normalization."""
from pathlib import Path
import json
ROOT=Path(__file__).resolve().parents[1]
def edit(name,old,new):
    p=ROOT/name;text=p.read_text();assert text.count(old)==1,(name,old);p.write_text(text.replace(old,new))
for name in ['source','identity','method']:
    edit('crates/fb-app/src/screens/d5/records.rs',f'pub(super) fn {name}(',f'pub(in crate::screens) fn {name}(')
edit('crates/fb-screendiff/src/normalize.rs','mod literal;','mod literal;\nmod printing;')
edit('crates/fb-screendiff/src/normalize.rs','    let html=d3::normalize(html,ids);','    let html=printing::normalize(html);\n    let html=d3::normalize(&html,ids);')
p=ROOT/'crates/fb-screendiff/screens.json';screens=json.loads(p.read_text())
for key,screen in list(screens.items()):
    path=screen.get('path','')
    if screen.get('post') or screen.get('status',200)!=200 or screen.get('media') or screen.get('template'):
        continue
    # Every existing document-detail/revision state becomes a printable snapshot comparison.
    import re
    if re.fullmatch(r'/c/[^/]+/(?:estimates|invoices|sales-receipts|customer-deposits)/[^/]+/(?:revisions/\d+/)?',path):
        if '/new/' not in path: screens['print-'+key]={'path':path+'print/'}
# Reports: retain the full D8 filter/basis/range matrix and the banking variants.
for key,screen in list(screens.items()):
    path=screen.get('path','')
    if screen.get('post') or screen.get('status',200)!=200 or screen.get('media') or screen.get('template') or key.startswith('print-'):
        continue
    if '/reports/' in path or key.startswith('d7b-report') or key in ['d7b-register','d7b-register-provenance','d7b-register-card','d7b-register-range','d7b-register-filters','d7b-register-month']:
        screens['print-'+key]={'path':path+('&' if '?' in path else '?')+'output=print'}
p.write_text(json.dumps(screens,indent=2)+'\n')
print('PRINT: full print views',sum(k.startswith('print-') and 'template' not in v for k,v in screens.items()),'; total',len(screens))
