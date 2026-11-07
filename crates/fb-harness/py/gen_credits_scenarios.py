"""Workflow coverage and an exactly 500-command randomized credit lifecycle."""
import json,uuid,random,copy
from pathlib import Path
OUT=Path(__file__).resolve().parents[1]/'scenarios'
def key(n):return str(uuid.UUID(int=600000+n))
def cmd(op,**v):return dict(op='credits_'+op,company='c',**v)
def setup():
 s=[dict(op='create_company',ref='c',name='Synthetic credits')]
 for ref,code,kind in [('bank','100','ASSET'),('bank2','101','ASSET'),('ar','110','ASSET'),('ar2','111','ASSET'),('uf','120','ASSET'),('ap','200','LIABILITY'),('tax','210','LIABILITY'),('income','400','INCOME'),('income2','410','INCOME'),('expense','500','EXPENSE')]:s.append(dict(op='save_account',company='c',ref=ref,code=code,name=ref,kind=kind))
 s.extend([dict(op='l2_save',company='c',model='customer',ref='buyer',values=dict(display_name='Buyer')),dict(op='l2_save',company='c',model='customer',ref='buyer2',values=dict(display_name='Other buyer')),dict(op='l2_save',company='c',model='item',ref='work',values=dict(code='WORK',description='Synthetic work',kind='SERVICE',sale_rate='9.99',income_account='income',expense_account='expense')),dict(op='l2_setup',company='c',expected_revision=0,values=dict(receivables='ar',payables='ap',undeposited='uf',collected_tax='tax',banks=['bank','bank2']))]);return s
def memo(n,**kw):
 v=dict(request_key=key(n),customer_id='buyer',date='2026-10-08',memo='Returned synthetic service',tax_mode='MANUAL',tax_rate='8',tax_label='Tax',lines=[dict(item_id='work',description='Taxable return',quantity='1',rate='10.00',taxable=True,income_account_id='income'),dict(item_id=None,description='Untaxed',quantity='1',rate='2.50',taxable=False,income_account_id='income2')]);v.update(kw);return v
def record(s,n,p=None):
 p=p or memo(n);name='cm'+str(n);s.extend([cmd('review_memo',values=p,review=name),cmd('record_memo',values=p,review=name,ref=name)]);return name
def invoice(s,n,**kw):
 p=memo(n,date='2026-10-01',memo='Synthetic invoice',**kw);p['lines'][0]['rate']='30.00';name='inv'+str(n)
 s.extend([dict(op='sales_save_invoice',company='c',ref=name,values=p),dict(op='sales_review_invoice',company='c',record=name,expected_revision=1,review=name),dict(op='sales_post_invoice',company='c',record=name,expected_revision=1,review=name)]);return name
def application(n,inv,amount='1.00',**kw):
 v=dict(request_key=key(n),date='2026-10-09',allocations=[dict(invoice_id=inv,amount=amount)]);v.update(kw);return v
def apply(s,n,m,inv,amount='1.00'):
 p=application(n,inv,amount);name='app'+str(n);s.extend([cmd('review_application',record=m,values=p,review=name),cmd('apply',record=m,values=p,review=name,ref=name)]);return name,p
def refund(n,amount='2.00',**kw):
 v=dict(request_key=key(n),date='2026-10-11',amount=amount,bank_id='bank',method='CHECK',check_number=str(n),memo='Synthetic refund');v.update(kw);return v
def undo(op,name,date='2026-10-20',reason='Synthetic correction',**kw):return cmd(op,record=name,expected_revision=1,date=date,reason=reason,**kw)
def save(name,s):
 (OUT/f'credits-{name}.json').write_text(json.dumps(dict(users=[dict(username='credits',is_staff=True),dict(username='outsider')],steps=s,as_of=['2026-10-01','2026-10-08','2026-10-11','2026-10-20','2026-12-31']),ensure_ascii=False,indent=1)+'\n')

s=setup();inv=invoice(s,1);p=memo(2);m=record(s,2,p);s.append(cmd('record_memo',ref=m,values=p,review=m));a,ap=apply(s,3,m,inv,'5.00');rp=refund(4);s.extend([cmd('review_refund',record=m,values=rp,review='r4'),cmd('refund',record=m,values=rp,review='r4',ref='r4'),cmd('refund',record=m,values=rp,review='r4',ref='r4'),undo('void_memo',m,expect_error=True),undo('unapply',a),undo('unapply',a),cmd('apply',record=m,values=ap,review=a,ref=a),undo('void_memo',m,expect_error=True),undo('void_refund','r4'),undo('void_refund','r4'),cmd('refund',record=m,values=rp,review='r4',ref='r4'),undo('void_memo',m,date='2026-10-19',expect_error=True),undo('void_memo',m),undo('void_memo',m),cmd('record_memo',ref=m,values=p,review=m)]);save('lifecycle',s)

s=setup();inv=invoice(s,10);m=record(s,11)
for n,p in enumerate([memo(20,lines=[]),memo(21,tax_mode='MANUAL',tax_label=''),memo(22,customer_id='buyer2',lines=[dict(item_id=None,description='x',quantity='0',rate='1',taxable=False,income_account_id='income')]),memo(23,date='garbage')]):s.append(cmd('review_memo',values=p,review='bad'+str(n),expect_error=True))
for n,p in enumerate([application(30,inv,'13.31'),application(31,inv,'1',date='2026-10-07'),application(32,inv,'0'),application(33,inv,'1',allocations=[dict(invoice_id=inv,amount='1'),dict(invoice_id=inv,amount='1')])]):s.append(cmd('review_application',record=m,values=p,review='badapp'+str(n),expect_error=True))
for n,p in enumerate([refund(40,'13.31'),refund(41,method='CASH',check_number='12'),refund(42,bank_id='uf'),refund(43,date='2026-10-01')]):s.append(cmd('review_refund',record=m,values=p,review='badr'+str(n),expect_error=True))
s.append(cmd('record_memo',values=memo(50),review='missing',ref='no',expect_error=True));save('refusals',s)

s=setup();inv=invoice(s,60);m=record(s,61);p=application(62,inv);s.append(cmd('review_application',record=m,values=p,review='stale'));a,_=apply(s,63,m,inv,'2.00');s.append(cmd('apply',record=m,values=p,review='stale',ref='stale',expect_error=True));s.append(undo('unapply',a));
rp=refund(64);s.append(cmd('review_refund',record=m,values=rp,review='stale-refund'));a,_=apply(s,65,m,inv,'1.00');s.append(cmd('refund',record=m,values=rp,review='stale-refund',ref='no',expect_error=True));
s.append(dict(op='l2_setup',company='c',expected_revision=1,values=dict(receivables='ar2',payables='ap',undeposited='uf',collected_tax='tax',banks=['bank','bank2'])));inv2=invoice(s,66);s.append(cmd('review_application',record=m,values=application(67,inv2),review='cross-ar',expect_error=True));a,_=apply(s,68,m,inv,'1.00');save('reviews-original-ar',s)

s=setup();s.append(dict(op='l2_settings',company='c',values=dict(charge_tax=True,default_item_id=None,owed_basis='ACCRUAL',filing_frequency='MONTHLY',expected_revision=0)))
for n,rate in [(1,'3'),(2,'4')]:s.append(dict(op='l2_rate',company='c',ref='tax'+str(n),values=dict(name='Rate '+str(n),description='Rate '+str(n),rate=rate,new_agency_name='Agency '+str(n),expected_revision=0)))
s.append(dict(op='l2_group',company='c',ref='group',values=dict(name='Combined',description='Combined',member_ids=['tax1','tax2'],expected_revision=0)))
p=memo(70,tax_mode='ITEM',tax_item_id='group',tax_rate=None,tax_label='');p['lines'][0].update(quantity='1',rate='112.50');m=record(s,70,p)
s.append(dict(op='l2_rate',company='c',ref='tax1',values=dict(item_id='tax1',name='Rate 1',description='Rate 1',rate='5',agency_id='agency-for:tax1',expected_revision=1)));s.append(cmd('record_memo',ref=m,values=p,review=m));save('tax-group-retry',s)

s=setup();p=memo(80,memo='漢é🚀x'*500);m=record(s,80,p);s.append(undo('void_memo',m,reason='界'*500));save('maximum-text',s)

s=setup();m=record(s,803)
for n,value in enumerate(['1\n2','1\u200d2','\u2028','\u202e123','\ue000','\u1c89','123456789012345678901',False,None]):
 s.append(cmd('review_refund',record=m,values=refund(804+n,'0.01',check_number=value),review='bad-number'+str(n),expect_error=True))
for n,value in enumerate([' \t1001\n','\u00a01002\u00a0','\u20071003\u2007']):
 p=refund(820+n,'0.01',check_number=value);name='number'+str(n);s.extend([cmd('review_refund',record=m,values=p,review=name),cmd('refund',record=m,values=p,review=name,ref=name)])
save('number-text',s)

s=setup();p=memo(801);m=record(s,801,p);s.append(cmd('record_memo',values=p,review=m,ref='aaa-memo-retry'));rp=refund(802);s.extend([cmd('review_refund',record=m,values=rp,review='refund'),cmd('refund',record=m,values=rp,review='refund',ref='refund'),cmd('refund',record=m,values=rp,review='refund',ref='aaa-refund-retry'),undo('void_refund','refund'),undo('void_refund','aaa-refund-retry'),undo('void_memo',m),undo('void_memo','aaa-memo-retry')]);save('retry-aliases',s)

s=setup();p=memo(800);s.append(cmd('review_memo',values=p,review='catalog'));s.append(dict(op='l2_save',company='c',model='item',ref='work',record='work',values=dict(description='Catalog metadata changed',sale_rate='99.00')));s.append(cmd('record_memo',values=p,review='catalog',ref='catalog'));save('catalog-review-oddity',s)

s=setup();inv=invoice(s,81);m=record(s,82);a,p=apply(s,83,m,inv,'13.30')
v=dict(request_key=key(84),customer_id='buyer',date='2026-10-10',amount='21.61',method='CHECK',destination='BANK',bank_id='bank',allocations=[dict(invoice_id=inv,amount='21.61')])
s.append(dict(op='sales_review_receipt',company='c',values=copy.deepcopy(v),review='overpay',expect_error=True));v.update(request_key=key(85),amount='21.60',allocations=[dict(invoice_id=inv,amount='21.60')]);s.extend([dict(op='sales_review_receipt',company='c',values=v,review='pay'),dict(op='sales_record_receipt',company='c',values=v,review='pay',ref='pay')]);s.append(cmd('review_application',record=m,values=application(86,inv,'0.01'),review='overcredit',expect_error=True));s.extend([dict(op='sales_reverse_receipt',company='c',record='pay',expected_revision=1,date='2026-10-20',reason='Release payment'),undo('unapply',a),dict(op='sales_void_invoice',company='c',record=inv,expected_revision=2,date='2026-10-20',reason='Release invoice')]);save('receipt-settlement',s)

s=setup();one=invoice(s,87);two=invoice(s,88);m=record(s,89);p=application(890,one,allocations=[dict(invoice_id=one,amount='10.00'),dict(invoice_id=two,amount='3.30')]);s.extend([cmd('review_application',record=m,values=p,review='multi'),cmd('apply',record=m,values=p,review='multi',ref='multi'),cmd('apply',record=m,values=p,review='multi',ref='multi'),undo('unapply','multi'),cmd('apply',record=m,values=p,review='multi',ref='multi')]);save('multi-invoice',s)

# Each random command consumes one step. Maintain legal balances while mixing
# reviews, records, exact retries, unapplications, refunds and their voids.
s=setup();invs=[invoice(s,90),invoice(s,91)];rng=random.Random(6030500);records={};pending={};apps_live={};refunds_live={};counter=100;settled={i:0 for i in invs}
start=len(s)
for step in range(500):
 counter+=1;choice=rng.randrange(12)
 if (not records and not pending) or choice==0:
  p=memo(counter,tax_mode='NONE',tax_rate=None,tax_label='');p['lines'][0]['rate']=str(rng.randint(50,100));name='cm'+str(counter);s.append(cmd('review_memo',values=p,review=name));pending[name]=p
 elif (choice==1 or not records) and pending:
  name=rng.choice(list(pending));p=pending[name];s.append(cmd('record_memo',ref=name,values=p,review=name));records.setdefault(name,dict(used=0,void=False,total=int(p['lines'][0]['rate'])*100+250))
 elif choice==2 and records:
  name=rng.choice(list(records));s.append(cmd('record_memo',ref=name,values=pending[name],review=name))
 elif choice in (3,4) and any(not st['void'] and st['used']<st['total'] for st in records.values()):
  name=rng.choice([k for k,v in records.items() if not v['void'] and v['used']<v['total']]);inv=rng.choice(invs);p=application(counter,inv,'0.01');an='app'+str(counter)
  s.append(cmd('review_application',record=name,values=p,review=an));apps_live[an]=[name,p,'pending',records[name]['used'],settled[inv]]
 elif choice==5 and any(v[2]=='pending' for v in apps_live.values()):
  an=rng.choice([k for k,v in apps_live.items() if v[2]=='pending']);name,p,_,used,paid=apps_live[an];inv=p['allocations'][0]['invoice_id'];good=not records[name]['void'] and records[name]['used']==used and settled[inv]==paid;s.append(cmd('apply',record=name,values=p,review=an,ref=an,**({} if good else dict(expect_error=True))));apps_live[an][2]='live' if good else 'failed'
  if good:records[name]['used']+=1;settled[inv]+=1
 elif choice==6 and any(v[2]=='live' for v in apps_live.values()):
  an=rng.choice([k for k,v in apps_live.items() if v[2]=='live']);name,p,*_=apps_live[an];s.append(undo('unapply',an));apps_live[an][2]='released';records[name]['used']-=1;settled[p['allocations'][0]['invoice_id']]-=1
 elif choice==7 and any(not st['void'] and st['used']<st['total'] for st in records.values()):
  name=rng.choice([k for k,v in records.items() if not v['void'] and v['used']<v['total']]);p=refund(counter,'0.01',method=rng.choice(['CASH','OTHER']),check_number='');rn='r'+str(counter);s.append(cmd('review_refund',record=name,values=p,review=rn));refunds_live[rn]=[name,p,'pending',records[name]['used']]
 elif choice==8 and any(v[2]=='pending' for v in refunds_live.values()):
  rn=rng.choice([k for k,v in refunds_live.items() if v[2]=='pending']);name,p,_,used=refunds_live[rn];good=not records[name]['void'] and records[name]['used']==used;s.append(cmd('refund',record=name,values=p,review=rn,ref=rn,**({} if good else dict(expect_error=True))));refunds_live[rn][2]='live' if good else 'failed'
  if good:records[name]['used']+=1
 elif choice==9 and any(v[2]=='live' for v in refunds_live.values()):
  rn=rng.choice([k for k,v in refunds_live.items() if v[2]=='live']);name,p,*_=refunds_live[rn];s.append(undo('void_refund',rn));refunds_live[rn][2]='released';records[name]['used']-=1
 elif choice==10 and any(not v['void'] and v['used']==0 for v in records.values()):
  name=rng.choice([k for k,v in records.items() if not v['void'] and v['used']==0]);s.append(undo('void_memo',name));records[name]['void']=True
 else:
  name=rng.choice(list(records));p=refund(counter,'0');s.append(cmd('review_refund',record=name,values=p,review='bad'+str(counter),expect_error=True))
assert len(s)-start==500
save('random-500',s)
