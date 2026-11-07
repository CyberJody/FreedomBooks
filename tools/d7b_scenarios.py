"""Append D7b evidence without rewriting prior waves' cases."""
from pathlib import Path
import json
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'crates/fb-screendiff/scenarios/screens-dfix2.json'
scenario=json.loads(p.read_text())
scenario['name']='screens-d7b'
steps=[]
def add(op,**kw): steps.append(dict(op=op,company='c',expect_error=False,**kw))
def key(n): return f'00000000-0000-0000-0000-{0xd7b000+n:012x}'
for ref,code,name,detail in [('d7b-bank','1070','Statement bank','BANK'),('d7b-card','2170','Statement card','CREDIT_CARD')]:
    add('save_account',ref=ref,code=code,name=name,detail_type=detail)
for ref,date,lines in [
    ('d7b-opening','2026-09-01',[dict(account='d7b-bank',debit='1000.00',name='Opening'),dict(account='income',credit='1000.00')]),
    ('d7b-payment','2026-09-15',[dict(account='expense',debit='200.00'),dict(account='d7b-bank',credit='200.00',name='Statement payee')]),
    ('d7b-next','2026-10-01',[dict(account='d7b-bank',debit='100.00',name='Autumn deposit'),dict(account='income',credit='60.00'),dict(account='income',credit='40.00')]),
    ('d7b-later','2026-12-01',[dict(account='d7b-bank',debit='10.00'),dict(account='income',credit='10.00')]),
    ('d7b-charge','2026-09-01',[dict(account='expense',debit='50.00'),dict(account='d7b-card',credit='50.00')]),
    ('d7b-card-payment','2026-09-15',[dict(account='d7b-card',debit='5.00'),dict(account='bank',credit='5.00')]),
]:
    add('save_draft',ref=ref,date=date,memo=ref,lines=lines)
    add('post_journal',journal=ref,expected_revision=1)
for n in range(51):
    ref=f'd7b-page-{n}'
    add('save_draft',ref=ref,date='2026-11-15',memo=f'Pagination fixture {n}',lines=[dict(account='expense',debit='1000.00'),dict(account='d7b-bank',credit='1000.00')])
    add('post_journal',journal=ref,expected_revision=1)
for n,(acct,ref,ending,shown) in enumerate([
    ('d7b-bank','d7b-complete','800.00',{'d7b-opening:1':0,'d7b-payment:2':0}),
    ('d7b-card','d7b-card-complete','45.00',{'d7b-charge:2':0,'d7b-card-payment:1':0})
]):
    add('banking_begin',account=acct,ref=ref,values=dict(request_key=key(n*2),statement_date='2026-09-30',ending_balance=ending))
    add('banking_ticks',account=acct,reconciliation=ref,expected_revision=1,shown=shown,ticked=list(shown),values={})
    add('banking_finish',account=acct,reconciliation=ref,expected_revision=2,completion_key=key(n*2+1))
add('banking_begin',account='d7b-bank',ref='d7b-current',values=dict(request_key=key(5),statement_date='2026-10-31',ending_balance='900.00'))
add('banking_begin',account='d7b-card',ref='d7b-card-current',values=dict(request_key=key(6),statement_date='2026-10-31',ending_balance='45.00'))
scenario['steps']+=steps
(ROOT/'crates/fb-screendiff/scenarios/screens-d7b.json').write_text(json.dumps(scenario,indent=2)+'\n')

screens_path=ROOT/'crates/fb-screendiff/screens.json'
screens=json.loads(screens_path.read_text())
new={}
def screen(name,path,post=None,status=200,**kw):
    case=dict(path='/c/{company}/'+path,status=status,**kw)
    if post is not None:case['post']=post
    new['d7b-'+name]=case
bank='accounts/{account:1070}/register/'
begin='reconcile/{account:1070}/begin/'
active='reconcile/{account:1070}/'
history='reconcile/{account:1070}/history/'
undo='reconcile/{account:1070}/undo/'
report='reconcile/reports/{reconciliation:1070:2026-09-30}/'
# Exact paths are drawn from the frozen URL table below, rather than guessed.
routes=json.loads((ROOT/'crates/fb-templates/urls.json').read_text())
print({k:v for k,v in routes.items() if k.startswith('reconcile:')})
def route(name,account='1070',rec='1070:2026-09-30'):
    p=routes[name].replace('<int:company_id>','{company}').replace('<int:account_id>','{account:'+account+'}').replace('<int:reconciliation_id>','{reconciliation:'+rec+'}')
    return p.removeprefix('/c/{company}/')
begin=route('reconcile:begin');active=route('reconcile:screen');history=route('reconcile:history');undo=route('reconcile:undo');report=route('reconcile:report')
screen('registers','registers/')
screen('register',bank)
screen('register-provenance','accounts/{account:1000}/register/')
screen('register-card','accounts/{account:2170}/register/')
screen('register-range',bank+'?start=2026-10-01&end=2026-10-31')
screen('register-filters',bank+'?state=U&payee=Autumn&amount=$100.00')
screen('register-month',bank+'?preset=this_month')
screen('register-invalid',bank+'?preset=bad&start=bad&end=1899-01-01&state=bad&amount=1.234&output=no',status=400)
screen('register-reverse-range',bank+'?start=2026-10-31&end=2026-10-01',status=400)
screen('chooser',route('reconcile:list'))
screen('chooser-left',route('reconcile:list')+'?left={account:1070}')
screen('begin',route('reconcile:begin',account='1000'))
screen('begin-change',begin)
screen('begin-card',route('reconcile:begin',account='2170'))
screen('begin-invalid',begin,dict(statement_date='bad',ending_balance='',service_charge='1'*25,request_key=''),400)
screen('begin-invalid-amount',begin,dict(statement_date='2026-10-31',ending_balance='bad',request_key=key(30),revision='1'),400)
screen('screen',active)
screen('screen-saved',active+'?saved=1')
screen('screen-card',route('reconcile:screen',account='2170'))
screen('screen-invalid-action',active,dict(action='no'),400)
screen('screen-invalid-shown',active,dict(action='save',reconciliation='{reconciliation:1070:2026-10-31}',revision='1',shown='bad'),400)
screen('screen-difference',active,dict(action='reconcile',reconciliation='{reconciliation:1070:2026-10-31}',revision='1',shown='',completion_key=key(31)),400)
screen('history',history)
screen('report',report)
screen('report-summary',report+'?view=summary&done=1')
screen('report-card',route('reconcile:report',account='2170',rec='2170:2026-09-30'))
screen('report-undone','/unused')
new['d7b-report-undone']['path']='/c/{company:D8 report books}/'+route('reconcile:report',rec='1000:2026-10-31')
# Reconciliations in the D8 book gain their own scoped names in each renderer.
new['d7b-report-undone']['path']=new['d7b-report-undone']['path'].replace('{reconciliation:1000:2026-10-31}','{reconciliation:D8 report books:100:2026-10-31}')
screen('report-invalid',report+'?view=bad',status=400)
screen('undo',undo)
screen('undo-invalid',undo,dict(reconciliation='{reconciliation:1070:2026-09-30}',revision='3',undo_key=key(32),reason=''),400)
new['d7b-cleared-invalid']=dict(path=routes['registers:cleared'].replace('<int:company_id>','{company}').replace('<int:account_id>','{account:1070}'),post=dict(line='{line:first:1070}',state='R',revision='2'),status=400)
for source in ['d7b-register','d7b-register-card','d7b-register-filters','d7b-report','d7b-report-summary','d7b-report-undone']:
    case=new[source].copy();case['path']+=('&' if '?' in case['path'] else '?')+'output=csv';case['media']='csv'
    new[source+'-csv']=case
for name,value in [('page-two','2'),('page-unicode','%D9%A2'),('page-huge','9'*60),('page-control','%1C2')]:
    new['d7b-register-'+name]=dict(path='/c/{company}/'+bank+'?page='+value,status=200)
for name,case in new.items():
    if name in screens:assert screens[name]==case,(name,screens[name],case)
screens.update(new)
screens_path.write_text(json.dumps(screens,indent=2)+'\n')
