"""Populate directory activity dependencies through the existing harness services."""
from pathlib import Path
import json

p=Path('crates/fb-screendiff/scenarios/screens-d3.json');s=json.loads(p.read_text())
def step(op,**kw):s['steps'].append(dict(op=op,company='c',expect_error=False,**kw))
def key(n):return f'00000000-0000-0000-0000-{700+n:012x}'
step('save_account',ref='ap',code='2000',name='Payables',detail_type='PAYABLE')
step('save_account',ref='cdp',code='2300',name='Customer deposits',detail_type='OTHER_CURRENT_LIABILITY')
step('l2_setup',expected_revision=1,values=dict(receivables='ar',undeposited='uf',payables='ap',collected_tax='tax',customer_deposits='cdp',banks=['bank']))
step('payables_save',ref='bill',values=dict(vendor_id='vendor',date='2026-10-01',due_date='2026-10-31',reference='DELIVERY-1',request_key=key(1),lines=[dict(description='Supplies',quantity='1',rate='30.50',expense_account_id='expense')]))
step('payables_post',bill='bill',expected_revision=1)
step('payables_pay',ref='billpay',values=dict(vendor_id='vendor',date='2026-10-15',amount='10.00',method='OTHER',reference='Sample payment',memo='',bank_id='bank',request_key=key(2),allocations=[dict(bill_id='bill',amount='10.00')]))
step('checks_save',ref='check',values=dict(bank_id='bank',payee_kind='VENDOR',vendor_id='vendor',date='2026-10-20',check_number='101',memo='Supplies check',request_key=key(3),lines=[dict(tab='EXPENSE',account_id='expense',rate='12.34')]))
step('checks_record',check='check',expected_revision=1)
v=dict(request_key=key(4),customer_id='buyer',date='2026-10-16',memo='Returned service',tax_mode='NONE',lines=[dict(description='Return',quantity='1',rate='10.00',taxable=False,income_account_id='income')])
step('credits_review_memo',review='memo-review',values=v);step('credits_record_memo',review='memo-review',ref='memo',values=v)
v=dict(request_key=key(5),date='2026-10-17',allocations=[dict(invoice_id='inv1',amount='3.00')])
step('credits_review_application',record='memo',review='apply-review',values=v);step('credits_apply',record='memo',review='apply-review',ref='memo-application',values=v)
v=dict(request_key=key(6),date='2026-10-18',amount='2.00',bank_id='bank',method='OTHER',check_number='',memo='Refund')
step('credits_review_refund',record='memo',review='refund-review',values=v);step('credits_refund',record='memo',review='refund-review',ref='memo-refund',values=v)
v=dict(request_key=key(7),customer_id='buyer',date='2026-10-19',amount='50.00',method='CHECK',destination='BANK',bank_id='bank',order_reference='ORDER-1')
step('pp_review_deposit',review='deposit-review',values=v);step('pp_record_deposit',review='deposit-review',ref='deposit',values=v)
p.write_text(json.dumps(s,indent=2)+'\n')

# Complete identity maps for links in activity fragments; values are still scoped by model/context.
p=Path('crates/fb-screendiff/py/render_django.py');t=p.read_text();old='    from unittest.mock import patch\n';assert old in t
t=t.replace(old,'''    from payables.models import Bill, BillPayment
    from checks.models import Check
    from credits.models import CreditMemo, CreditRefund
    for model, cls in [('bill', Bill), ('billpayment', BillPayment), ('check', Check), ('creditmemo', CreditMemo), ('creditrefund', CreditRefund)]:
        for row in cls.objects.filter(company=company).select_related('identity'):
            ids[model + ':' + row.identity.number] = str(row.pk)
''');p.write_text(t)
p=Path('crates/fb-screendiff/src/main.rs');t=p.read_text();t=t.replace('let queries: [(&str, &str); 8]','let queries: [(&str, &str); 13]');old='                ("vendor", "SELECT display_name, id';assert old in t
extra=''.join(f'                ("{model}", "SELECT i.number, d.id FROM {table} d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=?1"),\n' for model,table in [('bill','payables_bill'),('billpayment','payables_billpayment'),('check','checks_check'),('creditmemo','credits_creditmemo'),('creditrefund','credits_creditrefund')]);t=t.replace(old,extra+old);p.write_text(t)
p=Path('crates/fb-screendiff/src/normalize.rs');t=p.read_text();old='        "receipt_id" => "receipt",';assert old in t;t=t.replace(old,old+'''\n        "bill_id" => "bill",\n        "payment_id" => "billpayment",\n        "check_id" => "check",\n        "memo_id" => "creditmemo",\n        "refund_id" => "creditrefund",''');p.write_text(t)
