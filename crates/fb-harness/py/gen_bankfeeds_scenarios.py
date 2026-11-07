"""Generate the bank-feed harness scenarios (`bankfeeds-*.json`). Synthetic data only.

    python3 crates/fb-harness/py/gen_bankfeeds_scenarios.py

Every scenario sets `strict_outcomes` and explicitly pins each success with `expect_error=false`.
A violated expectation on either side is a comparer failure. The seeded random workflow cannot know which of
its commands are refused (stale picks, decided items, refused maps), so each of its steps carries the
outcome the frozen Python app gives it, pinned with kind and message in `bankfeeds-random-500.expect.json`
(written by `pin_bankfeeds_expectations.py` from a Python run of the unpinned scenario).

Every refusal in every bank-feed scenario pins `expect_kind` and `expect_message` (FIX3; main's rule).
The hand-written scenarios take theirs from `bankfeeds-refusals.expect.json` (written by
`pin_bankfeeds_refusals.py` from a harness run; set FB_BANKFEEDS_UNPINNED=1 to write the scenarios for
that run), and `write` refuses a scenario with an unpinned refusal.
"""
import json
import os
import random

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, '..', 'scenarios')
USERS = [{'username': 'feeds', 'is_staff': True}, {'username': 'outsider'}]
AS_OF = ['2026-09-30', '2026-10-15', '2099-12-31']


def setup(co='co', name='Synthetic feeds', currency='USD'):
    acct = lambda ref, code, nm, kind, **kw: dict(op='save_account', company=co, ref=f'{co}.{ref}' if co != 'co' else ref, code=code, name=nm, kind=kind, **kw)
    r = (lambda ref: ref) if co == 'co' else (lambda ref: f'{co}.{ref}')
    return [
        dict(op='create_company', ref=co, name=name, currency=currency),
        acct('bank', '100', 'Checking', 'ASSET'),
        acct('savings', '101', 'Savings', 'ASSET'),
        acct('card', '210', 'Card', 'LIABILITY'),
        acct('ar', '110', 'Receivables', 'ASSET'),
        acct('uf', '120', 'Undeposited', 'ASSET'),
        acct('ap', '200', 'Payables', 'LIABILITY'),
        acct('tax', '220', 'Tax', 'LIABILITY'),
        acct('old', '130', 'Old bank', 'ASSET', active=False),
        acct('equity', '300', 'Equity', 'EQUITY'),
        acct('income', '400', 'Income', 'INCOME'),
        acct('expense', '500', 'Expense', 'EXPENSE'),
        acct('fees', '510', 'Bank fees', 'EXPENSE'),
        dict(op='l2_save', company=co, model='vendor', ref=r('vendor'), values={'display_name': 'Synthetic utility', 'billing_address': 'Test address'}),
        dict(op='l2_save', company=co, model='customer', ref=r('buyer'), values={'display_name': 'Synthetic buyer'}),
        dict(op='l2_setup', company=co, expected_revision=0, values={'receivables': r('ar'), 'undeposited': r('uf'), 'collected_tax': r('tax'), 'payables': r('ap'), 'banks': [r('bank'), r('savings')]}),
    ]


def sale(co, ref, amount, date, key):
    values = {'request_key': f'00000000-0000-0000-0000-{key:012x}', 'customer_id': 'buyer', 'date': date, 'memo': 'Synthetic feed sale', 'tax_mode': 'NONE',
              'lines': [{'item_id': None, 'description': 'Service', 'quantity': '1', 'rate': amount, 'taxable': False, 'income_account_id': 'income'}],
              'method': 'CASH', 'destination': 'UNDEPOSITED'}
    return [dict(op='sales_review_sale', company=co, values=values, review=ref), dict(op='sales_record_sale', company=co, values=values, review=ref, ref=ref)]


def tx(remote, amount, posted=None, description='', pending=False, transacted=None):
    return dict(remote_id=remote, posted_on=posted, transacted_on=transacted, amount=amount, description=description, pending=pending)


def account(remote, transactions, conn_key='org1', name='Everyday checking', currency='USD', balance='1000.00', **kw):
    return dict(conn_key=conn_key, remote_id=remote, name=name, org_name=kw.get('org_name', 'Synthetic Bank'), currency=currency, balance=balance,
                available=kw.get('available', balance), balance_epoch=kw.get('balance_epoch', 1790000000), skipped=kw.get('skipped', []), transactions=transactions)


def T(conn, remote, t, key='org1'):
    return f'{conn}/{key}/{remote}/{t}'


def F(conn, remote, key='org1'):
    return f'{conn}/{key}/{remote}'


PINS = os.path.join(HERE, 'bankfeeds-random-500.expect.json')
REFUSALS = os.path.join(HERE, 'bankfeeds-refusals.expect.json')


def write(name, steps, **extra):
    refusals = {}
    if os.path.exists(REFUSALS):
        with open(REFUSALS) as handle:
            refusals = json.load(handle).get(name, {})
    for index, step in enumerate(steps):
        step.setdefault('expect_error', False)
        if step.get('expect_error') is True and str(index) in refusals:
            kind, message = refusals[str(index)]
            assert step.setdefault('expect_kind', kind) == kind and step.setdefault('expect_message', message) == message, (name, index, 'stale pin')
    unpinned = [i for i, step in enumerate(steps) if step.get('expect_error') is True and not (step.get('expect_kind') and step.get('expect_message'))]
    assert not unpinned or os.environ.get('FB_BANKFEEDS_UNPINNED') == '1', f'{name}: refusals without kind and message pins at steps {unpinned}: rerun pin_bankfeeds_refusals.py'
    data = dict(users=USERS, as_of=AS_OF, strict_outcomes=True, steps=steps, **extra)
    with open(os.path.join(OUT, f'{name}.json'), 'w') as handle:
        json.dump(data, handle, indent=1, ensure_ascii=False)
        handle.write('\n')


def connect_map():
    s = setup() + setup('other', 'Other synthetic', 'USD')
    s += [
        dict(op='bankfeeds_connect', company='co', ref='conn', host='Bridge.Example.TEST'),
        dict(op='bankfeeds_connect', company='co', ref='labelled', label='  Credit union  '),
        dict(op='bankfeeds_connect', company='co', ref='long', label='x' * 121, expect_error=True),
        dict(op='bankfeeds_connect', company='co', ref='blank', label='   '),
        dict(op='bankfeeds_connect', company='co', ref='typed', label=7, expect_error=True),
        dict(op='bankfeeds_connect', company='co', ref='outsider', actor='outsider', expect_error=True),
        dict(op='bankfeeds_mappable', company='co', result='mappable'),
        dict(op='bankfeeds_mappable', company='co', actor='outsider', result='outsider', expect_error=True),
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-10T15:00:00Z', result='first', pull=dict(accounts=[
            account('chk', [tx('t1', '-12.50', '2026-10-01', 'Coffee')]),
            account('cc', [tx('c1', '-40.00', '2026-10-02', 'Fuel')], name='Card', balance='-40.00'),
            account('eur', [tx('e1', '5.00', '2026-10-03', 'Euro')], name='Euro', currency='eur'),
            account('url', [], name='Points', currency='https://example.test/points', balance=None)])),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'chk'), account='bank'),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'cc'), account='card'),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'cc'), account='bank', expect_error=True),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'cc'), account='income', expect_error=True),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'cc'), account='ar', expect_error=True),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'cc'), account='uf', expect_error=True),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'cc'), account='old', expect_error=True),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'cc'), account='other.bank', expect_error=True),
        # 'eur' is not the company's USD: refused (multi-currency feeds are not supported)
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'eur'), account='savings', expect_error=True, expect_kind='ValidationError',
             expect_message='Synthetic Bank · Euro is in eur; this company keeps its books in USD. Multi-currency feeds are not supported.'),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'url'), account='savings', expect_error=True),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'cc'), account=True, expect_error=True),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'missing'), account='savings', expect_error=True),
        dict(op='bankfeeds_map', company='other', feed=F('conn', 'chk'), account='other.bank', expect_error=True),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'cc'), account=None),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'cc'), account=None),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'chk'), account='savings'),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'cc'), account='bank'),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'cc'), account='card'),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'chk'), account='bank'),
        dict(op='bankfeeds_mappable', company='co', result='after'),
        dict(op='bankfeeds_forget', company='co', connection='labelled'),
        dict(op='bankfeeds_forget', company='co', connection='labelled'),
        dict(op='bankfeeds_forget', company='other', connection='conn', expect_error=True),
        dict(op='bankfeeds_forget', company='co', connection='missing', expect_error=True),
        dict(op='bankfeeds_sync', company='co', connection='labelled', now='2026-10-11T15:00:00Z', pull=dict(accounts=[]), expect_error=True),
        dict(op='bankfeeds_sync', company='other', connection='conn', now='2026-10-11T15:00:00Z', pull=dict(accounts=[]), expect_error=True),
        dict(op='bankfeeds_forget', company='co', connection='conn'),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'chk'), account=None),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'chk'), account='bank'),
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-30T15:00:00Z', force=True, pull=dict(accounts=[]), expect_error=True),
    ]
    write('bankfeeds-connect-map', s)


def pulls():
    s = setup() + [dict(op='bankfeeds_connect', company='co', ref='conn')]
    first = [tx('p1', '-20.00', None, 'Pending grocery', True, '2026-10-05'), tx('p2', '-7.25', None, 'Pending parking', True),
             tx('t1', '-12.50', '2026-10-01', 'Coffee CHECK #1042'), tx('t2', '250.00', '2026-10-02', 'Payroll deposit'),
             tx('p3', '-3.00', None, 'Pending snack', True), tx('t3', '-1.00', '2026-10-03', '')]
    s += [
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-06T15:00:00Z', result='first',
             pull=dict(accounts=[account('chk', first, skipped=['transaction x9: amount is not a number'])], skipped=['account entry is not an object'],
                       errors=['Synthetic Bank needs a login refresh.'])),
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-06T16:00:00Z', pull=dict(accounts=[]), expect_error=True),
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-06T20:30:00Z', pull=dict(accounts=[]), expect_error=True),
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-06T15:09:00Z', min_interval=600, pull=dict(accounts=[]), expect_error=True),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'chk'), account='bank'),
        # the review list: undated pending rows first (PostgreSQL sorts NULL posted_on first in DESC order)
        dict(op='bankfeeds_items', company='co', result='review-first'),
        dict(op='bankfeeds_items', company='co', actor='outsider', result='review-outsider', expect_error=True),
        dict(op='bankfeeds_suggest', company='co', tx=T('conn', 'chk', 't1'), result='check-number'),
        dict(op='bankfeeds_ignore', company='co', tx=T('conn', 'chk', 'p3')),
        dict(op='bankfeeds_ignore', company='co', tx=T('conn', 'chk', 'p2')),
        dict(op='bankfeeds_return', company='co', tx=T('conn', 'chk', 'p2')),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'p1'), values=dict(kind='JOURNAL', account_id='expense'), expect_error=True),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 't2'), ref='payroll', values=dict(kind='JOURNAL', account_id='income', payee_name='Employer')),
        # overlap: duplicates unchanged, p1 posted under the same id, t1 description edited, t2 decided keeps its values,
        # p2 withdrawn (pending, to review, inside the window); p3 is ignored and stays.
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-07T15:00:00Z', force=True, result='overlap', pull=dict(accounts=[account('chk', [
            tx('p1', '-20.00', '2026-10-06', 'Grocery', False, '2026-10-05'), tx('t1', '-12.50', '2026-10-01', 'Coffee CHECK #1042 adj'),
            tx('t2', '999.00', '2026-10-02', 'Changed payroll'), tx('t3', '-1.00', '2026-10-03', ''), tx('t4', '-2.00', '2026-10-07', 'New one'),
            tx('t1', '-12.50', '2026-10-01', 'Coffee CHECK #1042 adj')], balance='1215.25', available='1200.00', balance_epoch=1791400000)])),
        dict(op='bankfeeds_return', company='co', tx=T('conn', 'chk', 'p2'), expect_error=True),
        # a second account in the same connection, and a pull whose window filter is off (direct _store, window None)
        dict(op='bankfeeds_store', company='co', connection='conn', now='2026-10-08T15:00:00Z', result='store-none', pull=dict(accounts=[
            account('chk', [tx('p5', '-9.00', None, 'Pending A', True), tx('p6', '-9.00', None, 'Pending B', True)]),
            account('sav', [tx('s1', '0.01', '2026-10-08', 'Interest')], conn_key='', name='Savings', balance=None, available=None, balance_epoch=None)])),
        dict(op='bankfeeds_store', company='co', connection='conn', now='2026-10-08T16:00:00Z', result='store-later', window_start='2030-01-01T00:00:00Z',
             pull=dict(accounts=[account('chk', [])])),
        dict(op='bankfeeds_store', company='co', connection='conn', now='2026-10-08T17:00:00Z', result='store-drop', window_start=None,
             pull=dict(accounts=[account('chk', [tx('p6', '-9.00', None, 'Pending B', True)])])),
        dict(op='bankfeeds_store', company='co', connection='conn', now='2026-10-08T18:00:00Z', expect_error=True,
             pull=dict(accounts=[account('chk', [tx('bad', '-1.00', '2026-10-08', 'x' * 501)])])),
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-09T15:00:00Z', force=True, fail='SimpleFIN is not reachable right now (synthetic https://user:pw@bridge.example.test/x).', expect_error=True),
        # Python bugs not copied (L4-BANKFEEDS.md): the frozen redact keeps these credentials; v2 removes them
        # and the comparer holds the Rust side to credential absence (bankfeeds_shape::kept_credentials).
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-09T15:01:00Z', force=True, fail='failedhttps://user:pw@bridge.example.test/x', expect_error=True),
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-09T15:02:00Z', force=True, fail='failed<b></b>https://user:pw@bridge.example.test/x now', expect_error=True),
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-09T15:03:00Z', force=True, fail='bridge 9https://tok3n@bridge.example.test or ftp://user:pw@files.example.test/x', expect_error=True),
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-09T15:04:00Z', force=True, fail='again: SimpleFIN https://user:pw@bridge.example.test/x', expect_error=True),
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-09T15:05:00Z', result='after-failure', pull=dict(accounts=[account('chk', [tx('t5', '-4.00', '2026-10-09', 'After failure')])])),
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-09T15:06:00Z', scheduled=True, force=True, result='scheduled', pull=dict(accounts=[account('chk', [])])),
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-12-30T15:06:00Z', result='late', pull=dict(accounts=[account('chk', [tx('t6', '-4.00', '2026-12-29', 'Late')])])),
        dict(op='bankfeeds_items', company='co', result='review-last'),
    ]
    write('bankfeeds-pulls', s)


def rules():
    s = setup() + setup('other', 'Other synthetic') + [dict(op='bankfeeds_connect', company='co', ref='conn')]
    s += [
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-06T15:00:00Z', pull=dict(accounts=[account('chk', [
            tx('fee', '-5.00', '2026-10-01', 'MONTHLY SERVICE FEE'), tx('big', '-500.00', '2026-10-01', 'Monthly service fee large'),
            tx('in', '5.00', '2026-10-02', 'Service fee refund'), tx('none', '-1.00', '2026-10-02', 'Nothing matches'),
            tx('uni', '-3.00', '2026-10-03', 'STRASSE café'), tx('match', '-30.00', '2026-10-04', 'Already entered'),
            tx('dep', '10.25', '2026-10-04', 'Customer deposit')]), account('cc', [tx('ccfee', '-5.00', '2026-10-01', 'service fee card')], name='Card')])),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'chk'), account='bank'),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'cc'), account='card'),
        dict(op='bankfeeds_rule_accounts', company='co', result='rule-accounts'),
        dict(op='bankfeeds_save_rule', company='co', ref='r1', values=dict(name='Fees', match_text='  service fee ', sign='OUT', amount_max='10', account_id='fees', payee_name='Bank')),
        dict(op='bankfeeds_save_rule', company='co', ref='r2', values=dict(name='Any fee', match_text='FEE', account_id='expense')),
        dict(op='bankfeeds_save_rule', company='co', ref='r3', values=dict(name='Refunds', match_text='refund', sign='IN', amount_min='1.00', amount_max='1.00', account_id='income')),
        dict(op='bankfeeds_save_rule', company='co', ref='r4', values=dict(name='Unicode', match_text='straße', account_id='expense', active='false')),
        dict(op='bankfeeds_save_rule', company='co', ref='bad1', values=dict(name='Blank', match_text='   ', account_id='fees'), expect_error=True),
        dict(op='bankfeeds_save_rule', company='co', ref='bad2', values=dict(name='', match_text='x', account_id='fees'), expect_error=True),
        dict(op='bankfeeds_save_rule', company='co', ref='bad3', values=dict(name='Sign', match_text='x', sign='BOTH', account_id='fees'), expect_error=True),
        dict(op='bankfeeds_save_rule', company='co', ref='bad4', values=dict(name='Neg', match_text='x', amount_min='-1', account_id='fees'), expect_error=True),
        dict(op='bankfeeds_save_rule', company='co', ref='bad5', values=dict(name='Range', match_text='x', amount_min='5', amount_max='4', account_id='fees'), expect_error=True),
        dict(op='bankfeeds_save_rule', company='co', ref='bad6', values=dict(name='Control', match_text='x', account_id='ar'), expect_error=True),
        dict(op='bankfeeds_save_rule', company='co', ref='bad7', values=dict(name='Inactive', match_text='x', account_id='old'), expect_error=True),
        dict(op='bankfeeds_save_rule', company='co', ref='bad8', values=dict(name='Other', match_text='x', account_id='other.fees'), expect_error=True),
        dict(op='bankfeeds_save_rule', company='co', ref='bad9', values=dict(name='Money', match_text='x', amount_min='abc', account_id='fees'), expect_error=True),
        dict(op='bankfeeds_save_rule', company='co', ref='bad10', values=dict(name='Long', match_text='x' * 201, account_id='fees'), expect_error=True),
        dict(op='bankfeeds_save_rule', company='co', rule='missing', values=dict(name='Missing', match_text='x', account_id='fees'), expect_error=True),
        dict(op='bankfeeds_save_rule', company='other', rule='r1', values=dict(name='Cross', match_text='x', account_id='other.fees'), expect_error=True),
        dict(op='bankfeeds_suggest', company='co', tx=T('conn', 'chk', 'fee'), result='fee'),
        dict(op='bankfeeds_suggest', company='co', tx=T('conn', 'chk', 'big'), result='big'),
        dict(op='bankfeeds_suggest', company='co', tx=T('conn', 'chk', 'in'), result='in-miss'),
        dict(op='bankfeeds_suggest', company='co', tx=T('conn', 'chk', 'none'), result='none'),
        dict(op='bankfeeds_suggest', company='co', tx=T('conn', 'chk', 'uni'), result='unicode-active-text-false'),
        dict(op='bankfeeds_suggest', company='co', tx=T('conn', 'cc', 'ccfee'), result='card-journal'),
        dict(op='bankfeeds_suggest', company='other', tx=T('conn', 'chk', 'fee'), result='other-company', expect_error=True),
        dict(op='bankfeeds_save_rule', company='co', rule='r3', values=dict(name='Refunds', match_text='refund', sign='IN', amount_min='5.00', amount_max='', account_id='income', payee_name='Back')),
        dict(op='bankfeeds_suggest', company='co', tx=T('conn', 'chk', 'in'), result='in-hit'),
        dict(op='bankfeeds_move_rule', company='co', rule='r2', direction='up'),
        dict(op='bankfeeds_move_rule', company='co', rule='r2', direction='up'),
        dict(op='bankfeeds_move_rule', company='co', rule='r4', direction='down'),
        dict(op='bankfeeds_move_rule', company='co', rule='r4', direction='sideways', expect_error=True),
        dict(op='bankfeeds_move_rule', company='co', rule='missing', direction='up', expect_error=True),
        dict(op='bankfeeds_move_rule', company='other', rule='r1', direction='up', expect_error=True),
        dict(op='bankfeeds_suggest', company='co', tx=T('conn', 'chk', 'fee'), result='fee-after-move'),
        dict(op='bankfeeds_rule_active', company='co', rule='r2', active=False),
        dict(op='bankfeeds_rule_active', company='co', rule='r2', active=False),
        dict(op='bankfeeds_rule_active', company='co', rule='r4', active=0),
        dict(op='bankfeeds_rule_active', company='co', rule='r4', active='yes'),
        dict(op='bankfeeds_rule_active', company='co', rule='missing', active=True, expect_error=True),
        dict(op='bankfeeds_suggest', company='co', tx=T('conn', 'chk', 'fee'), result='fee-after-off'),
        dict(op='bankfeeds_move_rule', company='co', rule='r1', direction='down'),
        dict(op='save_draft', company='co', ref='entered', date='2026-10-02', memo='Entered by hand', lines=[dict(account='expense', debit='30.00', credit='0'), dict(account='bank', debit='0', credit='30.00')]),
        dict(op='post_journal', company='co', journal='entered'),
        dict(op='save_draft', company='co', ref='entered2', date='2026-10-09', memo='Entered later', lines=[dict(account='expense', debit='30.00', credit='0'), dict(account='bank', debit='0', credit='30.00')]),
        dict(op='post_journal', company='co', journal='entered2'),
        dict(op='save_draft', company='co', ref='entered3', date='2026-10-10', memo='Too late', lines=[dict(account='expense', debit='30.00', credit='0'), dict(account='bank', debit='0', credit='30.00')]),
        dict(op='post_journal', company='co', journal='entered3'),
        dict(op='bankfeeds_suggest', company='co', tx=T('conn', 'chk', 'match'), result='match'),
    ] + sale('co', 'sale1', '10.25', '2026-09-20', 0x3101) + sale('co', 'sale2', '10.25', '2026-08-01', 0x3102) + [
        dict(op='bankfeeds_suggest', company='co', tx=T('conn', 'chk', 'dep'), result='deposit'),
    ]
    for n in range(196):                         # 4 rules above + 196 = the 200-rule cap; 'over' is the first refused
        s.append(dict(op='bankfeeds_save_rule', company='co', ref=f'fill{n}', values=dict(name=f'Fill {n}', match_text=f'zz{n}', account_id='fees')))
    s.append(dict(op='bankfeeds_save_rule', company='co', ref='over', values=dict(name='Over', match_text='over', account_id='fees'), expect_error=True,
                  expect_kind='ValidationError', expect_message='A company has at most 200 rules.'))
    s.append(dict(op='bankfeeds_save_rule', company='co', rule='fill5', values=dict(name='Edited at the cap', match_text='zz5', account_id='expense')))
    write('bankfeeds-rules', s)


def accept():
    s = setup() + setup('other', 'Other synthetic') + [dict(op='bankfeeds_connect', company='co', ref='conn'), dict(op='bankfeeds_connect', company='other', ref='oconn')]
    s += sale('co', 'sale1', '10.25', '2026-10-01', 0x4101) + sale('co', 'sale2', '4.75', '2026-10-01', 0x4102) + sale('co', 'sale3', '15.00', '2026-10-01', 0x4103)
    chk = [tx('c1', '-12.50', '2026-10-05', 'Utility bill'), tx('c2', '-60.00', '2026-10-05', 'CHECK 1042'), tx('c3', '-8.00', '2026-10-05', 'Vendor paid'),
           tx('c4', '-9.00', '2026-10-05', ''), tx('d1', '15.00', '2026-10-06', 'Deposit of two'), tx('d2', '15.00', '2026-10-06', 'Wrong total'),
           tx('x1', '-100.00', '2026-10-07', 'To savings'), tx('x2', '40.00', '2026-10-07', 'From savings'),
           tx('j1', '-3.10', '2026-10-08', 'Service charge'), tx('j2', '2.00', '2026-10-08', 'Interest'), tx('m1', '-30.00', '2026-10-09', 'Matched'),
           tx('m2', '-30.00', '2026-10-09', 'Matched twice'), tx('pend', '-1.00', None, 'Pending', True), tx('nodate', '-1.00', None, 'No date', False)]
    s += [
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-10T15:00:00Z', pull=dict(accounts=[account('chk', chk),
             account('cc', [tx('cc1', '-25.00', '2026-10-05', 'Card purchase'), tx('cc2', '25.00', '2026-10-06', 'Card payment')], name='Card'),
             account('unm', [tx('u1', '-1.00', '2026-10-05', 'Unmapped')], name='Unmapped')])),
        dict(op='bankfeeds_sync', company='other', connection='oconn', now='2026-10-10T15:00:00Z', pull=dict(accounts=[account('chk', [tx('o1', '-5.00', '2026-10-05', 'Other')])])),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'chk'), account='bank'),
        dict(op='bankfeeds_map', company='co', feed=F('conn', 'cc'), account='card'),
        dict(op='bankfeeds_map', company='other', feed=F('oconn', 'chk'), account='other.bank'),
        # checks: default EFT number, CHECK 1234 number, vendor payee, empty description; retries return the item
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'c1'), ref='chk1', values=dict(kind='CHECK', account_id='expense')),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'c1'), ref='chk1', values=dict(kind='CHECK', account_id='fees')),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'c2'), ref='chk2', values=dict(kind='CHECK', account_id='expense', payee_name=' Landlord ')),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'c3'), ref='chk3', values=dict(kind='CHECK', account_id='expense', vendor_id='vendor', check_number='777')),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'c4'), ref='chk4', values=dict(kind='CHECK', account_id='expense', check_number='1042'), expect_error=True),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'c4'), ref='chk4', values=dict(kind='CHECK', account_id='expense')),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'd1'), values=dict(kind='CHECK', account_id='expense'), expect_error=True),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'cc', 'cc1'), values=dict(kind='CHECK', account_id='expense'), expect_error=True),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'j1'), values=dict(kind='CHECK', account_id=None), expect_error=True),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'j1'), values=dict(kind='CHECK', account_id='expense', check_number='x' * 21), expect_error=True),
        # deposits
        dict(op='bankfeeds_suggest', company='co', tx=T('conn', 'chk', 'd1'), result='d1'),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'd2'), values=dict(kind='DEPOSIT', receipt_ids=['sale1']), expect_error=True),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'd2'), values=dict(kind='DEPOSIT', receipt_ids=[]), expect_error=True),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'c1'), values=dict(kind='DEPOSIT', receipt_ids=['sale3']), expect_error=False),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'x1'), values=dict(kind='DEPOSIT', receipt_ids=['sale3']), expect_error=True),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'd1'), ref='dep1', values=dict(kind='DEPOSIT', receipt_ids=['sale1', 'sale2'])),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'd2'), ref='dep2', values=dict(kind='DEPOSIT', receipt_ids=['sale3'])),
        # transfers
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'x1'), ref='trf1', values=dict(kind='TRANSFER', transfer_account_id='savings')),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'x2'), ref='trf2', values=dict(kind='TRANSFER', transfer_account_id='savings')),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'cc', 'cc2'), values=dict(kind='TRANSFER', transfer_account_id='card'), expect_error=True),
        # a card payment is not a transfer: the transfers module moves money between two designated banks only
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'cc', 'cc2'), ref='trf3', values=dict(kind='TRANSFER', transfer_account_id='bank'), expect_error=True,
             expect_kind='ValidationError', expect_message='210 · Card is not an active designated bank account (Accounting setup). Transfers move money between two designated banks.'),
        # journal entries
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'j1'), values=dict(kind='JOURNAL', account_id='bank'), expect_error=True),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'j1'), values=dict(kind='JOURNAL', account_id='fees', payee_name='p' * 121), expect_error=True),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'j1'), ref='je1', values=dict(kind='JOURNAL', account_id='fees', payee_name='Bank')),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'j2'), ref='je2', values=dict(kind='JOURNAL', account_id='income')),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'cc', 'cc1'), ref='je3', values=dict(kind='JOURNAL', account_id='expense')),
        # matches
        dict(op='save_draft', company='co', ref='hand', date='2026-10-08', memo='By hand', lines=[dict(account='expense', debit='30.00', credit='0'), dict(account='bank', debit='0', credit='30.00')]),
        dict(op='post_journal', company='co', journal='hand'),
        dict(op='bankfeeds_suggest', company='co', tx=T('conn', 'chk', 'm1'), result='m1'),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'm1'), values=dict(kind='MATCH', journal_line_id='hand:1'), expect_error=True),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'm1'), values=dict(kind='MATCH', journal_line_id='hand:2')),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'm2'), values=dict(kind='MATCH', journal_line_id='hand:2'), expect_error=True),
        dict(op='bankfeeds_suggest', company='co', tx=T('conn', 'chk', 'm2'), result='m2'),
        # refusals
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'pend'), values=dict(kind='JOURNAL', account_id='fees'), expect_error=True),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'nodate'), values=dict(kind='JOURNAL', account_id='fees'), expect_error=True),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'unm', 'u1'), values=dict(kind='JOURNAL', account_id='fees'), expect_error=True),
        dict(op='bankfeeds_accept', company='co', tx=T('oconn', 'chk', 'o1'), values=dict(kind='JOURNAL', account_id='fees'), expect_error=True),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'm2'), values=dict(kind='OTHER'), expect_error=True),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'm2'), values=dict(kind='JOURNAL', account_id='fees'), actor='outsider', expect_error=True),
        dict(op='bankfeeds_accept', company='other', tx=T('oconn', 'chk', 'o1'), ref='oje', values=dict(kind='JOURNAL', account_id='other.fees')),
        # ignore / return
        dict(op='bankfeeds_ignore', company='co', tx=T('conn', 'chk', 'nodate')),
        dict(op='bankfeeds_ignore', company='co', tx=T('conn', 'chk', 'nodate')),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'nodate'), values=dict(kind='JOURNAL', account_id='fees'), expect_error=True),
        dict(op='bankfeeds_return', company='co', tx=T('conn', 'chk', 'nodate')),
        dict(op='bankfeeds_return', company='co', tx=T('conn', 'chk', 'nodate')),
        dict(op='bankfeeds_ignore', company='co', tx=T('conn', 'chk', 'j1'), expect_error=True),
        dict(op='bankfeeds_ignore', company='other', tx=T('conn', 'chk', 'm2'), expect_error=True),
        dict(op='bankfeeds_return', company='co', tx=T('conn', 'chk', 'm1')),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'm2'), values=dict(kind='MATCH', journal_line_id='hand:2')),
        # return-to-review only after the document is voided / reversed; then accept again with a fresh request key
        dict(op='bankfeeds_return', company='co', tx=T('conn', 'chk', 'j1'), expect_error=True),
        dict(op='bankfeeds_return', company='co', tx=T('conn', 'chk', 'c1'), expect_error=True),
        dict(op='bankfeeds_undo', company='co', tx=T('conn', 'chk', 'j1'), date='2026-10-12', reason='Wrong category'),
        dict(op='bankfeeds_undo', company='co', tx=T('conn', 'chk', 'j1'), date='2026-10-12', reason='Twice', expect_error=True),
        dict(op='bankfeeds_return', company='co', tx=T('conn', 'chk', 'j1')),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'j1'), ref='je1b', values=dict(kind='JOURNAL', account_id='fees', payee_name='Bank')),
        dict(op='bankfeeds_undo', company='co', tx=T('conn', 'chk', 'c1'), date='2026-10-12', reason='Duplicate'),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'c1'), values=dict(kind='CHECK', account_id='expense')),
        dict(op='bankfeeds_return', company='co', tx=T('conn', 'chk', 'c1')),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'c1'), ref='chk1b', values=dict(kind='CHECK', account_id='expense')),
        dict(op='bankfeeds_undo', company='co', tx=T('conn', 'chk', 'x1'), date='2026-10-12', reason='Wrong account'),
        dict(op='bankfeeds_return', company='co', tx=T('conn', 'chk', 'x1')),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'x1'), values=dict(kind='JOURNAL', account_id='equity')),
        dict(op='bankfeeds_undo', company='co', tx=T('conn', 'chk', 'd2'), date='2026-10-12', reason='Bounced'),
        dict(op='bankfeeds_undo', company='co', tx=T('conn', 'chk', 'm1'), date='2026-10-12', reason='Matched', expect_error=True),
        dict(op='bankfeeds_return', company='co', tx=T('conn', 'chk', 'd2')),
        dict(op='bankfeeds_ignore', company='co', tx=T('conn', 'chk', 'd2')),
        dict(op='bankfeeds_accept', company='co', tx=T('conn', 'chk', 'd1'), values=dict(kind='JOURNAL', account_id='income')),
        dict(op='bankfeeds_return', company='co', tx=T('conn', 'chk', 'm2')),
        dict(op='bankfeeds_ignore', company='co', tx=T('conn', 'chk', 'm2')),
        dict(op='bankfeeds_return', company='co', tx=T('conn', 'chk', 'm2')),
        dict(op='bankfeeds_suggest', company='co', tx=T('conn', 'chk', 'm2'), result='m2-after'),
        dict(op='bankfeeds_suggest', company='co', tx=T('conn', 'chk', 'c4'), result='c4'),
    ]
    write('bankfeeds-accept', s)


def random_500(seed=20261030, count=500):
    rng = random.Random(seed)
    s = setup() + [dict(op='bankfeeds_connect', company='co', ref='conn'), dict(op='bankfeeds_connect', company='co', ref='conn2', label='Second')]
    s += sale('co', 'sale1', '10.25', '2026-10-01', 0x5101) + sale('co', 'sale2', '20.00', '2026-10-02', 0x5102) + sale('co', 'sale3', '30.00', '2026-10-03', 0x5103)
    s += [dict(op='save_draft', company='co', ref='hand', date='2026-10-08', memo='By hand', lines=[dict(account='expense', debit='30.00', credit='0'), dict(account='bank', debit='0', credit='30.00')]),
          dict(op='post_journal', company='co', journal='hand')]
    feeds = [('conn', 'chk'), ('conn', 'cc'), ('conn2', 'sav')]
    known = {f: [] for f in feeds}
    decided = []                                 # items the generator expects to be ignored or accepted
    rules, minute = [], 0
    first = [account(remote, [tx(f'seed{n}', '-30.00' if n % 2 else '10.25', '2026-10-0' + str(n % 9 + 1), 'Seed ' + str(n)) for n in range(6)], name=remote.upper())
             for c, remote in feeds if c == 'conn']
    s += [dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-09T12:00:00Z', pull=dict(accounts=first)),
          dict(op='bankfeeds_sync', company='co', connection='conn2', now='2026-10-09T12:00:00Z', pull=dict(accounts=[account('sav', [tx('seed', '40.00', '2026-10-05', 'Seed')], name='SAV')])),
          dict(op='bankfeeds_map', company='co', feed=F('conn', 'chk'), account='bank'),
          dict(op='bankfeeds_map', company='co', feed=F('conn', 'cc'), account='card'),
          dict(op='bankfeeds_map', company='co', feed=F('conn2', 'sav'), account='savings')]
    for c, remote in feeds:
        known[(c, remote)] = [f'seed{n}' for n in range(6)] if c == 'conn' else ['seed']
    amounts = ['-12.50', '-30.00', '10.25', '20.00', '-5.00', '-100.00', '40.00', '30.00', '-0.01', '1234.56']
    words = ['Coffee', 'SERVICE FEE', 'Payroll', 'CHECK 1042', 'Refund', 'Transfer', 'Fuel', '', 'Café straße', 'Rent']
    targets = ['bank', 'savings', 'card', 'income', 'ar', 'old', None]
    base, i = len(s), 0
    while len(s) - base < count:
        i += 1
        roll = rng.random()
        if roll < 0.18:
            conn = rng.choice(['conn', 'conn2'])
            minute += rng.choice([1, 30, 400])
            now = f'2026-10-{10 + minute // 1440:02d}T{(minute // 60) % 24:02d}:{minute % 60:02d}:00Z'
            accts = []
            for c, remote in feeds:
                if c != conn or rng.random() < 0.2:
                    continue
                txs, before, fresh = [], list(known[(c, remote)]), []
                for _ in range(rng.randint(0, 4)):
                    if before and rng.random() < 0.4:
                        rid = rng.choice(before)
                    else:
                        rid = f'r{i}x{len(txs)}'
                        fresh.append(rid)
                    pending = rng.random() < 0.25
                    day = f'2026-10-{rng.randint(1, 9):02d}'
                    txs.append(tx(rid, rng.choice(amounts), None if pending else day, rng.choice(words), pending, day if rng.random() < 0.5 else None))
                accts.append(account(remote, txs, name=remote.upper(), balance=rng.choice(['10.00', None])))
            step = dict(op=rng.choice(['bankfeeds_sync', 'bankfeeds_sync', 'bankfeeds_store']), company='co', connection=conn, now=now, pull=dict(accounts=accts))
            stored = True
            if step['op'] == 'bankfeeds_sync':
                step.update(force=rng.random() < 0.85, min_interval=rng.choice([600, 21600]))
                if rng.random() < 0.08:
                    step['fail'] = 'Synthetic provider outage.'
                    stored = False
            else:
                step['window_start'] = rng.choice([None, '2026-01-01T00:00:00Z', '2030-01-01T00:00:00Z'])
            if stored:
                for a in accts:
                    known[(conn, a['remote_id'])].extend(t['remote_id'] for t in a['transactions'] if t['remote_id'] not in known[(conn, a['remote_id'])])
            s.append(step)
        elif roll < 0.26:
            c, remote = rng.choice(feeds)
            s.append(dict(op='bankfeeds_map', company='co', feed=F(c, remote), account=rng.choice(targets)))
            if rng.random() < 0.7:                   # usually put the working mapping back
                s.append(dict(op='bankfeeds_map', company='co', feed=F(c, remote), account={'chk': 'bank', 'cc': 'card', 'sav': 'savings'}[remote]))
        elif roll < 0.36:
            ref = f'rule{i}'
            values = dict(name=f'Rule {i}', match_text=rng.choice(['fee', 'COFFEE', 'refund', 'check', 'straße', ' ']), sign=rng.choice(['ANY', 'IN', 'OUT', 'X']),
                          amount_min=rng.choice([None, '', '1', '-1', '50']), amount_max=rng.choice([None, '20', '0.5']), account_id=rng.choice(['fees', 'expense', 'income', 'ar']),
                          payee_name=rng.choice(['', 'Payee']), active=rng.choice([True, False, 'false']))
            if rules and rng.random() < 0.3:
                s.append(dict(op='bankfeeds_save_rule', company='co', rule=rng.choice(rules), values=values))
            else:
                s.append(dict(op='bankfeeds_save_rule', company='co', ref=ref, values=values))
                rules.append(ref)
        elif roll < 0.40 and rules:
            if rng.random() < 0.5:
                s.append(dict(op='bankfeeds_move_rule', company='co', rule=rng.choice(rules), direction=rng.choice(['up', 'down'])))
            else:
                s.append(dict(op='bankfeeds_rule_active', company='co', rule=rng.choice(rules), active=rng.choice([True, False])))
        else:
            pool = [(f, r) for f, ids in known.items() for r in ids]
            if not pool:
                continue
            (c, remote), rid = rng.choice(pool)
            t = T(c, remote, rid)
            if decided and rng.random() < 0.35:
                t = rng.choice(decided)
            pick = rng.random()
            if pick < 0.02:
                s.append(dict(op='bankfeeds_items', company='co', result=f'items{i}'))
            elif pick < 0.12:
                s.append(dict(op='bankfeeds_suggest', company='co', tx=t, result=f'suggest{i}'))
            elif pick < 0.22:
                s.append(dict(op='bankfeeds_ignore', company='co', tx=t))
                decided.append(t)
            elif pick < 0.34:
                s.append(dict(op='bankfeeds_return', company='co', tx=t))
            elif pick < 0.42:
                s.append(dict(op='bankfeeds_undo', company='co', tx=t, date='2026-10-20', reason='Random undo'))
            else:
                kind = rng.choice(['CHECK', 'DEPOSIT', 'TRANSFER', 'JOURNAL', 'JOURNAL', 'MATCH'])
                values = dict(kind=kind)
                if kind == 'CHECK':
                    values.update(account_id=rng.choice(['expense', 'fees']), payee_name=rng.choice(['', 'Payee']), check_number=rng.choice(['', '', f'{5000 + i}']))
                    if rng.random() < 0.2:
                        values['vendor_id'] = 'vendor'
                elif kind == 'DEPOSIT':
                    values['receipt_ids'] = rng.sample(['sale1', 'sale2', 'sale3'], rng.randint(1, 2))
                elif kind == 'TRANSFER':
                    values['transfer_account_id'] = rng.choice(['savings', 'bank', 'card'])
                elif kind == 'JOURNAL':
                    values.update(account_id=rng.choice(['fees', 'income', 'bank']), payee_name=rng.choice(['', 'Payee']))
                else:
                    values['journal_line_id'] = rng.choice(['hand:1', 'hand:2'])
                s.append(dict(op='bankfeeds_accept', company='co', tx=t, ref=f'acc{i}', values=values))
                decided.append(t)
    s.append(dict(op='bankfeeds_items', company='co', result='items-final'))
    if os.path.exists(PINS):
        with open(PINS) as handle:
            pins = json.load(handle)
        assert len(pins) == len(s) and all(p['op'] == step['op'] for p, step in zip(pins, s)), 'pins are stale: rerun pin_bankfeeds_expectations.py'
        for p, step in zip(pins, s):
            if 'expect_error' not in step:
                step.update({k: p[k] for k in ('expect_error', 'expect_kind', 'expect_message') if k in p})
    write('bankfeeds-random-500', s)


def identity():
    """FIX5: provider ids that differ only in URL userinfo stay two rows on both sides. Python stores the raw
    ids; v2 stores them redacted (alike) and keeps them apart by identity keys. Each such account and
    transaction carries a scenario `ref`, so both runners name rows by the raw id they stand for (runner-
    recorded provenance) and no name carries a credential; the comparer redacts Python's raw ids the v2 way."""
    url = lambda user, path: f'https://{user}:pw-{user}@ids.example.test/{path}'
    A, B = url('first', 'acct'), url('second', 'acct')
    tx_a = dict(tx(url('first', 'tx'), '25.00', '2026-10-05', 'Deposit first'), ref='tx-a')
    tx_b = dict(tx(url('second', 'tx'), '40.00', '2026-10-05', 'Deposit second'), ref='tx-b')
    hold = lambda user: dict(tx(url(user, 'hold'), '-3.00', None, 'Pending hold', True, '2026-10-05'), ref=f'hold-{user}')
    pull = lambda holds: dict(accounts=[dict(account(A, [tx_a] + holds), ref='acct-a'),
                                        dict(account(B, [tx_b], name='Second checking'), ref='acct-b')])
    s = setup() + [dict(op='bankfeeds_connect', company='co', ref='conn')]
    s += [
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-06T15:00:00Z', result='first', pull=pull([hold('first')])),
        dict(op='bankfeeds_map', company='co', feed='conn/~/acct-a', account='bank'),
        # the second account never inherits the first one's mapping
        dict(op='bankfeeds_accept', company='co', tx='conn/~/acct-b/tx-b', values=dict(kind='JOURNAL', account_id='income'), expect_error=True,
             expect_kind='ValidationError', expect_message='Map Synthetic Bank · Second checking to a ledger account before accepting its transactions.'),
        dict(op='bankfeeds_map', company='co', feed='conn/~/acct-b', account='savings'),
        dict(op='bankfeeds_accept', company='co', tx='conn/~/acct-a/tx-a', ref='acc-a', values=dict(kind='JOURNAL', account_id='income')),
        dict(op='bankfeeds_items', company='co', result='review-after-first'),
        # the twin hold replaces the first: withdrawn by its own raw id, the new one is new
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-07T15:00:00Z', force=True, result='swap', pull=pull([hold('second')])),
        dict(op='bankfeeds_accept', company='co', tx='conn/~/acct-b/tx-b', ref='acc-b', values=dict(kind='JOURNAL', account_id='income')),
        dict(op='bankfeeds_sync', company='co', connection='conn', now='2026-10-08T15:00:00Z', force=True, result='again', pull=pull([hold('second')])),
        dict(op='bankfeeds_items', company='co', result='review-last'),
    ]
    write('bankfeeds-identity', s)


if __name__ == '__main__':
    connect_map()
    pulls()
    rules()
    accept()
    random_500()
    identity()
