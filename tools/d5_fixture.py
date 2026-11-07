from pathlib import Path
import json
import copy
import uuid
root=Path(__file__).resolve().parents[1]
base=root/'crates/fb-screendiff/scenarios'
scenario=json.loads((base/'screens-dfix1.json').read_text())
steps=scenario['steps']
serial=0
def key():
    global serial
    serial+=1
    return str(uuid.UUID(int=0xD5000000000040008000000000000000+serial))
def add(op,**kw):steps.append(dict(op=op,company='c',expect_error=False,**kw))
def workflow(prefix,review,record,ref,values,source=None):
    common=dict(values=values,review=ref+'-review')
    if source:common['deposit' if prefix=='pp_' else 'record']=source
    add(prefix+review,**common)
    add(prefix+record,ref=ref,**common)
add('save_account',ref='d5-liability',code='2300',name='Customer deposits',detail_type='OTHER_CURRENT_LIABILITY')
setups=[s for s in steps if s['op']=='l2_setup' and s.get('company')=='c']
values=copy.deepcopy(setups[-1]['values']);values['customer_deposits']='d5-liability'
add('l2_setup',expected_revision=len(setups),values=values)
# Two same-date invoices; revise the earlier one after creating the later one.
for n in [1,2]:
    v=dict(request_key=key(),customer_id='buyer',date='2026-10-24',due_date='2026-11-24',tax_mode='NONE',memo='D5 ordering '+str(n),lines=[dict(item_id=None,description='D5 open invoice '+str(n),quantity='1',rate='100.00',taxable=False,income_account_id='income')])
    add('sales_save_invoice',ref='d5-inv'+str(n),values=v)
v=dict(request_key=key(),customer_id='buyer',date='2026-10-24',due_date='2026-11-24',tax_mode='NONE',memo='D5 revised first',lines=[dict(item_id=None,description='D5 revised first',quantity='1',rate='100.00',taxable=False,income_account_id='income')])
add('sales_save_invoice',record='d5-inv1',ref='d5-inv1',values=v,expected_revision=1)
for n in [1,2]:
    add('sales_review_invoice',record='d5-inv'+str(n),review='d5-inv-review'+str(n),expected_revision=3-n)
    add('sales_post_invoice',record='d5-inv'+str(n),review='d5-inv-review'+str(n),expected_revision=3-n)
for n in [1,2,3]:
    v=dict(request_key=key(),customer_id='buyer',date='2026-10-24',memo='D5 credit '+str(n),tax_mode='NONE',lines=[dict(item_id=None,description='Returned service',quantity='2',rate='25.00',taxable=False,income_account_id='income')])
    workflow('credits_','review_memo','record_memo','d5-cm'+str(n),v)
for n in [1,2]:
    workflow('credits_','review_application','apply','d5-ca'+str(n),dict(request_key=key(),date='2026-10-26' if n==1 else '2026-10-25',allocations=[dict(invoice_id='d5-inv1',amount='5.00'),dict(invoice_id='d5-inv2',amount='7.00')]),'d5-cm1')
add('credits_unapply',record='d5-ca1',expected_revision=1,date='2026-10-27',reason='D5 release credit')
for n in [1,2]:
    workflow('credits_','review_refund','refund','d5-cr'+str(n),dict(request_key=key(),date='2026-10-25',amount='2.00',bank_id='bank',method='CHECK',check_number='D5C'+str(n),memo='Synthetic credit refund'),'d5-cm1')
add('credits_void_refund',record='d5-cr1',expected_revision=1,date='2026-10-26',reason='D5 reverse refund')
add('credits_void_memo',record='d5-cm3',expected_revision=1,date='2026-10-26',reason='D5 reverse credit')
for n in [1,2,3]:
    workflow('pp_','review_deposit','record_deposit','d5-cdp'+str(n),dict(request_key=key(),customer_id='buyer',date='2026-10-24',amount='50.00',method='CHECK',destination='UNDEPOSITED',reference='D5 check '+str(n),order_reference='D5 order',memo='Synthetic customer deposit'))
for n in [1,2]:
    workflow('pp_','review_application','apply','d5-da'+str(n),dict(request_key=key(),date='2026-10-26' if n==1 else '2026-10-25',amount='5.00',invoice_id='d5-inv1'),'d5-cdp1')
add('pp_unapply',record='d5-da1',expected_revision=1,date='2026-10-27',reason='D5 release deposit')
for n in [1,2]:
    workflow('pp_','review_refund','record_refund','d5-dr'+str(n),dict(request_key=key(),date='2026-10-25',amount='2.00',bank_id='bank',method='CHECK',check_number='D5D'+str(n),memo='Synthetic deposit refund'),'d5-cdp1')
add('pp_void_refund',record='d5-dr1',expected_revision=1,date='2026-10-26',reason='D5 reverse deposit refund')
add('pp_void_deposit',record='d5-cdp3',expected_revision=1,date='2026-10-26',reason='D5 reverse deposit')
# Fully used credits/deposits, cash and unnumbered checks exercise money truthiness and labels.
for prefix,source in [('credits_','d5-cm2'),('pp_','d5-cdp2')]:
    for n,method,amount in [(3,'CASH','40.00'),(4,'CHECK','10.00')]:
        workflow(prefix,'review_refund','record_refund' if prefix=='pp_' else 'refund','d5-extra-'+prefix+str(n),dict(request_key=key(),date='2026-10-27',amount=amount,bank_id='bank',method=method,check_number='',memo='D5 fully use source'),source)
(base/'screens-d5.json').write_text(json.dumps(scenario,indent=2)+'\n')
screens_path=root/'crates/fb-screendiff/screens.json'
screens=json.loads(screens_path.read_text())
screens={k:v for k,v in screens.items() if not k.startswith('d5-')}
def screen(name,path,post=None,status=200):
    spec=dict(path='/c/{company}/'+path)
    if post is not None:spec['post']=post
    if status!=200:spec['status']=status
    screens['d5-'+name]=spec
for owner,url,src,application,refund in [('credit','credit-memos','creditmemo:CM-000001','creditapplication:CM-000001:2','creditrefund:RFD-000002'),('deposit','customer-deposits','receipt:CDP-000001','depositapplication:CDA-000002','depositrefund:CDR-000002')]:
    prefix=url+'/{'+src+'}/'
    screen(owner+'-list',url+'/')
    screen(owner+'-new',url+'/new/')
    screen(owner+'-new-invalid',url+'/new/',{},400)
    screen(owner+'-detail',prefix)
    screen(owner+'-apply',prefix+'apply/')
    screen(owner+'-apply-invalid',prefix+'apply/',{},400)
    screen(owner+'-void',prefix+'void/')
    screen(owner+'-void-invalid',prefix+'void/',{},400)
    ap=('credit-applications' if owner=='credit' else 'deposit-applications')+'/{'+application+'}/unapply/'
    screen(owner+'-unapply',ap)
    screen(owner+'-unapply-invalid',ap,{},400)
    screen(owner+'-refund',prefix+'refund/')
    screen(owner+'-refund-invalid',prefix+'refund/',{},400)
    ref=('refunds' if owner=='credit' else 'deposit-refunds')+'/{'+refund+'}/'
    screen(owner+'-refund-detail',ref)
    screen(owner+'-refund-void',ref+'void/')
    screen(owner+'-refund-void-invalid',ref+'void/',{},400)
    screen(owner+'-voided',url+'/{'+('creditmemo:CM-000003' if owner=='credit' else 'receipt:CDP-000003')+'}/')
screen('invoice-deposit-apply','invoices/{invoice:INV-000005}/apply-deposit/')
screen('invoice-deposit-apply-invalid','invoices/{invoice:INV-000005}/apply-deposit/',{},400)
screen('invoice-fragments','invoices/{invoice:INV-000005}/')
screen('customer-fragments','lists/customers/{customer:Maple & Birch Café}/')
# Exercise line formset has_changed, field-specific stripping, and both review pages.
header=dict(request_key='d5000000-0000-4000-8000-000000000099',customer='{customer:Maple & Birch Café}',date='2026-10-27',memo='  D5 typed  ',tax_mode='NONE',**{'lines-TOTAL_FORMS':'3','lines-INITIAL_FORMS':'0','lines-MIN_NUM_FORMS':'1','lines-MAX_NUM_FORMS':'50','lines-0-description':'D5 return','lines-0-quantity':'1','lines-0-rate':'2.00','lines-0-income_account':'{account:4000}','lines-1-description':' \t ','lines-1-quantity':'  ','lines-1-rate':'\n','lines-1-origin_position':'ignored','lines-1-refresh_item':'true','lines-1-keep_archived':'true'})
screen('credit-review-whitespace-extra','credit-memos/new/',header)
screen('credit-padded-choice','credit-memos/new/',dict(header,tax_mode=' NONE '),400)
screen('credit-bad-management','credit-memos/new/',dict(header,**{'lines-TOTAL_FORMS':'0.0'}),400)
screen('credit-application-review','credit-memos/{creditmemo:CM-000001}/apply/',dict(request_key='d5000000-0000-4000-8000-000000000098',date='2026-10-27',**{'amount_{invoice:INV-000005}':'1.00'}))
screen('deposit-review','customer-deposits/new/',dict(request_key='d5000000-0000-4000-8000-000000000097',customer='{customer:Maple & Birch Café}',date='2026-10-27',amount='10.00',method='CHECK',destination='UNDEPOSITED',reference='D5',memo='  typed memo  '))
for owner,path in [('credit','credit-memos/{creditmemo:CM-000001}/'),('deposit','customer-deposits/{receipt:CDP-000001}/')]:
    screen(owner+'-refund-review',path+'refund/',dict(request_key='d5000000-0000-4000-8000-000000000096',date='2026-10-27',amount='1.00',bank='{account:1000}',method='CASH',memo='Synthetic refund'))
screen('deposit-application-review','customer-deposits/{receipt:CDP-000001}/apply/',dict(request_key='d5000000-0000-4000-8000-000000000095',invoice='{invoice:INV-000005}',date='2026-10-27',amount='1.00'))
for owner,path,source,refund in [('credit','credit-memos','creditmemo:CM-000002','creditrefund:RFD-'),('deposit','customer-deposits','receipt:CDP-000002','depositrefund:CDR-')]:
    screen(owner+'-fully-used',path+'/{'+source+'}/')
    screen(owner+'-unused-void',path+'/{'+source+'}/void/')
    for n,label in [(3,'cash'),(4,'unnumbered')]:
        screen(owner+'-refund-'+label,('refunds' if owner=='credit' else 'deposit-refunds')+'/{'+refund+str(n).zfill(6)+'}/')
screen('deposit-filter-order','customer-deposits/?q=D5+order')
screen('deposit-filter-memo','customer-deposits/?q=Synthetic+customer+deposit')
screen('credit-filter-invalid','credit-memos/?state=+AVAILABLE+',status=400)
screen('deposit-filter-invalid','customer-deposits/?state=+AVAILABLE+',status=400)
screen('deposit-with-estimate','customer-deposits/new/?customer={customer:Maple & Birch Café}&estimate={estimate:EST-000001}')
screen('deposit-padded-models','customer-deposits/new/',dict(request_key=' d5000000-0000-4000-8000-000000000094 ',customer=' {customer:Maple & Birch Café} ',date='2026-10-27',amount='10.00',method='CHECK',destination='BANK',bank=' {account:1000} '))
screen('credit-padded-models','credit-memos/new/',dict(header,customer=' {customer:Maple & Birch Café} ',**{'lines-0-income_account':' {account:4000} '}))
screen('credit-deleted-invalid-row','credit-memos/new/',dict(header,**{'lines-1-DELETE':'true','lines-1-quantity':'bad','lines-1-description':'','lines-1-income_account':'','lines-1-tax_auto':'99'}))
# COMMON 10: preserve the failing existing-service case instead of normalizing it away.
screen('deposit-estimate-core-gap','customer-deposits/new/',dict(request_key='d5000000-0000-4000-8000-000000000093',customer='{customer:Maple & Birch Café}',date='2026-10-27',amount='10.00',method='CHECK',destination='BANK',bank='{account:1000}',estimate='{estimate:EST-000001}'))
screens_path.write_text(json.dumps(screens,indent=2)+'\n')
print(len(steps),'scenario steps;',len(screens),'screens')
