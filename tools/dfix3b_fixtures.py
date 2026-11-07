"""Permanent reconcile validator boundary/order fixtures; no oracle normalization."""
from pathlib import Path
from copy import deepcopy
import json

root = Path(__file__).resolve().parents[1]
path = root / 'crates/fb-screendiff/screens.json'
screens = json.loads(path.read_text())
base_begin = deepcopy(screens['d7b-begin-invalid-amount'])
base_begin['post'].update(statement_date='2026-10-31', ending_balance='0.00')
base_undo = screens['dfix3-undo-truncated-revision']
base_tick = screens['d7b-screen-invalid-shown']
added = {}

def case(name, base, **post):
    spec = deepcopy(base)
    spec['post'].update(post)
    added['dfix3b-' + name.replace('_','-')] = spec

# Review's six exact undo regressions, plus UUID-before-revision-before-reason.
for i, (rev, reason) in enumerate([
    ('abc', 'Synthetic correction'), ('', 'Synthetic correction'),
    ('-1', 'Synthetic correction'), ('1000000001', 'Synthetic correction'),
    ('abc', ''), ('-1', ''),
]):
    case(f'undo-review-{i}', base_undo, revision=rev, reason=reason)
for key in ['abc', '', '-1', '1000000001']:
    case('undo-key-' + (key or 'blank'), base_undo,
         undo_key=key, revision='abc', reason='')
case('undo-reason-long', base_undo, revision='2', reason='x' * 501)
case('undo-stale-before-lookup', base_undo, revision='0', reason='Synthetic correction')
case('undo-reason-before-stale', base_undo, revision='0', reason='')

# BeginForm field cleaning runs before all service validation. Service order:
# request UUID, date, ending, charge, interest, optional revision, accounts/state.
for field in ['ending_balance', 'service_charge', 'interest']:
    for i, value in enumerate(['abc', '', '-1', '0', '0.001', '100000000000000.00',
                               'NaN', 'Infinity', '1.2.3', '-', '$', '1e400',
                               '12abc', ' 1_000.001 ']):
        # Blank optional fields, signed ending and zero ending are valid; pair
        # with an invalid later revision to prove they do not reject first.
        case(f'begin-{field}-{i}', base_begin, **{field: value, 'revision': 'abc'})
for field, values in [
    ('statement_date', ['abc', '', '1900-01-01', '9999-12-31']),
    ('request_key', ['abc', '', '-1', '1000000001', 'x' * 41]),
    ('revision', ['abc', '-1', '1000000001', '0.0', 'x' * 13]),
    ('service_account', ['abc', '', '-1', '1000000001', '{account:4000}']),
    ('interest_account', ['abc', '', '-1', '1000000001', '{account:1070}']),
]:
    for i, value in enumerate(values):
        extra = {field: value}
        if field == 'service_account': extra.update(service_charge='1.00')
        if field == 'interest_account': extra.update(interest='1.00')
        case(f'begin-{field}-{i}', base_begin, **extra)
for field, limit in [('ending_balance', 24), ('service_charge', 24), ('interest', 24),
                     ('service_account', 20), ('interest_account', 20)]:
    case('begin-length-' + field, base_begin, **{field: 'x' * (limit + 1)})
case('begin-form-before-service', base_begin, statement_date='', ending_balance='abc', request_key='abc')
case('begin-key-before-amount', base_begin, request_key='abc', ending_balance='abc')
case('begin-ending-before-charge', base_begin, ending_balance='abc', service_charge='abc')
case('begin-charge-before-interest', base_begin, service_charge='abc', interest='abc')
case('begin-interest-before-revision', base_begin, interest='abc', revision='abc')
case('begin-revision-before-account', base_begin, revision='abc', service_charge='1', service_account='')
case('begin-charge-before-current', base_begin, service_charge='1', service_account='')
card = deepcopy(base_begin)
card['path'] = '/c/{company}/reconcile/{account:2170}/begin/'
case('begin-charge-before-card-interest', card, service_charge='1', service_account='', interest='1')
case('begin-card-interest', card, interest='1')

# Screen view: action/id, shown grammar, tick grammar, revision, ticks, completion.
for action in ['save', 'reconcile']:
    for i, rev in enumerate(['abc', '', '-1', '1000000001', '0.0']):
        case(f'{action}-revision-{i}', base_tick, action=action, shown='', revision=rev)
    for i, value in enumerate(['abc', '-1', '1000000001', '1:abc', '1:', '1:-1',
                               '1:1000000001', '1:0000000000']):
        case(f'{action}-shown-{i}', base_tick, action=action, shown=value, revision='abc')
    for i, value in enumerate(['abc', '', '-1', '1000000000000000001']):
        case(f'{action}-tick-{i}', base_tick, action=action, shown='', tick=value, revision='abc')
    case(f'{action}-membership', base_tick, action=action, shown='',
         tick='{line:first:1070}', revision='1')
for i, key in enumerate(['abc', '', '-1', '1000000001']):
    case(f'finish-key-{i}', base_tick, action='reconcile', shown='', revision='1', completion_key=key)
case('finish-revision-before-key', base_tick, action='reconcile', shown='', revision='abc', completion_key='abc')
case('save-stale-noop', base_tick, action='reconcile', shown='', revision='0', completion_key='00000000-0000-0000-0000-000000df3b01')

# Report ChoiceFields do not strip whitespace; OPEN redirect precedes cleaning.
for field in ['view', 'output']:
    for i, value in enumerate(['abc', '-1', '1000000001', ' summary ' if field == 'view' else ' csv ']):
        spec = deepcopy(screens['d7b-report-invalid'])
        spec['path'] = spec['path'].split('?')[0] + f'?{field}={value}'
        added[f'dfix3b-report-{field}-{i}'] = spec
screens = {k:v for k,v in screens.items() if k not in ['dfix3b-save-shown-8','dfix3b-reconcile-shown-8']}
path.write_text(json.dumps({**screens, **added}, indent=2, ensure_ascii=False) + '\n')
print(f'Added {len(added)} reconcile boundary/order fixtures; {len(screens | added)} total')
