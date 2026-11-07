"""Build a genuine completed 2,001-line statement through ordinary services."""
from pathlib import Path
import json
root=Path(__file__).resolve().parents[1]
p=root/'crates/fb-screendiff/scenarios/screens-d7b.json'
scenario=json.loads(p.read_text())
assert not any(s.get('ref')=='df3b-cap-company' for s in scenario['steps'])
steps=[]
def add(op, **kw):
    steps.append(dict(op=op,company='df3b-cap-company',expect_error=False,**kw))
add('create_company',ref='df3b-cap-company',name='FIX3b report cap',currency='USD')
add('save_account',ref='df3b-cap-bank',code='100',name='Cap bank',detail_type='BANK')
add('save_account',ref='df3b-cap-income',code='400',name='Cap income',detail_type='INCOME')
line_names=[]
remaining=2001
batch=0
while remaining:
    count=min(remaining,49)
    name=f'df3b-cap-journal-{batch}'
    lines=[dict(account='df3b-cap-bank',debit='1.00',credit='0.00') for _ in range(count)]
    lines.append(dict(account='df3b-cap-income',debit='0.00',credit=f'{count}.00'))
    add('save_draft',ref=name,date='2026-10-01',memo='Report cap synthetic',lines=lines)
    add('post_journal',journal=name)
    line_names += [f'{name}:{n}' for n in range(1,count+1)]
    remaining-=count
    batch+=1
add('banking_begin',ref='df3b-cap-statement',account='df3b-cap-bank',values=dict(request_key='00000000-0000-0000-0000-000000df3b10',statement_date='2026-10-31',ending_balance='2001.00'))
revision=1
for start in range(0,len(line_names),900):
    names=line_names[start:start+900]
    add('banking_ticks',account='df3b-cap-bank',reconciliation='df3b-cap-statement',expected_revision=revision,shown=dict.fromkeys(names,0),ticked=names)
    revision+=1
add('banking_finish',account='df3b-cap-bank',reconciliation='df3b-cap-statement',expected_revision=revision,completion_key='00000000-0000-0000-0000-000000df3b11')
scenario['steps']+=steps
p.write_text(json.dumps(scenario,indent=2)+'\n')
p=root/'crates/fb-screendiff/screens.json'
screens=json.loads(p.read_text())
url='/c/{company:FIX3b report cap}/reconciliations/{reconciliation:FIX3b report cap:100:2026-10-31}/'
for view in ['detail','summary']:
    for output in ['print','csv']:
        screens[f'dfix3b-report-cap-{view}-{output}']=dict(path=url+f'?view={view}&output={output}',status=400)
screens['dfix3b-report-choice-before-cap']=dict(path=url+'?view=bad&output=csv',status=400)
p.write_text(json.dumps(screens,indent=2,ensure_ascii=False)+'\n')
print(f'Added {len(steps)} successful setup steps for the report cap; {len(screens)} screens')
