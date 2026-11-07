"""Deterministic payables workflows; frozen services are the sole accounting oracle."""
import json,random
from pathlib import Path
from uuid import UUID
from copy import deepcopy
from typing import Any
OUT=Path(__file__).resolve().parents[1]/'scenarios'
serial=100000
def key():
 global serial
 serial+=1;return str(UUID(int=serial))
def setup():
 return [{'op':'create_company','ref':'co','name':'Synthetic bills'},*[{'op':'save_account','company':'co','ref':r,'code':code,'name':name,'kind':kind} for r,code,name,kind in [('bank','100','Bank','ASSET'),('ap','200','Payables','LIABILITY'),('ap2','201','Other payable','LIABILITY'),('expense','500','Expense','EXPENSE'),('asset','150','Asset','ASSET')]],{'op':'l2_save','company':'co','ref':'vendor','model':'vendor','values':{'display_name':'Synthetic supplier','billing_address':'Test address'}},{'op':'l2_setup','company':'co','expected_revision':0,'values':{'payables':'ap','banks':['bank']}}]
def payload(ref='',amount='1.00') -> dict[str, Any]:
 return dict(vendor_id='vendor',date='2026-10-23',due_date='2026-11-23',reference=ref,memo='',request_key=key(),lines=[dict(description='Gross cost including supplier tax',quantity='1.005',rate=amount,expense_account_id='expense')])
def step(op,**kw):return dict(op='payables_'+op,company='co',**deepcopy(kw))
def post(b,revision=1,**kw):return step('post',bill=b,expected_revision=revision,**kw)
def void(b,revision=2,**kw):return step('void',bill=b,expected_revision=revision,date='2026-10-24',reason='Test correction',**kw)
def payment(b,amount,reference='100') -> dict[str, Any]:
 return dict(vendor_id='vendor',date='2026-10-24',amount=amount,method='CHECK',reference=reference,memo='',bank_id='bank',request_key=key(),allocations=[dict(bill_id=b,amount=amount)])
def save(name,steps):
 (OUT/('payables-'+name+'.json')).write_text(json.dumps(dict(users=[dict(username='bill-test',is_staff=True)],steps=steps,as_of=['2026-10-23','2026-10-24','2099-12-31']),indent=2)+'\n')
s=setup();p=payload('Supplier 42');s += [step('save',ref='b',values=p),step('review',bill='b',expected_revision=1,review='r'),post('b',review='r'),post('b',review='r'),void('b'),void('b'),step('save',ref='b',values=p)]
s += [step('save',ref='bad',values=payload('supplier 42'),expect_error=True)]
p2=payload('supplier 42');p2.update(supersedes_id='b',reentry_reason='Corrected amount');s += [step('save',ref='b2',values=p2),post('b2'),void('b2')];p3=payload('Supplier 42');p3.update(supersedes_id='b',reentry_reason='Stale predecessor');s += [step('save',ref='bad',values=deepcopy(p3),expect_error=True)];p3['supersedes_id']='b2';s += [step('save',ref='b3',values=p3)];save('lifecycle',s)
s=setup();p=payload('Edit');s+=[step('save',ref='b',values=p)];edit=deepcopy(p);edit.update(bill_id='b',expected_revision=1,memo='Revised');s+=[step('save',ref='b',values=edit),step('save',ref='b',values=edit,expect_error=True),step('review',bill='b',expected_revision=2,review='r')];s+=[{'op':'l2_setup','company':'co','expected_revision':1,'values':{'payables':'ap2','banks':['bank']}},post('b',2,review='r',expect_error=True),post('b',2),void('b',3)]
for q,r in [('0','1'),('1.00001','1'),('1','1.001'),('1','-1'),('NaN','1'),('99999999999999','99999999999999')]:
 p=payload();p['lines'][0].update(quantity=q,rate=r);s.append(step('save',ref='bad',values=p,expect_error=True))
p=payload('zero','0');s+=[step('save',ref='zero',values=p),post('zero',expect_error=True)];save('edit-refusals',s)
s=setup();p=payload('Paid');s += [step('save',ref='b',values=p),post('b')];p1=payment('b','0.50');s += [step('review_payment',review='p1r',values=p1),step('pay',ref='p1',review='p1r',values=p1),void('b',expect_error=True)];p2=payment('b','0.51','101');s += [step('pay',ref='p2',values=p2),step('pay',ref='bad',values=payment('b','0.01','102'),expect_error=True)]
for b in ['p1','p2']:s += [step('reverse',payment=b,expected_revision=1,date='2026-10-24',reason='Correction'),step('reverse',payment=b,expected_revision=1,date='2026-10-24',reason='Correction')]
s += [void('b'),{'op':'l2_active','company':'co','model':'vendor','record':'vendor','active':False},step('pay',ref='p1',review='p1r',values=p1),step('save',ref='b',values=p)];save('payments',s)
s=setup();rng=random.Random(20261030);bills={};payments={};used=0
# Exactly 500 commands after setup, including lifecycle success and refusals.
for i in range(500):
 choice=rng.randrange(7)
 if not bills or choice==0:
  name=f'b{i}';p=payload('',str(rng.randrange(1,100)));p['lines'][0]['quantity']='1';s.append(step('save',ref=name,values=p));bills[name]=dict(state='DRAFT',revision=1,payload=p,paid=False)
 else:
  name=rng.choice(list(bills));b=bills[name]
  if choice==1 and b['state']=='DRAFT':
   p=deepcopy(b['payload']);p.update(bill_id=name,expected_revision=b['revision'],memo=f'Edit {i}');s.append(step('save',ref=name,values=p));b['revision']+=1
  elif choice==2:
   s.append(post(name,b['revision'],expect_error=b['state']!='DRAFT'))
   if b['state']=='DRAFT':b['state']='OPEN';b['revision']+=1
  elif choice==3 and b['state']=='OPEN' and not b['paid']:
   used+=1;pn=f'p{i}';p=payment(name,b['payload']['lines'][0]['rate'],str(1000+used));s.append(step('pay',ref=pn,values=p));b['paid']=True;payments[pn]=name
  elif choice==4 and payments:
   pn=rng.choice(list(payments));s.append(step('reverse',payment=pn,expected_revision=1,date='2026-10-24',reason='Correction'));bills[payments.pop(pn)]['paid']=False
  elif choice==5:
   refused=b['state']!='OPEN' or b['paid'];s.append(void(name,b['revision'],expect_error=refused))
   if not refused:b['state']='VOID';b['revision']+=1
  else:
   p=deepcopy(b['payload']);p.update(bill_id=name,expected_revision=0);s.append(step('save',ref=name,values=p,expect_error=True))
save('random-500',s)
print('wrote payables lifecycle, edit-refusals, payments, random-500')
