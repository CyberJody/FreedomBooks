"""Synthetic L3 hardening adoption, through both frozen Python and Rust services."""
import copy
import json
from pathlib import Path

scenarios = Path(__file__).resolve().parents[1] / 'scenarios'
base = json.loads((scenarios / 'core-l3-combined.json').read_text())
steps = copy.deepcopy(base['steps'][:14])
invoice = copy.deepcopy(next(s['values'] for s in base['steps'] if s['op'] == 'sales_save_invoice'))
bill = copy.deepcopy(next(s['values'] for s in base['steps'] if s['op'] == 'payables_save'))
bill['lines'][0]['item_id'] = 'work'

def add(op, **values):
    steps.append(copy.deepcopy(dict(op=op, company='c', **values)))

# Configuration/default references alone do not freeze an unused account's kind.
add('save_account', account='ar', code='110', name='ar', kind='LIABILITY', detail_type='OTHER_CURRENT_LIABILITY')
add('save_account', account='ar', code='110', name='ar', kind='ASSET', detail_type='OTHER_CURRENT_ASSET')
add('sales_save_invoice', ref='draftinv', values=invoice)
add('payables_save', ref='draftbill', values=bill)
for model, record in [('customer', 'buyer'), ('vendor', 'vendor'), ('item', 'work')]:
    add('l2_active', model=model, record=record, active=False)

# New archived selections are refused. Existing draft selections require keep.
new_invoice = copy.deepcopy(invoice)
new_invoice['request_key'] = '00000000-0000-0000-0000-000000000051'
add('sales_save_invoice', ref='refused-invoice', values=new_invoice, expect_error=True)
new_bill = copy.deepcopy(bill)
new_bill.update(request_key='00000000-0000-0000-0000-000000000052', reference='Other')
add('payables_save', ref='refused-bill', values=new_bill, expect_error=True)
add('sales_save_invoice', ref='draftinv', record='draftinv', expected_revision=1, values=invoice, expect_error=True)
edit_bill = copy.deepcopy(bill)
edit_bill.update(bill_id='draftbill', expected_revision=1)
add('payables_save', ref='draftbill', values=edit_bill, expect_error=True)
edit_invoice = copy.deepcopy(invoice)
edit_invoice['keep_archived'] = True
edit_invoice['lines'][0]['keep_archived'] = True
edit_bill['keep_archived'] = True
edit_bill['lines'][0]['keep_archived'] = True
add('sales_save_invoice', ref='draftinv', record='draftinv', expected_revision=1, values=edit_invoice)
add('payables_save', ref='draftbill', values=edit_bill)
add('sales_review_invoice', record='draftinv', expected_revision=2, review='archived', expect_error=True)
add('payables_review', bill='draftbill', expected_revision=2, review='archived', expect_error=True)
for model, record in [('customer', 'buyer'), ('vendor', 'vendor'), ('item', 'work')]:
    add('l2_active', model=model, record=record, active=True)

# Invalid optional values must fail rather than be treated as omissions.
for field, value in [('memo', []), ('refresh_customer', None), ('keep_archived', 0)]:
    invalid = copy.deepcopy(invoice)
    invalid[field] = value
    invalid['request_key'] = '00000000-0000-0000-0000-000000000053'
    add('sales_save_invoice', ref='invalid-invoice', values=invalid, expect_error=True)
for field, value in [('memo', []), ('refresh_vendor', None), ('reference', None)]:
    invalid = copy.deepcopy(new_bill)
    invalid[field] = value
    add('payables_save', ref='invalid-bill', values=invalid, expect_error=True)
add('sales_review_invoice', record='draftinv', expected_revision=2, review='ready')
add('sales_post_invoice', record='draftinv', expected_revision=2, review='ready')
add('payables_post', bill='draftbill', expected_revision=2)
result = dict(users=base['users'], steps=steps, as_of=base['as_of'])
(scenarios / 'core-l3-hardening.json').write_text(json.dumps(result, indent=2) + '\n')
