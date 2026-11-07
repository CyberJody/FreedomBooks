"""Regenerate synthetic receipt parity cases from the frozen Python pure parser.

No Django import, database, provider call or customer data. Loads only the named
pure functions/constants through AST; does not execute the reference module.
Run: python3 <this file> /path/to/frozen/assistant/receipts.py
"""
import ast
import copy
from datetime import date, timedelta
from decimal import Decimal, InvalidOperation
import json
from pathlib import Path
import re
import sys
import types
import unicodedata
from typing import Any

source = Path(sys.argv[1])
tree = ast.parse(source.read_text())
ns: dict[str, Any] = dict(date=date, timedelta=timedelta, Decimal=Decimal, InvalidOperation=InvalidOperation,
          json=json, re=re, unicodedata=unicodedata)
wanted = {'CENT','MONEY_MAX','QUANTITY_MAX','MAX_LINES','MAX_ADDRESS_LINES','TEXT_MAX',
          'EARLIEST_DATE','FUTURE_DAYS','TAX_LINE_DESCRIPTION','LINE_SCHEMA','SCHEMA','FIELDS','_NUMBER_JUNK'}
functions = {'_money','_quantity','_text','_date','parse_reply','_plain'}
for node in tree.body:
    if isinstance(node, ast.Assign) and len(node.targets)==1 and isinstance(node.targets[0],ast.Name):
        name = node.targets[0].id
        if name in wanted or name.startswith('NOTE_') or name == 'CURRENCY_MISMATCH':
            exec(compile(ast.Module(body=[node],type_ignores=[]),str(source),'exec'),ns)
    elif isinstance(node, ast.ClassDef) and node.name=='ReceiptError':
        exec(compile(ast.Module(body=[node],type_ignores=[]),str(source),'exec'),ns)
    elif isinstance(node, ast.FunctionDef) and node.name in functions:
        exec(compile(ast.Module(body=[node],type_ignores=[]),str(source),'exec'),ns)

base = {'vendor_name':'Demo Supply','vendor_address':['1 Example Way'], 'document_number':'SYN-1',
        'document_date':'2023-11-14','due_date':'2023-12-14','currency':'USD',
        'subtotal':'40.00','tax':'3.20','total':'43.20',
        'lines':[{'description':'Synthetic supplies','quantity':'2','unit_price':'20.00','amount':'40.00'}]}
cases = []
def add(name, data): cases.append((name,data))
def top(name,field,value):
    d=copy.deepcopy(base);d[field]=value;add(name,d)
def line(name,field,value):
    d=copy.deepcopy(base);d['lines'][0][field]=value;add(name,d)
add('consistent',copy.deepcopy(base))
for field in ['subtotal','tax','total']:
    for value in ['', '-', '-1', '(1.00)', 'NaN', 'Infinity', '1e100', '1e2', '$ 40.00', '1,000.01', '0.005', '0.015']:
        top(f'{field}-{value}',field,value)
for field in ['quantity','unit_price','amount']:
    for value in ['', '0', '-1', '2.50000', '0.00001', 'NaN', '1.005', '1e2']:
        line(f'line-{field}-{value}',field,value)
for value in ['', 'EUR', 'usd', 'USD extra']:
    top('currency-'+value,'currency',value)
for value in ['', '1989-12-31','1990-01-01','2023-02-29','2024-02-29','2024-11-14','2024-11-15','2023-11-14T00:00:00']:
    top('date-'+value,'document_date',value)
for value in ['', '2023-11-13','2023-11-14','2024-11-14','2024-11-15']:
    top('due-'+value,'due_date',value)
top('no-lines','lines',[])
top('no-lines-no-subtotal','lines',[]);cases[-1][1]['subtotal']=''
top('too-many-lines','lines',[copy.deepcopy(base['lines'][0]) for _ in range(50)])
top('controls-nfc','vendor_name',' Cafe\u0301\u0000\u202e \t Supply\n')
for field in base:
    d=copy.deepcopy(base);del d[field];add('missing-'+field,d)
d=copy.deepcopy(base);d['extra']='bad';add('extra-field',d)
line('line-mismatch','amount','39.00')
line('blank-description','description','\n\t')
line('line-half-even','quantity','0.025');cases[-1][1]['lines'][0].update(unit_price='0.20',amount='0.00')
line('large-product','quantity','99999999999999.9999');cases[-1][1]['lines'][0]['unit_price']='99999999999999.99'

# Precision-boundary regressions: every monetary field, with coefficients larger
# than rust_decimal's 96-bit mantissa and scales larger than its 28-place limit.
for field in ['subtotal', 'tax', 'total']:
    for value in [
        '0.00500000000000000000000000001',
        '0.01499999999999999999999999999',
        '1e-100', '5e-3', '15e-3',
        '0.' + '0' * 99 + '1',
        '0.005' + '0' * 100 + '1',
        '0.014' + '9' * 100,
        '99999999999999.99000000000000000000000000001',
        '99999999999999.98999999999999999999999999999',
        '-1e-100', '-0e-100',
    ]:
        top(f'precision-{field}-{value}', field, value)
for field in ['unit_price', 'amount']:
    for value in ['0.00500000000000000000000000001', '0.01499999999999999999999999999', '1e-100']:
        line(f'precision-{field}-{value}', field, value)

# The same lossy parser also fed quantities: values not exactly four-place
# quantities must trigger Python's fallback, never silently become valid ones.
for value in [
    '1.00000000000000000000000000001',
    '0.99999999999999999999999999999',
    '99999999999999.99990000000000000000000000001',
    '100000000000000000000000000000e-29',
    '1e-100',
]:
    line(f'precision-quantity-{value}', 'quantity', value)

def cents(s): return int(Decimal(s)*100)
def normalized(result):
    return dict(vendor_name=result['vendor_name'], vendor_address=result['vendor_address'],document_number=result['document_number'],
                document_date=result['document_date'] or None,due_date=result['due_date'] or None,currency=result['currency'],
                subtotal_cents=cents(result['subtotal']),tax_cents=cents(result['tax']),total_cents=cents(result['total']),
                lines=[dict(description=l['description'],quantity=int(Decimal(l['quantity'])*10000),unit_price_cents=cents(l['rate']),
                            printed_amount_cents=cents(l['amount']),product_cents=cents((Decimal(l['quantity'])*Decimal(l['rate'])).quantize(Decimal('0.01'))),
                            mismatch=bool(l.get('mismatch')),tax=bool(l.get('tax'))) for l in result['lines']])
out=[]
for name,data in cases:
    row={'name':name,'input':data}
    try:
        result=ns['parse_reply'](json.dumps(data),types.SimpleNamespace(currency='USD'),today=date(2023,11,14))
        row['expected']=normalized(result)
        row['note_count']=len(result['notes'])
    except Exception as exc:
        if not isinstance(exc, (ns['ReceiptError'], InvalidOperation)):
            raise
        row['error']=getattr(exc,'code',type(exc).__name__)
    out.append(row)
for filename, rows in [('receipt-reference-cases.json', out[:97]), ('receipt-precision-cases.json', out[97:])]:
    target = Path(__file__).with_name(filename)
    target.write_text(json.dumps(rows,ensure_ascii=False,indent=2)+'\n')
    print(f'{filename}: {len(rows)} synthetic Python reference cases generated; {sum("error" not in c for c in rows)} successful parses.')
