"""D9 screen-diff scenario: the combined D7b scenario plus one bank-feed company.

    python3 tools/d9_scenario.py

Writes crates/fb-screendiff/scenarios/screens-d9.json. The bank-feed rows come from the harness's own
`bankfeeds_*` steps with a pre-parsed pull (no network, no provider, synthetic data only). The company
is separate from Kettle Pond Supply so no earlier screen changes. Accepted rows span a check, a deposit,
a transfer and a journal entry; NEW rows carry a match, a receipt and a rule suggestion.
"""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT/'crates/fb-screendiff/scenarios/screens-d7b.json'
OUT = ROOT/'crates/fb-screendiff/scenarios/screens-d9.json'
CO = 'd9'


def step(op, **kw):
    s = {'op': op, 'company': CO}
    s.update(kw)
    s.setdefault('expect_error', False)
    return s


def tx(remote_id, day, amount, description, pending=False):
    return {'remote_id': remote_id, 'posted_on': day, 'transacted_on': None, 'amount': amount, 'description': description, 'pending': pending}


def feed(remote_id, name, transactions, balance='1000.00'):
    return {'conn_key': 'org1', 'remote_id': remote_id, 'name': name, 'org_name': 'Synthetic Bank', 'currency': 'USD', 'balance': balance,
            'available': balance, 'balance_epoch': 1790000000, 'skipped': [], 'transactions': transactions}


def sale(n, amount):
    values = {'request_key': '00000000-0000-0000-0000-0000000d9%03d' % n, 'customer_id': 'd9-buyer', 'date': '2026-10-04', 'memo': 'Synthetic feed sale',
              'tax_mode': 'NONE', 'lines': [{'item_id': None, 'description': 'Service', 'quantity': '1', 'rate': amount, 'taxable': False, 'income_account_id': 'd9-income'}],
              'method': 'CASH', 'destination': 'UNDEPOSITED'}
    return [step('sales_review_sale', values=values, review='d9-sale%d' % n), step('sales_record_sale', values=values, review='d9-sale%d' % n, ref='d9-sale%d' % n)]


def d9_steps():
    steps = [{'op': 'create_company', 'ref': CO, 'name': 'D9 feed books', 'currency': 'USD', 'expect_error': False}]
    for ref, code, name, kind in [('bank', '1000', 'Checking', 'ASSET'), ('savings', '1010', 'Savings', 'ASSET'), ('ar', '1100', 'Receivables', 'ASSET'),
                                  ('uf', '1200', 'Undeposited', 'ASSET'), ('ap', '2000', 'Payables', 'LIABILITY'), ('card', '2100', 'Card', 'LIABILITY'),
                                  ('tax', '2200', 'Tax', 'LIABILITY'), ('equity', '3000', 'Equity', 'EQUITY'), ('income', '4000', 'Income', 'INCOME'),
                                  ('expense', '6000', 'Expense', 'EXPENSE'), ('fees', '6100', 'Bank fees', 'EXPENSE')]:
        steps.append(step('save_account', ref='d9-' + ref, code=code, name=name, kind=kind))
    steps.append(step('l2_save', model='vendor', ref='d9-vendor', values={'display_name': 'Synthetic utility', 'billing_address': 'Test address'}))
    steps.append(step('l2_save', model='customer', ref='d9-buyer', values={'display_name': 'Synthetic buyer'}))
    steps.append(step('l2_setup', expected_revision=0, values={'receivables': 'd9-ar', 'undeposited': 'd9-uf', 'collected_tax': 'd9-tax', 'payables': 'd9-ap', 'banks': ['d9-bank', 'd9-savings']}))
    steps += sale(1, '15.00') + sale(2, '22.00')
    steps.append(step('bankfeeds_connect', ref='d9-conn', label='Synthetic feed'))
    steps.append(step('bankfeeds_connect', ref='d9-old', label='Old feed'))
    steps.append(step('bankfeeds_forget', connection='d9-old'))
    pull = {'accounts': [
        feed('chk', 'Everyday checking', [
            tx('c1', '2026-10-05', '-12.50', 'Utility bill'), tx('c2', '2026-10-05', '-60.00', 'CHECK 1042'),
            tx('d1', '2026-10-06', '15.00', 'Deposit one'), tx('d2', '2026-10-06', '22.00', 'Customer payment'),
            tx('x1', '2026-10-07', '-100.00', 'To savings'), tx('j1', '2026-10-08', '-3.10', 'Service charge'),
            tx('r1', '2026-10-08', '-45.00', 'OFFICE DEPOT #221 PURCHASE'), tx('m1', '2026-10-09', '-30.00', 'Matched'),
            tx('ig1', '2026-10-09', '-1.00', 'Ignore me'), tx('pend', None, '-2.00', 'Pending', pending=True)]),
        feed('cc', 'Card', [tx('cc1', '2026-10-05', '-25.00', 'Card purchase')], balance='-25.00'),
        feed('unm', 'Unmapped', [tx('u1', '2026-10-05', '-5.00', 'Unmapped')]),
    ]}
    steps.append(step('bankfeeds_sync', connection='d9-conn', now='2026-10-10T15:00:00Z', pull=pull))
    steps.append(step('bankfeeds_map', feed='d9-conn/org1/chk', account='d9-bank'))
    steps.append(step('bankfeeds_map', feed='d9-conn/org1/cc', account='d9-card'))
    steps.append(step('bankfeeds_save_rule', ref='d9-rule-office', values={'name': 'Office supplies', 'match_text': 'office depot', 'sign': 'OUT', 'account_id': 'd9-expense', 'payee_name': 'Office Depot'}))
    steps.append(step('bankfeeds_save_rule', ref='d9-rule-interest', values={'name': 'Interest', 'match_text': 'interest', 'sign': 'IN', 'amount_min': '0.01', 'amount_max': '5.00', 'account_id': 'd9-income'}))
    steps.append(step('bankfeeds_rule_active', rule='d9-rule-interest', active=False))
    steps.append(step('bankfeeds_accept', tx='d9-conn/org1/chk/c1', ref='d9-chk1', values={'kind': 'CHECK', 'account_id': 'd9-expense', 'vendor_id': 'd9-vendor', 'payee_name': 'Synthetic utility'}))
    steps.append(step('bankfeeds_accept', tx='d9-conn/org1/chk/d1', ref='d9-dep1', values={'kind': 'DEPOSIT', 'receipt_ids': ['d9-sale1']}))
    steps.append(step('bankfeeds_accept', tx='d9-conn/org1/chk/x1', ref='d9-trf1', values={'kind': 'TRANSFER', 'transfer_account_id': 'd9-savings'}))
    steps.append(step('bankfeeds_accept', tx='d9-conn/org1/chk/j1', ref='d9-je1', values={'kind': 'JOURNAL', 'account_id': 'd9-fees', 'payee_name': 'Bank'}))
    steps.append(step('bankfeeds_ignore', tx='d9-conn/org1/chk/ig1'))
    steps.append(step('save_draft', ref='d9-hand', date='2026-10-09', memo='By hand', lines=[{'account': 'd9-expense', 'debit': '30.00', 'credit': '0'}, {'account': 'd9-bank', 'debit': '0', 'credit': '30.00'}]))
    steps.append(step('post_journal', journal='d9-hand'))
    # The AI assistant page: this company allows reading attachments; Kettle Pond Supply has no row.
    steps.append(step('assistant_switch', enabled=True))
    # A draft bill with one attached file and one removed file (the attachments panel).
    steps.append(step('payables_save', ref='d9-bill', values={'vendor_id': 'd9-vendor', 'date': '2026-10-20', 'due_date': '2026-11-20', 'reference': 'Feed supplies',
                                                              'memo': '', 'request_key': '00000000-0000-0000-0000-0000000d9b01',
                                                              'lines': [{'description': 'Supplies', 'quantity': '1', 'rate': '45.00', 'expense_account_id': 'd9-expense'}]}))
    pdf = '%PDF-1.{v}\n%\xe2\xe3\xcf\xd3\n1 0 obj<</Type/Catalog>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n'
    steps.append(step('attachment_add', ref='d9-file1', name='scan.pdf', hex=pdf.format(v=4).encode('latin-1').hex(), request_key='00000000-0000-0000-0000-0000000d9a01', document='d9-bill:source'))
    steps.append(step('attachment_add', ref='d9-file2', name='wrong.pdf', hex=pdf.format(v=5).encode('latin-1').hex(), request_key='00000000-0000-0000-0000-0000000d9a02', document='d9-bill:source'))
    steps.append(step('attachment_remove', attachment='d9-file2', reason='Wrong file'))
    # J-ATTACH: the manual journal page's panel with one file.
    steps.append(step('attachment_add', ref='d9-file3', name='opening.pdf', hex=pdf.format(v=6).encode('latin-1').hex(), request_key='00000000-0000-0000-0000-0000000d9a04', journal='d9-hand'))
    return steps


def ai_steps():
    """A second company with a connected (synthetic, fake-credential) ChatGPT account, reading switched on,
    and a draft bill with one attachment: the bill page's Read with AI section in its available state."""
    co = 'd9ai'

    def st(op, **kw):
        s = step(op, **kw)
        s['company'] = co
        return s
    steps = [{'op': 'create_company', 'ref': co, 'name': 'D9 AI books', 'currency': 'USD', 'expect_error': False}]
    for ref, code, name, kind in [('bank', '1000', 'Checking', 'ASSET'), ('ar', '1100', 'Receivables', 'ASSET'), ('uf', '1200', 'Undeposited', 'ASSET'),
                                  ('ap', '2000', 'Payables', 'LIABILITY'), ('tax', '2200', 'Tax', 'LIABILITY'), ('expense', '6000', 'Expense', 'EXPENSE')]:
        steps.append(st('save_account', ref='d9ai-' + ref, code=code, name=name, kind=kind))
    steps.append(st('l2_save', model='vendor', ref='d9ai-vendor', values={'display_name': 'Synthetic stationer'}))
    steps.append(st('l2_setup', expected_revision=0, values={'receivables': 'd9ai-ar', 'undeposited': 'd9ai-uf', 'collected_tax': 'd9ai-tax', 'payables': 'd9ai-ap', 'banks': ['d9ai-bank']}))
    steps.append(st('assistant_connect', state={'mode': 'siwc', 'key_last4': '', 'account_email': 'synthetic@example.invalid', 'registration': True,
                                                'model_choices': [{'slug': 'demo-model', 'display_name': 'Demo'}]}))
    steps.append(st('assistant_switch', enabled=True))
    steps.append(st('payables_save', ref='d9ai-bill', values={'vendor_id': 'd9ai-vendor', 'date': '2026-10-21', 'due_date': '2026-11-21', 'reference': 'Paper',
                                                             'memo': '', 'request_key': '00000000-0000-0000-0000-0000000d9b02',
                                                             'lines': [{'description': 'Paper', 'quantity': '2', 'rate': '7.50', 'expense_account_id': 'd9ai-expense'}]}))
    pdf = '%PDF-1.6\n%\xe2\xe3\xcf\xd3\n1 0 obj<</Type/Catalog>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n'
    steps.append(st('attachment_add', ref='d9ai-file', name='receipt.pdf', hex=pdf.encode('latin-1').hex(), request_key='00000000-0000-0000-0000-0000000d9a03', document='d9ai-bill:source'))
    return steps


if __name__ == '__main__':
    scenario = json.loads(SRC.read_text())
    scenario['name'] = 'screens-d9'
    scenario['steps'] += d9_steps() + ai_steps()
    OUT.write_text(json.dumps(scenario, indent=2, ensure_ascii=False) + '\n')
    print('D9: wrote', OUT.relative_to(ROOT), len(scenario['steps']), 'steps')
