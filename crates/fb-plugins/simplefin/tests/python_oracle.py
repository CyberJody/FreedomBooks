"""Regenerate synthetic golden cases by executing the frozen Python client's pure functions.

No Django setup, DB, settings files, credentials, DNS, or HTTP. The only Django import
in client.py is a settings reference; stub that import instead of loading a company.
Run: python3 tests/python_oracle.py /path/to/frozen/bankfeeds/client.py
"""
import importlib.util
import json
from pathlib import Path
import random
import sys
import types

# The frozen reference is read-only, including incidental import cache files.
sys.dont_write_bytecode = True
reference = Path(sys.argv[1]).resolve()
conf = types.ModuleType('django.conf')
setattr(conf, 'settings', types.SimpleNamespace())
sys.modules['django'] = types.ModuleType('django')
sys.modules['django.conf'] = conf
spec = importlib.util.spec_from_file_location('frozen_bankfeeds_client', reference)
assert spec is not None and spec.loader is not None
client = importlib.util.module_from_spec(spec)
spec.loader.exec_module(client)

rng = random.Random(245)
values = [None, True, False, '', '.', 'NaN', 'sNaN', '+Infinity', '-inf', 'abc',
          'NaNbad', 'sNaNbad', '1e', '1.2.3', '1e2e3', '1e2', '0.125', '-0.125',
          '2.675', '1.005', '99999999999999.99', '99999999999999.991',
          '-99999999999999.991', '100000000000000', '1_000.0', '+.005', '-.005',
          '0e-100', '-0.0000000', '0.0000009', ' 2.56 ', 0, 1, -2, 1.005]
for _ in range(512):
    sign = rng.choice(['', '-', '+'])
    whole = rng.randrange(0, 100000000000000)
    fraction = rng.randrange(0, 1000000)
    exponent = rng.randrange(-20, 8)
    values.append(f'{sign}{whole}.{fraction:06}e{exponent:+}')
money = []
for value in values:
    amount, why = client.money(value)
    money.append({'input': value, 'cents': int(amount * 100) if amount is not None else None, 'error': why})

epoch = [{'input': value, 'output': client.epoch(value)} for value in
         [None, True, False, 0, -1, 1, 4102444800, 4102444801, '1', '1.9',
          1.9, 4102444800.9, ' 978366153 ', '978_366_153', 'NaN', {}, []]]

def make_account(i):
    return {'id': f'A{i}', 'name': f'Synthetic {i}', 'currency': 'USD',
            'balance': f'{i}.125', 'balance-date': 1792800000,
            'org': {'id': 'old-connection', 'name': 'Old bank', 'domain': 'bank.example'},
            'transactions': [{'id': f'T{i}', 'posted': 1792800000, 'transacted_at': 1792713600,
                              'amount': f'-{i}.125', 'description': '<b>Purchase</b>', 'pending': False}]}

accounts = []
for i in range(32):
    raw = make_account(i)
    if i % 3 == 0:
        raw['conn_id'] = 'new-connection'
    if i % 4 == 0:
        raw['available-balance'] = 'bad'
    if i % 5 == 0:
        raw['transactions'].append({'id': 'pending', 'amount': '0.005', 'posted': 0})
    if i % 6 == 0:
        raw['transactions'].extend([{'id': 'bad', 'amount': 'NaN'}, 'not an object'])
    data = {'accounts': [raw], 'connections': [{'conn_id': 'new-connection', 'org_name': 'New bank'}],
            'errlist': [{'code': 'bank.example', 'msg': 'Test warning', 'conn_id': 'new-connection'}]}
    parsed = client.parse_account_set(data)
    for account in parsed['accounts']:
        account['balance'] = int(account['balance'] * 100)
        account['available'] = int(account['available'] * 100) if account['available'] is not None else None
        account['skipped_count'] = len(account.pop('skipped'))
        for tx in account['transactions']:
            tx['amount'] = int(tx['amount'] * 100)
    parsed['skipped_count'] = len(parsed.pop('skipped'))
    accounts.append({'input': data, 'output': parsed})

result = {'source': 'python-reference-20261030 bankfeeds/client.py; synthetic inputs only',
          'money': money, 'epoch': epoch, 'account_sets': accounts}
out = Path(__file__).parent / 'fixtures' / 'python_client_cases.json'
out.parent.mkdir(exist_ok=True)
out.write_text(json.dumps(result, indent=2) + '\n')
print(f'Generated {len(money)} money, {len(epoch)} epoch, {len(accounts)} Account Set cases from frozen Python')
