"""Extra cross-module, card, lock, and report cases, kept separate from random generator."""
from pathlib import Path
import json,copy,uuid
root=Path(__file__).resolve().parents[1]
counter=20000
def key():
    global counter
    counter+=1;return str(uuid.UUID(int=counter))
def op(opname,**kw):return dict(op=opname,company='c',**kw)
base=json.loads((root/'scenarios/banking-deposits.json').read_text());base['steps']=base['steps'][:12]
def save(name,steps):
    v=copy.deepcopy(base);v['steps']+=steps;(root/f'scenarios/banking-{name}.json').write_text(json.dumps(v,indent=1)+'\n')
steps=[op('save_account',ref='card',code='220',name='Synthetic card',kind='LIABILITY',detail_type='CREDIT_CARD'),op('save_draft',ref='purchase',date='2026-10-01',memo='Card purchase',lines=[dict(account='expense',debit='100'),dict(account='card',credit='100')]),op('post_journal',journal='purchase'),op('banking_clear',line='purchase:2',state='C',expected_revision=0)]
steps+=[op('banking_begin',ref='bad-interest',account='card',values=dict(request_key=key(),statement_date='2026-10-31',ending_balance='100',interest='1',interest_account_id='income'),expect_error=True)]
steps+=[op('banking_begin',ref='card-statement',account='card',values=dict(request_key=key(),statement_date='2026-10-31',ending_balance='105',service_charge='5',service_account_id='expense')),op('banking_screen',account='card',reconciliation='card-statement',result='card-screen'),op('banking_finish',account='card',reconciliation='card-statement',expected_revision=1,completion_key=key()),op('banking_report',account='card',reconciliation='card-statement',result='card-done'),op('banking_register',account='card',values={},result='card-register'),op('banking_undo',account='card',reconciliation='card-statement',expected_revision=2,reason='Card correction',undo_key=key()),op('banking_report',account='card',reconciliation='card-statement',result='card-undone')];save('credit-card',steps)
steps=[op('save_draft',ref='entry',date='2026-10-01',memo='Clearing locks',entry_number='TEST-1',lines=[dict(account='bank',debit='10'),dict(account='income',credit='10')]),op('post_journal',journal='entry'),op('banking_clear',line='entry:1',state='C',expected_revision=0)]
refused=op('banking_reverse_journal',journal='entry',ref='reversal',date='2026-10-31',reason='Correction',expect_error=True);steps+=[refused,op('banking_begin',ref='statement',account='bank',values=dict(request_key=key(),statement_date='2026-10-31',ending_balance='10')),op('banking_finish',account='bank',reconciliation='statement',expected_revision=1,completion_key=key()),copy.deepcopy(refused),op('banking_clear',line='entry:1',state='U',expected_revision=2,expect_error=True),op('banking_undo',account='bank',reconciliation='statement',expected_revision=2,reason='Correction',undo_key=key()),op('banking_clear',line='entry:1',state='U',expected_revision=3),{**refused,'expect_error':False},op('banking_register',account='bank',values={},result='reversed')];save('clearing-locks',steps)
inv=dict(request_key=key(),customer_id='buyer',date='2026-10-01',due_date='2026-10-31',memo='Payment through original funds',tax_mode='NONE',lines=[dict(item_id=None,description='Invoice',quantity='1',rate='25',taxable=False,income_account_id='income')])
steps=[op('sales_save_invoice',ref='invoice',values=inv,expected_revision=0),op('sales_review_invoice',record='invoice',review='invoice-review',expected_revision=1),op('sales_post_invoice',record='invoice',review='invoice-review',expected_revision=1)]
pay=dict(request_key=key(),customer_id='buyer',date='2026-10-02',amount='25',method='CHECK',destination='UNDEPOSITED',allocations=[dict(invoice_id='invoice',amount='25')]);steps+=[op('sales_review_receipt',values=pay,review='payment'),op('sales_record_receipt',ref='payment',values=pay,review='payment')]
steps+=[op('save_account',ref='uf2',code='121',name='Other Undeposited',kind='ASSET'),op('l2_setup',expected_revision=1,values=dict(receivables='ar',undeposited='uf2',collected_tax='tax',banks=['bank']))]
sale=dict(request_key=key(),customer_id='buyer',date='2026-10-02',memo='Second original destination',tax_mode='NONE',method='CASH',destination='UNDEPOSITED',lines=[dict(item_id=None,description='Sale',quantity='1',rate='10',taxable=False,income_account_id='income')]);steps+=[op('sales_review_sale',values=sale,review='sale'),op('sales_record_sale',ref='sale',values=sale,review='sale')]
dep=dict(request_key=key(),bank_id='bank',date='2026-10-03',receipt_ids=['payment','sale']);steps+=[op('banking_deposit',ref='mixed',values=dep),op('banking_register',account='bank',values={'payee':'Buyer'},result='found-party'),op('banking_receipt_guard',receipt='payment',date='2026-10-04',expect_error=True),op('banking_reverse',deposit='mixed',expected_revision=1,date='2026-10-05',reason='Synthetic correction'),op('banking_receipt_guard',receipt='payment',date='2026-10-04',expect_error=True),op('sales_reverse_receipt',record='payment',expected_revision=1,date='2026-10-05',reason='Synthetic correction')];save('payments-original-funds',steps)
steps=[op('save_account',account='bank',code='100',name='bank',kind='EXPENSE'),op('banking_begin',ref='reclassified',account='bank',values=dict(request_key=key(),statement_date='2026-10-31',ending_balance='1.00',interest='1.00',interest_account_id='income')),op('banking_finish',account='bank',reconciliation='reclassified',expected_revision=1,completion_key=key()),op('banking_register',account='bank',values={},result='reclassified-register')];save('reclassified-designation',steps)
# Existing payables workflows must resolve their source/party/check metadata in the bank register too.
payables=json.loads((root/'scenarios/payables-payments.json').read_text())
payables['steps'].append(dict(op='banking_register',company='co',account='bank',values={},result='payables-register'))
(root/'scenarios/banking-payable-register.json').write_text(json.dumps(payables,indent=1)+'\n')
# Every successful completion and undo gets a real frozen-reference report; every completion a screen.
for p in root.glob('scenarios/banking-*.json'):
    v=json.loads(p.read_text());new=[]
    for i,s in enumerate(v['steps']):
        if s['op']=='banking_finish' and not s.get('expect_error'):
            new.append(op('banking_screen',account=s['account'],reconciliation=s['reconciliation'],result=f'auto-screen-{i}'))
        new.append(s)
        if s['op'] in ('banking_finish','banking_undo') and not s.get('expect_error'):
            new.append(op('banking_report',account=s['account'],reconciliation=s['reconciliation'],result=f'auto-report-{i}'))
    v['steps']=new;p.write_text(json.dumps(v,indent=1)+'\n')
