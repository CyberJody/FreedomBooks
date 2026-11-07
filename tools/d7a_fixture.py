"""Append isolated D7a synthetic fixtures and screens; preserve every existing screen."""
from pathlib import Path
import json
from copy import deepcopy
ROOT=Path(__file__).resolve().parents[1]
scenario=json.loads((ROOT/'crates/fb-screendiff/scenarios/screens-dfix1.json').read_text())
steps=scenario['steps']
def add(op,**kw):steps.append(dict(op=op,company='d7',expect_error=False,**kw))
def key(n):return f'00000000-0000-0000-0000-000000007{n:03x}'
add('create_company',ref='d7',name='D7a synthetic books',currency='USD')
for ref,code,name,detail in [('bank','1000','Checking #typed','BANK'),('bank2','1010','Reserve','BANK'),('ar','1100','Accounts receivable','RECEIVABLE'),('uf','1200','Undeposited funds','OTHER_CURRENT_ASSET'),('ap','2000','Accounts payable','PAYABLE'),('income','4000','Sales','INCOME'),('expense','5000','Supplies','EXPENSE')]:
    add('save_account',ref='d7-'+ref,code=code,name=name,detail_type=detail)
add('l2_setup',expected_revision=0,values=dict(receivables='d7-ar',undeposited='d7-uf',payables='d7-ap',banks=['d7-bank','d7-bank2']))
add('l2_save',model='vendor',ref='d7-vendor',values=dict(display_name='Willow & Stone Supply',billing_address='17 Fiction Lane\nSampletown',default_expense_account='d7-expense'))
add('l2_save',model='customer',ref='d7-customer',values=dict(display_name='Cedar Café #typed'))
add('l2_save',model='item',ref='d7-item',values=dict(code='PART #typed',description='Synthetic parts',kind='NONSTOCK',sale_rate='5.00',purchase_cost='2.50',income_account='d7-income',expense_account='d7-expense'))
def chk(n,**kw):
    v=dict(request_key=key(n),bank_id='d7-bank',payee_kind='VENDOR',vendor_id='d7-vendor',date='2026-10-15',check_number=str(100+n),address='17 Fiction Lane\nSampletown',memo=f'Check {n} synthetic #123',lines=[dict(tab='EXPENSE',account_id='d7-expense',description='Synthetic supplies',quantity='1',rate='10.25'),dict(tab='ITEM',item_id='d7-item',account_id='d7-expense',description='Parts & service',quantity='2',rate='2.50')])
    v.update(kw);return v
add('checks_save',ref='d7-check1',values=chk(1))
add('checks_save',ref='d7-check2',values=chk(2))
revised=chk(1,check_id='d7-check1',expected_revision=1,memo='Revised after the second check')
for i,line in enumerate(revised['lines'],1):line['origin_position']=i
add('checks_save',ref='d7-check1',values=revised)
add('checks_save',ref='d7-check3',values=chk(3,print_later=True,check_number=''))
add('checks_record',check='d7-check3',expected_revision=1)
add('checks_save',ref='d7-check4',values=chk(4,payee_kind='OTHER',vendor_id=None,payee_name='Other synthetic payee'))
add('checks_record',check='d7-check4',expected_revision=1)
add('checks_void',check='d7-check4',expected_revision=2,date='2026-10-20',reason='Synthetic correction\nFull reason retained')
# A posted bill ensures the check warns that writing a check does not settle bills.
add('payables_save',ref='d7-bill',values=dict(request_key=key(8),vendor_id='d7-vendor',date='2026-10-01',due_date='2026-10-31',lines=[dict(description='Open bill',quantity='1',rate='20.00',expense_account_id='d7-expense')]))
add('payables_post',bill='d7-bill',expected_revision=1)
add('sales_save_invoice',ref='d7-invoice',values=dict(request_key=key(9),customer_id='d7-customer',date='2026-10-01',due_date='2026-10-31',tax_mode='NONE',lines=[dict(description='Synthetic sale',quantity='1',rate='100.00',income_account_id='d7-income',taxable=False)]))
add('sales_review_invoice',record='d7-invoice',expected_revision=1,review='d7-invoice')
add('sales_post_invoice',record='d7-invoice',expected_revision=1,review='d7-invoice')
for n in range(1,5):
    receipt=dict(request_key=key(16+n),customer_id='d7-customer',date='2026-10-05',amount=f'{n*10}.25',method='CHECK' if n%2 else 'CASH',reference=f'REF-{n}',memo='Synthetic receipt',destination='UNDEPOSITED',allocations=[dict(invoice_id='d7-invoice',amount=f'{n*10}.25')])
    if n<4:
        add('sales_review_receipt',review=f'd7-rcp{n}',values=receipt)
        add('sales_record_receipt',review=f'd7-rcp{n}',ref=f'd7-rcp{n}',values=receipt)
    else:
        receipt.pop('amount');receipt.pop('allocations');receipt.update(tax_mode='NONE',lines=[dict(item_id=None,description='Cash sale',quantity='1',rate='40.25',income_account_id='d7-income',taxable=False)])
        add('sales_review_sale',review='d7-sale',values=receipt)
        add('sales_record_sale',review='d7-sale',ref='d7-sale',values=receipt)
dep=dict(request_key=key(32),bank_id='d7-bank',date='2026-10-10',reference='Slip #1',memo='Synthetic deposit\nFull memo',receipt_ids=['d7-rcp1','d7-rcp2'])
add('banking_deposit',ref='d7-dep1',values=dep)
add('banking_reverse',deposit='d7-dep1',expected_revision=1,date='2026-10-12',reason='Synthetic deposit correction\nFull reason')
dep2=deepcopy(dep);dep2.update(request_key=key(33),date='2026-10-15')
add('banking_deposit',ref='d7-dep2',values=dep2)
scenario['name']='screens-d7a'
(ROOT/'crates/fb-screendiff/scenarios/screens-d7a.json').write_text(json.dumps(scenario,indent=2)+'\n')
screens=json.loads((ROOT/'crates/fb-screendiff/screens.json').read_text())
screens={k:v for k,v in screens.items() if not k.startswith('d7a-')}
base='/c/{company:D7a synthetic books}'
def screen(name,path,post=None,status=None):
    assert 'd7a-'+name not in screens,name
    v=dict(path=base+path)
    if post is not None:v['post']=post
    if status is not None:v['status']=status
    screens['d7a-'+name]=v
check='{check:D7a synthetic books:CHK-000001}'
recorded='{check:D7a synthetic books:CHK-000003}'
voided='{check:D7a synthetic books:CHK-000004}'
dep='{deposit:D7a synthetic books:DEP-000002}'
for name,path in [('checks-list','/checks/'),('checks-filter','/checks/?state=DRAFT&vendor={vendor:D7a synthetic books:Willow & Stone Supply}'),('check-new','/checks/new/'),('check-edit',f'/checks/{check}/edit/'),('check-detail',f'/checks/{check}/'),('check-recorded',f'/checks/{recorded}/'),('check-voided',f'/checks/{voided}/'),('check-revision',f'/checks/{check}/revisions/1/'),('check-number',f'/checks/{recorded}/number/'),('check-review',f'/checks/{check}/review/?revision=2'),('check-void',f'/checks/{recorded}/void/'),('vendor-activity','/lists/vendors/{vendor:D7a synthetic books:Willow & Stone Supply}/'),('deposits-list','/deposits/'),('deposits-filter','/deposits/?state=LIVE'),('deposit-new','/deposits/new/'),('deposit-detail',f'/deposits/{dep}/'),('deposit-reversed','/deposits/{deposit:D7a synthetic books:DEP-000001}/'),('deposit-reverse',f'/deposits/{dep}/reverse/')]:screen(name,path)
for name,path in [('checks-invalid-filter','/checks/?state=%20DRAFT%20&print_later=bad&start=bad'),('deposits-invalid-filter','/deposits/?state=%20LIVE%20'),('check-invalid-review',f'/checks/{check}/review/?revision=2.0')]:screen(name,path,status=400)
bad=dict(request_key=key(100),expected_revision='0',bank='{account:D7a synthetic books:1000}',date='bad',payee_kind='VENDOR',vendor='',check_number='',**{'lines-TOTAL_FORMS':'2','lines-INITIAL_FORMS':'0','lines-0-tab':'EXPENSE','lines-0-quantity':'1','lines-0-rate':'bad','lines-0-description':'Typed <value>','lines-1-tab':'ITEM','lines-1-quantity':'1'})
screen('check-new-invalid','/checks/new/',bad,400)
screen('check-edit-invalid',f'/checks/{check}/edit/',bad,400)
blank=deepcopy(bad);blank.update(date='2026-10-15',vendor='{vendor:D7a synthetic books:Willow & Stone Supply}',check_number='999');blank.update({'lines-0-rate':'2.00','lines-0-account':'{account:D7a synthetic books:5000}','lines-1-description':' \t\n '})
blank['date']='bad';screen('check-whitespace-extra','/checks/new/',blank,400)
mgmt=deepcopy(blank);mgmt['lines-INITIAL_FORMS']='bad';screen('check-management-invalid','/checks/new/',mgmt,400)
choice_bad=deepcopy(blank);choice_bad.update(payee_kind=' OTHER ',vendor=' bad ',expected_revision='0.0');screen('check-padded-choice','/checks/new/',choice_bad,400)
for name,path,post in [('check-record-invalid',f'/checks/{check}/record/',dict(expected_revision='bad',review_token='')),('check-number-invalid',f'/checks/{recorded}/number/',dict(expected_revision='0',check_number='')),('check-void-invalid',f'/checks/{recorded}/void/',dict(expected_revision='1.0',date='bad',reason='')),('deposit-new-invalid','/deposits/new/',dict(request_key='bad',bank='',date='bad',reference='typed <note>')),('deposit-reverse-invalid',f'/deposits/{dep}/reverse/',dict(expected_revision='0',date='bad',reason=''))]:screen(name,path,post,400)
review=dict(request_key=key(101),bank='{account:D7a synthetic books:1000}',date='2026-10-15',reference='New slip',memo='New deposit review',receipts='{receipt:D7a synthetic books:RCP-000003}',action='review')
screen('deposit-review','/deposits/new/',review)
review_bad=deepcopy(review);review_bad['receipts']='not-a-receipt';screen('deposit-choice-invalid','/deposits/new/',review_bad,400)
model_padding=deepcopy(blank)
for field in ['bank','vendor','lines-0-account']:model_padding[field]=' \t'+model_padding[field]+' \n'
screen('check-modelchoice-padding','/checks/new/',model_padding,400)
deposit_padding=deepcopy(review);deposit_padding['bank']=' \t'+deposit_padding['bank']+' \n';deposit_padding['date']='bad'
screen('deposit-modelchoice-padding','/deposits/new/',deposit_padding,400)
screens['d7a-check-no-banks']={'path':'/c/{company:Other synthetic books}/checks/new/'}
screens['d7a-deposit-no-receipts']={'path':'/c/{company:Other synthetic books}/deposits/new/'}
(ROOT/'crates/fb-screendiff/screens.json').write_text(json.dumps(screens,indent=2)+'\n')
print(f'D7a: {len(steps)} total fixture steps; {len(screens)} total screens')
