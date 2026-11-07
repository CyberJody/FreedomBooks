"""Seeded live-service differential scenarios for the sales translation."""
import json,random,copy,uuid
from pathlib import Path
OUT=Path(__file__).resolve().parents[1]/'scenarios'
def key(n):return str(uuid.UUID(int=n))
def setup():
 s=[dict(op='create_company',ref='c',name='Synthetic sales')]
 for ref,code,kind in [('bank','100','ASSET'),('ar','110','ASSET'),('ar2','111','ASSET'),('uf','120','ASSET'),('tax','210','LIABILITY'),('income','400','INCOME'),('income2','410','INCOME'),('expense','500','EXPENSE')]:s.append(dict(op='save_account',company='c',ref=ref,code=code,name=ref,kind=kind))
 s.extend([dict(op='l2_save',company='c',model='customer',ref='buyer',values=dict(display_name='Buyer')),dict(op='l2_save',company='c',model='item',ref='work',values=dict(code='WORK',description='Synthetic work',kind='SERVICE',sale_rate='9.99',income_account='income',expense_account='expense')),dict(op='l2_setup',company='c',expected_revision=0,values=dict(receivables='ar',undeposited='uf',collected_tax='tax',banks=['bank']))]);return s

def payload(n,**changes):
 v=dict(request_key=key(n),customer_id='buyer',date='2026-10-01',memo='Synthetic invoice',tax_mode='MANUAL',tax_rate='6.25',tax_label='Combined tax',lines=[dict(item_id='work',description='Half cents',quantity='1.0025',rate='4.00',taxable=True,income_account_id='income')]);v.update(changes);return v

def cmd(op,**kw):return dict(op='sales_'+op,company='c',**kw)
def add_post(s,n,p=None):
 name='inv'+str(n);s.extend([cmd('save_invoice',ref=name,values=p or payload(n)),cmd('review_invoice',record=name,expected_revision=1,review='review'+str(n)),cmd('post_invoice',record=name,expected_revision=1,review='review'+str(n))]);return name

def pay(n,invoice,amount,**kw):
 v=dict(request_key=key(n),customer_id='buyer',date='2026-10-02',amount=amount,method='CHECK',destination='UNDEPOSITED',allocations=[dict(invoice_id=invoice,amount=amount)]);v.update(kw);return v

def save(name,s):
 data=dict(users=[dict(username='sales',is_staff=True),dict(username='outsider')],steps=s,as_of=['2026-09-30','2026-10-01','2026-10-02','2026-10-03']);(OUT/f'sales-{name}.json').write_text(json.dumps(data,ensure_ascii=False,indent=1)+'\n')

s=setup();name=add_post(s,1);s.append(cmd('post_invoice',record=name,expected_revision=1,review='review1'))
p=pay(20,name,'1.00');s.extend([cmd('review_receipt',values=p,review='pay1'),cmd('record_receipt',values=p,review='pay1',ref='payment1'),cmd('record_receipt',values=p,review='pay1',ref='payment1'),cmd('void_invoice',record=name,expected_revision=2,date='2026-10-03',reason='Correction',expect_error=True)])
q=pay(21,name,'3.26',destination='BANK',bank_id='bank');s.extend([cmd('review_receipt',values=q,review='pay2'),cmd('record_receipt',values=q,review='pay2',ref='payment2')])
for r in ['payment1','payment2']:s.append(cmd('reverse_receipt',record=r,expected_revision=1,date='2026-10-03',reason='Correction'))
s.extend([cmd('void_invoice',record=name,expected_revision=2,date='2026-10-03',reason='Correction'),cmd('void_invoice',record=name,expected_revision=2,date='2026-10-03',reason='Correction'),cmd('record_receipt',values=p,review='pay1',ref='payment1'),cmd('save_invoice',ref=name,values=payload(1)),cmd('review_invoice',record=name,expected_revision=3,review='no',expect_error=True)]);save('invoice-payments',s)

s=setup();p=payload(30,method='CASH',destination='UNDEPOSITED');s.extend([cmd('review_sale',values=p,review='sale'),cmd('record_sale',values=p,review='sale',ref='sale1'),cmd('record_sale',values=p,review='sale',ref='sale1'),cmd('reverse_receipt',record='sale1',expected_revision=1,date='2026-10-03',reason='Wrong route',expect_error=True),cmd('void_sale',record='sale1',expected_revision=1,date='2026-10-03',reason='Correction'),cmd('void_sale',record='sale1',expected_revision=1,date='2026-10-03',reason='Correction'),cmd('record_sale',values=p,review='sale',ref='sale1')]);
p=payload(31,method='CARD',destination='BANK',bank_id='bank',tax_mode='NONE',tax_rate=None,tax_label='');p['lines'] += [dict(item_id=None,description='Second income',quantity='2',rate='3',taxable=False,income_account_id='income2')];s.extend([cmd('review_sale',values=p,review='sale2'),cmd('record_sale',values=p,review='sale2',ref='sale2')]);save('cash-sales',s)

s=setup();p=payload(40);s.append(cmd('save_invoice',ref='draft',values=p));s.append(cmd('review_invoice',record='draft',expected_revision=1,review='stale'))
p2=payload(40,memo='Revised');s.append(cmd('save_invoice',ref='draft',record='draft',expected_revision=1,values=p2));s.append(cmd('post_invoice',record='draft',expected_revision=1,review='stale',expect_error=True));s.extend([cmd('review_invoice',record='draft',expected_revision=2,review='fresh'),dict(op='l2_setup',company='c',expected_revision=1,values=dict(receivables='ar2',undeposited='uf',collected_tax='tax',banks=['bank'])),cmd('post_invoice',record='draft',expected_revision=2,review='fresh',expect_error=True),cmd('review_invoice',record='draft',expected_revision=2,review='fresh2'),cmd('post_invoice',record='draft',expected_revision=2,review='fresh2')]);save('edits-reviews',s)

s=setup();name=add_post(s,50)
for i,v in enumerate([pay(60,name,'4.27'),pay(61,name,'4.26',date='2026-09-30'),pay(62,name,'0'),pay(63,name,'1',destination='BANK',bank_id='ar'),pay(64,name,'1',method='WIRE'),pay(65,name,'2',allocations=[dict(invoice_id=name,amount='1')])]):s.append(cmd('review_receipt',values=v,review='bad'+str(i),expect_error=True))
for i,changes in enumerate([dict(date='bad'),dict(due_date='2026-09-30'),dict(lines=[]),dict(tax_mode=''),dict(lines=[dict(item_id=None,description='x',quantity=True,rate='1',taxable=True,income_account_id='income')])]):s.append(cmd('save_invoice',ref='bad'+str(i),values=payload(70+i,**changes),expect_error=True))
s.append(cmd('review_invoice',record=name,expected_revision=2,review='denied',actor='outsider',expect_error=True));save('refusals',s)

s=setup();s.append(dict(op='l2_settings',company='c',values=dict(charge_tax=True,default_item_id=None,owed_basis='ACCRUAL',filing_frequency='QUARTERLY',expected_revision=0)))
for i,rate in [(1,'3'),(2,'4')]:s.append(dict(op='l2_rate',company='c',ref='tax'+str(i),values=dict(name='Rate '+str(i),description='Rate '+str(i),rate=rate,new_agency_name='Agency '+str(i),expected_revision=0)))
s.append(dict(op='l2_group',company='c',ref='group',values=dict(name='Combined',description='Combined',member_ids=['tax1','tax2'],expected_revision=0)))
p=payload(90,tax_mode='ITEM',tax_item_id='group',tax_rate=None,tax_label='',method='CASH',destination='UNDEPOSITED');p['lines'][0].update(quantity='1',rate='112.50')
s.extend([cmd('review_sale',values=p,review='group-sale'),cmd('record_sale',values=p,review='group-sale',ref='group-sale'),dict(op='l2_rate',company='c',ref='tax1',values=dict(item_id='tax1',name='Rate 1',description='Rate 1',rate='5',agency_id='agency-for:tax1',expected_revision=1)),cmd('record_sale',values=p,review='group-sale',ref='group-sale')]);save('tax-group-retry',s)

s=setup();rng=random.Random(310030);states={};reviews={};counter=1000
from decimal import Decimal, ROUND_HALF_UP
paid={};pending_sales={};recorded_sales={};pending_payments={};recorded_payments={}
def total_of(v):
    a=(Decimal(v['lines'][0]['quantity'])*Decimal(v['lines'][0]['rate'])).quantize(Decimal('.01'),rounding=ROUND_HALF_UP)
    return a+((a*Decimal('0.0625')).quantize(Decimal('.01'),rounding=ROUND_HALF_UP) if v['tax_mode']=='MANUAL' else Decimal(0))
for step in range(500):
 counter+=1;choice=rng.randrange(12)
 if not states or choice<=1:
  name='random'+str(counter);v=payload(counter,tax_mode=rng.choice(['NONE','MANUAL']),memo='Random '+str(counter))
  if v['tax_mode']=='NONE':v.update(tax_rate=None,tax_label='')
  v['lines'][0].update(quantity=str(rng.randint(1,9)),rate=str(rng.randint(1,200))+'.'+str(rng.randint(0,99)).zfill(2));s.append(cmd('save_invoice',ref=name,values=v));states[name]=[1,'DRAFT',v];paid[name]=Decimal(0)
 elif choice==6:
  v=payload(counter,method=rng.choice(['CASH','CARD','OTHER']),destination='BANK',bank_id='bank');v['lines'][0]['rate']=str(rng.randint(1,100));name='rs'+str(counter);s.append(cmd('review_sale',values=v,review=name));pending_sales[name]=v
 elif choice==7 and pending_sales:
  name=rng.choice(list(pending_sales));s.append(cmd('record_sale',values=pending_sales[name],review=name,ref=name));recorded_sales.setdefault(name,True)
 elif choice==8 and recorded_sales:
  name=rng.choice(list(recorded_sales));s.append(cmd('void_sale',record=name,expected_revision=1,date='2026-10-03',reason='Random sale reversal'));recorded_sales[name]=False
 elif choice==9 and any(v[1]=='OPEN' and total_of(v[2])-paid[k]>=Decimal('.01') for k,v in states.items()):
  invoice=rng.choice([k for k,v in states.items() if v[1]=='OPEN' and total_of(v[2])-paid[k]>=Decimal('.01')]);name='rp'+str(counter);v=pay(counter,invoice,'0.01');s.append(cmd('review_receipt',values=v,review=name));pending_payments[name]=(invoice,v)
 elif choice==10 and pending_payments:
  name=rng.choice(list(pending_payments));invoice,v=pending_payments[name];existing=name in recorded_payments;good=existing or (states[invoice][1]=='OPEN' and total_of(states[invoice][2])-paid[invoice]>=Decimal('.01'));s.append(cmd('record_receipt',values=v,review=name,ref=name,**({} if good else dict(expect_error=True))))
  if good and not existing:recorded_payments[name]=[invoice,True];paid[invoice]+=Decimal('.01')
 elif choice==11 and recorded_payments:
  name=rng.choice(list(recorded_payments));invoice,live=recorded_payments[name];s.append(cmd('reverse_receipt',record=name,expected_revision=1,date='2026-10-03',reason='Random payment reversal'))
  if live:paid[invoice]-=Decimal('.01');recorded_payments[name][1]=False
 else:
  name=rng.choice(list(states));revision,status,v=states[name]
  if choice==2 and status=='DRAFT':
   r='rr'+str(counter);s.append(cmd('review_invoice',record=name,expected_revision=revision,review=r));reviews[name]=(r,revision)
  elif choice==3 and status=='DRAFT' and name in reviews:
   r,seen=reviews[name];good=seen==revision;s.append(cmd('post_invoice',record=name,expected_revision=seen,review=r,**({}if good else dict(expect_error=True))))
   if good:states[name][0]+=1;states[name][1]='OPEN'
  elif choice==4 and status=='DRAFT':
   v=copy.deepcopy(v);v['memo']='Changed '+str(counter);s.append(cmd('save_invoice',ref=name,record=name,expected_revision=revision,values=v));states[name]=[revision+1,status,v]
  elif choice==5 and status=='OPEN':
   good=paid[name]==0;s.append(cmd('void_invoice',record=name,expected_revision=revision,date='2026-10-03',reason='Random reversal',**({} if good else dict(expect_error=True))))
   if good:states[name][0]+=1;states[name][1]='VOID'
  else:s.append(cmd('review_invoice',record=name,expected_revision=revision+10,review='bad'+str(counter),expect_error=True))
assert len(s)-len(setup())==500
save('random-500',s)

# A deliberate Python review oddity: unshown live catalog metadata is not bound.
s=setup();v=payload(3000,method='CASH',destination='UNDEPOSITED');s.extend([cmd('review_sale',values=v,review='metadata'),dict(op='l2_save',company='c',model='item',record='work',ref='work',values=dict(description='Changed while reviewing',sale_rate='50.00')),cmd('record_sale',values=v,review='metadata',ref='metadata')]);save('catalog-review-oddity',s)

s=setup();v=payload(3100,memo='界'*2000);name=add_post(s,3100,v);s.append(cmd('void_invoice',record=name,expected_revision=2,date='2026-10-03',reason='訂'*500));s.append(cmd('void_invoice',record=name,expected_revision=2,date='2026-10-03',reason='訂'*500));save('maximum-text',s)

s=setup();one=add_post(s,3200);s.append(dict(op='l2_setup',company='c',expected_revision=1,values=dict(receivables='ar2',undeposited='uf',collected_tax='tax',banks=['bank'])));two=add_post(s,3201);v=pay(3202,one,'2.00',allocations=[dict(invoice_id=one,amount='1.00'),dict(invoice_id=two,amount='1.00')]);s.extend([cmd('review_receipt',values=v,review='split'),cmd('record_receipt',values=v,review='split',ref='split'),dict(op='l2_setup',company='c',expected_revision=2,values=dict(receivables='ar2',undeposited=None,collected_tax='tax',banks=['bank']))]);save('original-ar-split',s)
print('Generated sales scenarios')
