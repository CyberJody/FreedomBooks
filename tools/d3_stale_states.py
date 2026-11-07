"""Stale defaults retain their grouped widget choices but stay invalid when submitted unchanged."""
from pathlib import Path
import json
p=Path('crates/fb-screendiff/scenarios/screens-d3.json');s=json.loads(p.read_text())
steps=[
 dict(op='save_account',ref='stale-expense',code='5999',name='Old expense',detail_type='EXPENSE'),
 dict(op='l2_save',model='vendor',ref='stale-vendor',values=dict(display_name='Stale vendor',default_expense_account='stale-expense')),
 dict(op='l2_save',model='item',ref='stale-item',values=dict(code='STALE',description='Old default',kind='SERVICE',sale_rate='5',expense_account='stale-expense')),
 dict(op='save_account',account='stale-expense',code='5999',name='Old expense',detail_type='EXPENSE',active=False),
]
s['steps'] += [dict(company='activity',expect_error=False,**step) for step in steps];p.write_text(json.dumps(s,indent=2)+'\n')
p=Path('crates/fb-screendiff/screens.json');s=json.loads(p.read_text())
base='/c/{company:Activity synthetic books}/lists/'
for model,plural,key,post in [('vendor','vendors','Stale vendor',dict(display_name='Stale vendor',default_expense_account='{account:Activity synthetic books:5999}')),('item','items','STALE',dict(code='STALE',kind='SERVICE',sale_rate='5',expense_account='{account:Activity synthetic books:5999}'))]:
    path=base+plural+'/{'+model+':Activity synthetic books:'+key+'}/edit/'
    s['directory-'+model+'-stale']={'path':path}
    s['directory-'+model+'-stale-invalid']={'path':path,'post':post,'status':400}
s['directory-email-validation-order']={'path':'/c/{company}/lists/customers/new/','post':{'display_name':'Test','email':'a'*255},'status':400}
p.write_text(json.dumps(s,indent=2)+'\n')
# Generated timestamps come from the actual audit inventory of every fixture company.
p=Path('crates/fb-screendiff/py/render_django.py');t=p.read_text();old='for event in AuditEvent.objects.filter(company=company):';assert old in t;t=t.replace(old,'for event in AuditEvent.objects.all():');p.write_text(t)
p=Path('crates/fb-screendiff/src/main.rs');t=p.read_text();old='''let mut st=tx.prepare("SELECT occurred_at FROM ledger_auditevent WHERE company_id=?1")?;
            for stamp in st.query_map([&company],''';assert old in t;t=t.replace(old,'''let mut st=tx.prepare("SELECT occurred_at FROM ledger_auditevent")?;
            for stamp in st.query_map([],''');p.write_text(t)
