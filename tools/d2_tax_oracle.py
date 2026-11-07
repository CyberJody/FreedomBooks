"""Call the actual frozen regex for the source tests plus adversarial Unicode word edges."""
from pathlib import Path
import ast
import json
import os
import sys

sys.path.insert(0,os.environ['FB_REFERENCE'])
os.environ.setdefault('DJANGO_SETTINGS_MODULE','config.settings')
import django
django.setup()
from documents.first_run import TAX_NAME

module=ast.parse(Path(os.environ['FB_REFERENCE'],'documents/test_corefx_tax_name.py').read_text())
names=[]
for statement in module.body:
    if isinstance(statement,ast.Assign) and any(isinstance(t,ast.Name) and t.id in ['ACCEPTED','REJECTED'] for t in statement.targets):
        names.extend(ast.literal_eval(statement.value))
for prefix in ['', 'é', '\u0301', '_', '²', 'K', 'İ', 'ı', 'ſ', '\x1c']:
    for word in ['Sales tax','ſales tax','VAT','Payroll sales tax']:
        names.extend([prefix+word,word+prefix])
Path('crates/fb-core/tests/data/d2-tax-names.json').write_text(json.dumps([[n,bool(TAX_NAME.search(n))] for n in names],ensure_ascii=True,indent=2)+'\n')
