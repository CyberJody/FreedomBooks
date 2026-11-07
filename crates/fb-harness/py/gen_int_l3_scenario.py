"""One company, every integrated document family, full checkpoints after each step."""
from pathlib import Path
import json
import copy
import os
from uuid import UUID

steps=[]
def add(op, **kw):
    row=dict(op=op,company='c',**kw);row.setdefault('expect_error',False)
    if row['expect_error']:row.setdefault('expect_kind','ValidationError')
    steps.append(row);return row
def key(n):return str(UUID(int=930000+n))
def pair(review,record,name,values,**kw):
    add(review,review=name,values=values,**kw)
    add(record,review=name,ref=name,values=values,**kw)

add('create_company',ref='c',name='Synthetic integrated layer 3')
for ref,code,kind,detail in [('bank','100','ASSET','BANK'),('bank2','101','ASSET','BANK'),('ar','110','ASSET','RECEIVABLE'),('uf','120','ASSET','OTHER_CURRENT_ASSET'),('ap','200','LIABILITY','PAYABLE'),('liability','230','LIABILITY','OTHER_CURRENT_LIABILITY'),('income','400','INCOME','INCOME'),('expense','500','EXPENSE','EXPENSE')]:
    add('save_account',ref=ref,code=code,name=ref,kind=kind,detail_type=detail)
add('l2_save',ref='buyer',model='customer',values=dict(display_name='Buyer'))
add('l2_save',ref='vendor',model='vendor',values=dict(display_name='Supplier'))
add('l2_setup',expected_revision=0,values=dict(receivables='ar',payables='ap',undeposited='uf',customer_deposits='liability',banks=['bank','bank2']))
add('sales_save_invoice',ref='invoice',values=dict(request_key=key(1),customer_id='buyer',date='2026-10-01',tax_mode='NONE',lines=[dict(description='Work',quantity='1',rate='1200.00',taxable=False,income_account_id='income')]))
add('sales_review_invoice',record='invoice',review='invoice',expected_revision=1)
add('sales_post_invoice',record='invoice',review='invoice',expected_revision=1)
pair('pp_review_deposit','pp_record_deposit','deposit',dict(request_key=key(2),customer_id='buyer',date='2026-10-02',amount='500.00',method='CASH',destination='BANK',bank_id='bank'))
pair('pp_review_application','pp_apply','application',dict(request_key=key(3),date='2026-10-03',invoice_id='invoice',amount='300.00'),deposit='deposit')
pair('sales_review_receipt','sales_record_receipt','payment',dict(request_key=key(4),customer_id='buyer',date='2026-10-04',amount='900.00',method='CASH',destination='BANK',bank_id='bank',allocations=[dict(invoice_id='invoice',amount='900.00')]))
add('payables_save',ref='bill',values=dict(request_key=key(5),vendor_id='vendor',date='2026-10-05',due_date='2026-11-05',reference='Supplier bill',lines=[dict(description='Cost',quantity='1',rate='60.00',expense_account_id='expense')]))
add('payables_post',bill='bill',expected_revision=1)
billpay=dict(request_key=key(6),vendor_id='vendor',date='2026-10-06',amount='50.00',method='CHECK',reference='1001',bank_id='bank',allocations=[dict(bill_id='bill',amount='50.00')])
pair('payables_review_payment','payables_pay','billpayment',billpay)
check=dict(request_key=key(7),bank_id='bank',payee_kind='OTHER',payee_name='Other supplier',date='2026-10-07',check_number='1002',lines=[dict(tab='EXPENSE',account_id='expense',description='Written check',rate='10.00')])
add('checks_save',ref='check',values=check)
add('checks_review',check='check',expected_revision=1,review='check')
add('checks_record',check='check',expected_revision=1,review='check')
pair('transfers_review','transfers_record','transfer',dict(request_key=key(8),from_account_id='bank',to_account_id='bank2',date='2026-10-08',amount='25.00'))
pending={**billpay,'request_key':key(15),'reference':'1003','amount':'1.00','allocations':[dict(bill_id='bill',amount='1.00')]}
add('payables_review_payment',review='before-refund',values=pending)
refund=dict(request_key=key(9),date='2026-10-09',amount='100.00',bank_id='bank',method='CHECK',check_number='1003')
pair('pp_review_refund','pp_record_refund','refund',refund,deposit='deposit')
add('payables_pay',review='before-refund',ref='blocked-after-review',values=pending,expect_error=True)
for n,number in enumerate(['1001','1002'],10):
    add('pp_review_refund',deposit='deposit',review='collision'+str(n),values={**refund,'request_key':key(n),'check_number':number,'amount':'1.00'},expect_error=True)
add('checks_save',ref='blockedcheck',values={**check,'request_key':key(12),'check_number':'1003'},expect_error=True)
add('payables_review_payment',review='blockedpayment',values={**billpay,'request_key':key(13),'reference':'1003'},expect_error=True)
add('checks_next',bank='bank',ref='historical-next')
add('sales_void_invoice',record='invoice',expected_revision=2,date='2026-10-20',reason='Synthetic correction',expect_error=True)
add('sales_reverse_receipt',record='payment',expected_revision=1,date='2026-10-20',reason='Synthetic correction')
add('sales_void_invoice',record='invoice',expected_revision=2,date='2026-10-20',reason='Synthetic correction',expect_error=True)
add('pp_unapply',record='application',expected_revision=1,date='2026-10-21',reason='Synthetic correction')
add('sales_void_invoice',record='invoice',expected_revision=2,date='2026-10-20',reason='Synthetic correction',expect_error=True)
add('sales_void_invoice',record='invoice',expected_revision=2,date='2026-10-21',reason='Synthetic correction')
add('pp_void_refund',record='refund',expected_revision=1,date='2026-10-22',reason='Synthetic correction')
add('pp_void_deposit',record='deposit',expected_revision=1,date='2026-10-22',reason='Synthetic correction')
add('payables_reverse',payment='billpayment',expected_revision=1,date='2026-10-23',reason='Synthetic correction')
add('payables_void',bill='bill',expected_revision=2,date='2026-10-23',reason='Synthetic correction')
add('checks_void',check='check',expected_revision=2,date='2026-10-24',reason='Synthetic correction')
add('transfers_void',transfer='transfer',expected_revision=1,date='2026-10-24',reason='Synthetic correction')
add('checks_save',ref='reuse-refund-number',values={**check,'request_key':key(14),'date':'2026-10-25','check_number':'1003'})
add('checks_next',bank='bank',ref='released-next')
scenario=dict(users=[dict(username='integration',is_staff=True)],capture_steps=True,steps=steps,as_of=['2026-10-01','2026-10-03','2026-10-09','2026-10-21','2026-10-24','2026-10-31'])

# Credits, bank deposits, reconciliation and release guards share this company.
counter=0xf0000
def extension_key():
    global counter
    counter+=1;return str(UUID(int=counter))
def paired(review_op,record_op,ref,values,**kw):
    add(review_op,review=ref,values=values,**kw)
    add(record_op,review=ref,ref=ref,values=copy.deepcopy(values),**kw)
invoice=copy.deepcopy(next(s for s in steps if s['op']=='sales_save_invoice'))
invoice['ref']='credit-invoice';invoice['values'].update(request_key=extension_key(),date='2026-10-25');invoice['values']['lines'][0]['rate']='100.00';steps.append(invoice)
add('sales_review_invoice',record='credit-invoice',review='credit-invoice',expected_revision=1)
add('sales_post_invoice',record='credit-invoice',review='credit-invoice',expected_revision=1)
memo=dict(request_key=extension_key(),customer_id='buyer',date='2026-10-25',tax_mode='NONE',lines=[dict(item_id=None,description='Return',quantity='1',rate='100.00',taxable=False,income_account_id='income')])
paired('credits_review_memo','credits_record_memo','memo',memo)
application=dict(request_key=extension_key(),date='2026-10-26',allocations=[dict(invoice_id='credit-invoice',amount='60.00')])
paired('credits_review_application','credits_apply','credit-application',application,record='memo')
add('sales_void_invoice',record='credit-invoice',expected_revision=2,date='2026-10-26',reason='Blocked by credit',expect_error=True)
refund=dict(request_key=extension_key(),date='2026-10-27',amount='40.00',bank_id='bank',method='CHECK',check_number='1004')
collision=dict(refund,request_key=extension_key(),check_number='1003')
add('credits_review_refund',record='memo',review='blocked-credit-refund',values=collision,expect_error=True)
paired('credits_review_refund','credits_refund','credit-refund',refund,record='memo')
check=copy.deepcopy(next(s for s in steps if s['op']=='checks_save'));check['ref']='blocked-credit-number';check['values'].update(request_key=extension_key(),check_number='1004');check['expect_error']=True;check['expect_kind']='ValidationError';steps.append(check)
add('checks_next',bank='bank',ref='credit-number-hint')
sale=dict(request_key=extension_key(),customer_id='buyer',date='2026-10-28',method='CASH',destination='UNDEPOSITED',tax_mode='NONE',lines=[dict(item_id=None,description='Cash sale',quantity='1',rate='70.00',taxable=False,income_account_id='income')])
paired('sales_review_sale','sales_record_sale','undeposited-sale',sale)
payment=dict(request_key=extension_key(),customer_id='buyer',date='2026-10-28',amount='40.00',method='CASH',destination='UNDEPOSITED',allocations=[dict(invoice_id='credit-invoice',amount='40.00')])
paired('sales_review_receipt','sales_record_receipt','undeposited-payment',payment)
deposit=dict(request_key=extension_key(),customer_id='buyer',date='2026-10-28',amount='30.00',method='CHECK',reference='Deposit reference',destination='UNDEPOSITED')
paired('pp_review_deposit','pp_record_deposit','undeposited-customer-deposit',deposit)
add('pp_review_application',deposit='undeposited-customer-deposit',review='blocked-by-credit-settlement',values=dict(request_key=extension_key(),date='2026-10-28',invoice_id='credit-invoice',amount='0.01'),expect_error=True)
bankdep=dict(request_key=extension_key(),date='2026-10-29',bank_id='bank',receipt_ids=['undeposited-sale','undeposited-payment','undeposited-customer-deposit'])
paired('banking_review','banking_deposit','bank-deposit',bankdep)
for op,field,ref in [('sales_void_sale','record','undeposited-sale'),('sales_reverse_receipt','record','undeposited-payment'),('pp_void_deposit','record','undeposited-customer-deposit')]:
    add(op,**{field:ref},expected_revision=1,date='2026-10-30',reason='Blocked by bank deposit',expect_error=True)
for line in ['credit-refund:journal:2','bank-deposit:posted:1']:
    add('banking_clear',line=line,state='C',expected_revision=0)
rec=dict(request_key=extension_key(),statement_date='2026-10-31',ending_balance='99.00',service_charge='2.00',service_account_id='expense',interest='1.00',interest_account_id='income')
add('banking_begin',account='bank',ref='reconciliation',values=rec)
add('banking_screen',account='bank',reconciliation='reconciliation',result='zero-difference')
add('banking_finish',account='bank',reconciliation='reconciliation',expected_revision=1,completion_key=extension_key())
add('banking_report',account='bank',reconciliation='reconciliation',result='completed')
add('credits_void_refund',record='credit-refund',expected_revision=1,date='2026-10-31',reason='Blocked by reconciliation',expect_error=True)
add('banking_reverse',deposit='bank-deposit',expected_revision=1,date='2026-10-31',reason='Blocked by reconciliation',expect_error=True)
add('banking_register',account='bank',result='integrated-register',values={})
for field,text in [('number','1004'),('number','1003'),('payee','Buyer'),('payee','bank2'),('number','Deposit reference')]:
    add('banking_register',account='bank',result='find-'+field+'-'+text,values={field:text})
add('banking_undo',account='bank',reconciliation='reconciliation',expected_revision=2,reason='Synthetic correction',undo_key=extension_key())
add('banking_report',account='bank',reconciliation='reconciliation',result='undone')
for line in ['credit-refund:journal:2','bank-deposit:posted:1']:
    add('banking_clear',line=line,state='U',expected_revision=3)
add('banking_reverse',deposit='bank-deposit',expected_revision=1,date='2026-11-01',reason='Synthetic release')
for op,ref in [('sales_void_sale','undeposited-sale'),('sales_reverse_receipt','undeposited-payment'),('pp_void_deposit','undeposited-customer-deposit')]:
    add(op,record=ref,expected_revision=1,date='2026-10-31',reason='Backdated release',expect_error=True)
    add(op,record=ref,expected_revision=1,date='2026-11-01',reason='Synthetic release')
add('credits_void_refund',record='credit-refund',expected_revision=1,date='2026-11-01',reason='Synthetic correction')
add('credits_unapply',record='credit-application',expected_revision=1,date='2026-11-01',reason='Synthetic correction')
add('sales_void_invoice',record='credit-invoice',expected_revision=2,date='2026-10-31',reason='Backdated credit release',expect_error=True)
add('sales_void_invoice',record='credit-invoice',expected_revision=2,date='2026-11-01',reason='Synthetic correction')
add('credits_void_memo',record='memo',expected_revision=1,date='2026-11-01',reason='Synthetic correction')
scenario['as_of']=scenario['as_of'][:6]+['2026-10-29','2026-11-01'];scenario['today']='2026-10-30'


# Reviewed intended refusals, independent of the implementations at generation time.
REFUSALS = {33: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'Check number 1003 is already used by deposit refund CDR-000001 on this bank '
                        'account. Choose another number or void that refund first.'},
 34: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'Check number 1001 is already used by bill payment BPAY-000001 on this bank account. '
                        'Choose another number or reverse that payment first.'},
 35: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'Check number 1002 is already used by check CHK-000001 on this bank account. Choose '
                        'another number or void that check first.'},
 36: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'Check number 1003 is already used by deposit refund CDR-000001 on this bank '
                        'account. Choose another number or void that refund first.'},
 37: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'Check number 1003 is already used by deposit refund CDR-000001 on this bank '
                        'account. Choose another number or void that refund first.'},
 39: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'Reverse every live receipt allocated to this invoice before voiding it.'},
 41: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'Unapply customer deposit CDP-000001 from this invoice before voiding it.'},
 43: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'Void date cannot precede 2026-10-21, when a customer deposit was unapplied from '
                        'this invoice.'},
 60: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'Unapply credit memo CM-000001 from this invoice before voiding it.'},
 61: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'Check number 1003 is already used by check CHK-000002 on this bank account. Choose '
                        'another number or void that check first.'},
 64: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'Check number 1004 is already used by refund RFD-000001 on this bank account. Choose '
                        'another number or void that refund first.'},
 72: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'The amount to apply (0.01) exceeds invoice INV-000002’s open balance 0.00.'},
 75: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'Reverse deposit DEP-000001 before reversing this receipt.'},
 76: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'Reverse deposit DEP-000001 before reversing this receipt.'},
 77: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'Reverse deposit DEP-000001 before reversing this receipt.'},
 84: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'RFD-000001 is reconciled in the 100 · bank register (statement 2026-10-31). It '
                        'cannot be voided unless that reconciliation is undone: Banking → Reconcile → Undo '
                        'last reconciliation, then mark it not cleared and void it.'},
 85: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'DEP-000001 is reconciled in the 100 · bank register (statement 2026-10-31). It '
                        'cannot be voided unless that reconciliation is undone: Banking → Reconcile → Undo '
                        'last reconciliation, then mark it not cleared and void it.'},
 97: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'SR-000001 was released from deposit DEP-000001 on 2026-11-01; the receipt reversal '
                        'cannot be dated earlier.'},
 99: {'expect_error': True,
      'expect_kind': 'ValidationError',
      'expect_message': 'RCP-000002 was released from deposit DEP-000001 on 2026-11-01; the receipt reversal '
                        'cannot be dated earlier.'},
 101: {'expect_error': True,
       'expect_kind': 'ValidationError',
       'expect_message': 'CDP-000002 was released from deposit DEP-000001 on 2026-11-01; the receipt '
                         'reversal cannot be dated earlier.'},
 105: {'expect_error': True,
       'expect_kind': 'ValidationError',
       'expect_message': 'Void date cannot precede 2026-11-01, when a credit memo was unapplied from this '
                         'invoice.'}}
for index,step in enumerate(steps):
    assert step['expect_error'] == (index in REFUSALS),(index,step)
    step.update(REFUSALS.get(index,{}))

assert len(steps)==108
out=Path(os.environ.get('FB_SCENARIO_OUT',Path(__file__).resolve().parents[1]/'scenarios'))
(out/'int-l3-combined.json').write_text(json.dumps(scenario,indent=2,ensure_ascii=False)+'\n')
