"""Deterministic checks/transfers workflows and 500 random service commands."""
from pathlib import Path
from copy import deepcopy
import json,random,uuid
out=Path(__file__).resolve().parents[1]/'scenarios'
counter=300000
def key():
    global counter
    counter+=1
    return str(uuid.UUID(int=counter))
def setup():
    return [dict(op='create_company',ref='co',name='Synthetic checks'),
      *[dict(op='save_account',company='co',ref=r,code=c,name=n,kind=k) for r,c,n,k in [('bank','100','Bank','ASSET'),('bank2','101','Savings','ASSET'),('ap','200','Payables','LIABILITY'),('expense','500','Expense','EXPENSE'),('asset','150','Equipment','ASSET')]],
      dict(op='l2_save',company='co',model='vendor',ref='vendor',values=dict(display_name='Synthetic supplier',billing_address='Test address')),
      dict(op='l2_save',company='co',model='item',ref='item',values=dict(code='Parts',description='Parts',kind='NONSTOCK',sale_rate='2.00',purchase_cost='1.00',expense_account='expense')),
      dict(op='l2_setup',company='co',expected_revision=0,values=dict(payables='ap',banks=['bank','bank2']))]
def check(**kw):
    p=dict(bank_id='bank',payee_kind='VENDOR',vendor_id='vendor',date='2026-10-23',check_number='101',memo='Synthetic check',request_key=key(),lines=[dict(tab='EXPENSE',account_id='expense',description='Materials',rate='12.34')]);p.update(kw);return p
def transfer(**kw):
    p=dict(from_account_id='bank',to_account_id='bank2',date='2026-10-23',amount='13.45',memo='',request_key=key());p.update(kw);return p
def step(op,**kw):return dict(op=op,company='co',**kw)
def save(name,p,**kw):return step('checks_save',ref=name,values=deepcopy(p),**kw)
def write(name,steps):
    (out/(name+'.json')).write_text(json.dumps(dict(users=[dict(username='check-test',is_staff=True)],steps=steps),indent=2)+'\n')

s=setup();p=check(lines=[dict(tab='EXPENSE',account_id='expense',rate='12.34'),dict(tab='ITEM',item_id='item',account_id='expense',quantity='1.005',rate='1.00'),dict(tab='EXPENSE',account_id='asset',rate='150.00')])
s += [save('c',p),save('c',p),step('checks_review',check='c',expected_revision=1,review='r'),step('checks_record',check='c',expected_revision=1,review='r'),step('checks_record',check='c',expected_revision=1,review='r'),step('checks_void',check='c',expected_revision=2,date='2026-10-24',reason='Corrected'),step('checks_void',check='c',expected_revision=2,date='2026-10-24',reason='Corrected'),save('c',p),step('checks_review',check='c',expected_revision=3,review='bad',expect_error=True),step('checks_next',bank='bank',ref='next')]
write('checks-lifecycle',s)
s=setup();p=check(check_number='',print_later=True,payee_kind='OTHER',vendor_id=None,payee_name='Cash supplier')
s += [save('c',p),step('checks_record',check='c',expected_revision=1),step('checks_number',check='c',expected_revision=2,number='99999999999999999999'),step('checks_number',check='c',expected_revision=2,number='99999999999999999999'),step('checks_next',bank='bank',ref='overflow-next'),save('duplicate',check(check_number='99999999999999999999'),expect_error=True),step('checks_void',check='c',expected_revision=3,date='2026-10-24',reason='Release number'),save('replacement',check(check_number='99999999999999999999')),save('leading',check(check_number='0001')),step('checks_next',bank='bank',ref='fallback-next')]
write('checks-numbering',s)
s=setup();p=check();s+=[save('c',p),step('checks_review',check='c',expected_revision=1,review='old')];p.update(check_id='c',expected_revision=1,memo='Edited')
s += [save('c',p),step('checks_record',check='c',expected_revision=2,review='old',expect_error=True),step('checks_record',check='c',expected_revision=1,expect_error=True),step('checks_record',check='c',expected_revision=2)]
for kw in [dict(check_number='a\u200b'),dict(check_number='x'*21),dict(print_later=True),dict(bank_id='expense'),dict(payee_kind='OTHER',vendor_id=None,payee_name=''),dict(lines=[]),dict(lines=[dict(tab='EXPENSE',account_id='ap',rate='1.00')]),dict(lines=[dict(tab='EXPENSE',account_id='bank',rate='1.00')]),dict(lines=[dict(tab='ITEM',item_id='item',account_id='expense',quantity='1',rate='0')]),dict(date='2026-02-30')]:s.append(save('bad',check(check_number='202',**{k:v for k,v in kw.items() if k!='check_number'}) if 'check_number' not in kw else check(**kw),expect_error=True))
s += [save('draft',check(check_number='300',lines=[dict(tab='EXPENSE',rate='1')])),step('checks_review',check='draft',expected_revision=1,review='missing',expect_error=True)]
write('checks-edit-refusals',s)
s=setup();p=transfer();s+=[step('transfers_review',values=p,review='tr'),step('transfers_record',ref='t',values=p,review='tr'),step('transfers_record',ref='t',values=p,review='tr'),step('transfers_void',transfer='t',expected_revision=1,date='2026-10-24',reason='Corrected'),step('transfers_void',transfer='t',expected_revision=1,date='2026-10-24',reason='Corrected'),step('transfers_record',ref='t',values=p,review='tr')]
for kw in [dict(to_account_id='bank'),dict(from_account_id='expense'),dict(amount='0'),dict(amount='-1'),dict(date='2026-02-30')]:s.append(step('transfers_record',ref='bad',values=transfer(**kw),expect_error=True))
q=transfer();s += [step('transfers_review',values=q,review='old'),step('transfers_record',ref='bad',values={**q,'amount':'99.00'},review='old',expect_error=True)]
write('transfers-lifecycle',s)
s=setup();bill=dict(vendor_id='vendor',date='2026-10-23',due_date='2026-11-23',reference='Bill',memo='',request_key=key(),lines=[dict(description='Cost',quantity='1',rate='20',expense_account_id='expense')]);payment=dict(vendor_id='vendor',date='2026-10-23',amount='10',method='CHECK',reference='101',memo='',bank_id='bank',request_key=key(),allocations=[dict(bill_id='b',amount='10')]);p=check()
s += [step('payables_save',ref='b',values=bill),step('payables_post',bill='b',expected_revision=1),save('c',p),step('checks_pay',ref='bad',values=payment,expect_error=True),step('checks_record',check='c',expected_revision=1),step('checks_void',check='c',expected_revision=2,date='2026-10-24',reason='Release'),step('checks_review_payment',values=payment,review='pr'),step('checks_pay',ref='p',values=payment,review='pr'),step('checks_pay',ref='p',values=payment,review='pr'),save('bad',check(),expect_error=True),step('checks_next',bank='bank',ref='next'),step('payables_reverse',payment='p',expected_revision=1,date='2026-10-24',reason='Correction'),save('replacement',check())]
write('checks-payables-collisions',s)
r=random.Random(303049);s=setup();states={};transfers=[];serial=0
for i in range(500):
    choice=r.randrange(8)
    drafts=[n for n,v in states.items() if v[0]=='DRAFT'];posted=[n for n,v in states.items() if v[0]=='RECORDED']
    if choice==0 or not states:
        serial+=1;name=f'c{serial}';p=check(check_number=str(1000+serial),lines=[dict(tab='EXPENSE',account_id=r.choice(['expense','asset']),rate=f'{r.randint(1,1000)}.{r.randint(0,99):02}'),dict(tab='ITEM',item_id='item',account_id='expense',quantity=r.choice(['1.005','0.3333','2.5']),rate='1.50')]);s.append(save(name,p));states[name]=['DRAFT',1,p]
    elif choice==1 and drafts:
        n=r.choice(drafts);state,rev,p=states[n];s.append(step('checks_record',check=n,expected_revision=rev));states[n][0]='RECORDED';states[n][1]+=1
    elif choice==2 and posted:
        n=r.choice(posted);s.append(step('checks_void',check=n,expected_revision=states[n][1],date='2026-10-24',reason='Random correction'));states[n][0]='VOID';states[n][1]+=1
    elif choice==3 and drafts:
        n=r.choice(drafts);p=deepcopy(states[n][2]);p.update(check_id=n,expected_revision=states[n][1],memo=f'Edit {i}');s.append(save(n,p));states[n][1]+=1;states[n][2]=p
    elif choice==4:
        n=f't{i}';p=transfer(amount=f'{r.randint(1,999)}.{r.randint(0,99):02}',from_account_id=r.choice(['bank','bank2']));p['to_account_id']='bank2' if p['from_account_id']=='bank' else 'bank';s.append(step('transfers_record',ref=n,values=p));transfers.append(n)
    elif choice==5 and transfers:
        n=transfers.pop(r.randrange(len(transfers)));s.append(step('transfers_void',transfer=n,expected_revision=1,date='2026-10-24',reason='Random correction'))
    elif choice==6:
        s.append(save('bad',check(check_number=f'X{i}',lines=[dict(tab='EXPENSE',rate=r.choice(['0','-1']),account_id='expense')]),expect_error=True))
    else:s.append(step('checks_next',bank=r.choice(['bank','bank2']),ref=f'next{i}'))
assert len(s)-len(setup())==500
write('checks-random-500',s)
