"""Append D8 evidence without changing any inherited screen or scenario step."""
from pathlib import Path
from collections import Counter
import json

ROOT = Path(__file__).resolve().parents[1]
screens_path = ROOT/'crates/fb-screendiff/screens.json'
base = json.loads((ROOT/'crates/fb-screendiff/scenarios/screens-dfix1.json').read_text())
reports = json.loads((ROOT/'crates/fb-harness/scenarios/reports-combined.json').read_text())
print('Combined report fixture:', len(reports['steps']), dict(Counter(s['op'] for s in reports['steps'])))
# All symbolic references are explicit scenario values, never arbitrary text. Prefix only values that
# equal a declared ref; transaction prose and numbers remain byte-for-byte the source fixture's values.
refs = {s['ref'] for s in reports['steps'] if 'ref' in s}
refs.update(u['username'] for u in reports['users'])
def translate(v):
    if isinstance(v, str):
        return 'd8-'+v if v in refs or v.split(':', 1)[0] in refs else v
    if isinstance(v, list):
        return [translate(x) for x in v]
    if isinstance(v, dict):
        literal = {'op','model','name','display_name','code','kind','detail_type','memo','description','expect_message','expect_kind','method','destination','reason','reference','check_number','state','number','tax_code','tax_mode','payee_name'}
        return {k: (x if k in literal else translate(x)) for k, x in v.items()}
    return v
steps = translate([s for s in reports['steps'] if not s.get('expect_error', False)])
for s in steps:
    if s.get('actor'):
        s['actor'] = 'owner'
    if s['op'] == 'create_company':
        s['name'] = 'D8 report books' if s['ref'] == 'd8-c' else 'D8 other report books'
    assert s.get('expect_error', False) is False, s
    s['expect_error'] = False
base['steps'].extend(steps)
base['steps'].extend([
    dict(op='create_company',ref='d8-other',name='D8 other report books',expect_error=False),
    dict(op='save_account',company='d8-other',ref='d8-other-income',code='400',name='Outside income',kind='INCOME',expect_error=False),
])
base['name'] = 'screens-d8'
# A single account spanning several GL pages. Same-date journals prove stable journal/line ordering;
# a changed draft saved after a later journal proves revision order cannot masquerade as creation order.
base['steps'].append(dict(op='save_account', company='d8-c', ref='d8-pages', code='090', name='Paging evidence', detail_type='OTHER_CURRENT_ASSET', expect_error=False))
base['steps'].extend([
    dict(op='save_account',company='d8-c',ref='d8-contra',code='170',name='Accumulated depreciation',detail_type='FIXED_ASSET',is_contra=True,expect_error=False),
    dict(op='save_draft',company='d8-c',ref='d8-prior',date='2025-12-31',memo='Prior-year income',lines=[dict(account='d8-bank',debit='12345.67'),dict(account='d8-income',credit='12345.67')],expect_error=False),
    dict(op='post_journal',company='d8-c',journal='d8-prior',expected_revision=1,expect_error=False),
    dict(op='save_draft',company='d8-c',ref='d8-depreciation',date='2026-10-31',memo='Depreciation',lines=[dict(account='d8-expense',debit='12.34'),dict(account='d8-contra',credit='12.34')],expect_error=False),
    dict(op='post_journal',company='d8-c',journal='d8-depreciation',expected_revision=1,expect_error=False),
])
for i,kind in enumerate(['FIXED_ASSET','OTHER_ASSET','CREDIT_CARD','OTHER_CURRENT_LIABILITY','LONG_TERM_LIABILITY','EQUITY','COST_OF_GOODS_SOLD','OTHER_INCOME','OTHER_EXPENSE']):
    ref='d8-kind-'+str(i)
    name='=Fixed "assets", tools' if kind=='FIXED_ASSET' else kind.replace('_',' ').title()
    base['steps'].append(dict(op='save_account',company='d8-c',ref=ref,code='9'+str(i),name=name,detail_type=kind,expect_error=False))
    base['steps'].append(dict(op='save_draft',company='d8-c',ref=ref+'-journal',date='2026-10-31',memo='Group layout evidence',lines=[dict(account=ref,debit='12.34'),dict(account='d8-pages',credit='12.34')],expect_error=False))
    base['steps'].append(dict(op='post_journal',company='d8-c',journal=ref+'-journal',expected_revision=1,expect_error=False))
for i in range(105):
    ref=f'd8-page-{i}'
    base['steps'].append(dict(op='save_draft', company='d8-c', ref=ref, date='2026-10-15', memo=f'Paging evidence {i:03}', lines=[dict(account='d8-pages', debit='0.33'), dict(account='d8-pages', debit='0.67'),dict(account='d8-bank', credit='1.00')], expect_error=False))
    base['steps'].append(dict(op='post_journal', company='d8-c', journal=ref, expected_revision=1, expect_error=False))
for ref,amount in [('d8-revision-old','1.00'),('d8-revision-new','2.00')]:
    base['steps'].append(dict(op='save_draft',company='d8-c',ref=ref,date='2026-10-14',entry_number=ref,memo=ref,lines=[dict(account='d8-bank',debit=amount),dict(account='d8-income',credit=amount)],expect_error=False))
base['steps'].extend([
    dict(op='save_draft',company='d8-c',journal='d8-revision-old',date='2026-10-14',entry_number='d8-revision-old',expected_revision=1,memo='Revised older journal',lines=[dict(account='d8-bank',debit='3.00'),dict(account='d8-income',credit='3.00')],expect_error=False),
    dict(op='post_journal',company='d8-c',journal='d8-revision-new',expected_revision=1,expect_error=False),
    dict(op='post_journal',company='d8-c',journal='d8-revision-old',expected_revision=2,expect_error=False),
])
(ROOT/'crates/fb-screendiff/scenarios/screens-d8.json').write_text(json.dumps(base, indent=2)+'\n')

screens = {k:v for k,v in json.loads(screens_path.read_text()).items() if not k.startswith('d8-')}
new = {}
company = '{company:D8 report books}'
def add(name, route, query='', status=200, **extra):
    path=f'/c/{company}/reports/{route}/'
    new['d8-'+name] = dict(path=path+('?' + query if query else ''), **({'status':status} if status != 200 else {}), **extra)
period='preset=custom&start=2026-10-01&end=2026-10-31'
for basis in ['ACCRUAL','CASH']:
    for key,route in [('pl','profit-and-loss'),('bs','balance-sheet'),('gl','general-ledger')]:
        query=('as_of=2026-10-31' if key=='bs' else period)+'&basis='+basis
        add(f'{key}-{basis.lower()}',route,query)
        if key!='gl':
            add(f'{key}-{basis.lower()}-csv',route,query+'&output=csv',media='csv')
    for end in ['2026-10-05','2026-10-09','2026-10-12','2026-10-20']:
        day=end[-2:]
        add(f'bs-{basis.lower()}-day-{day}','balance-sheet','as_of='+end+'&basis='+basis)
        add(f'gl-{basis.lower()}-through-{day}','general-ledger','end='+end+'&basis='+basis)
    add(f'gl-{basis.lower()}-csv','general-ledger',period+'&basis='+basis+'&output=csv',media='csv')
for preset in ['this_month','last_month','this_quarter','this_year','year_to_date']:
    for key,route in [('pl','profit-and-loss'),('gl','general-ledger')]:
        add(f'{key}-{preset.replace("_","-")}',route,'preset='+preset)
for key,route in [('pl','profit-and-loss'),('gl','general-ledger')]:
    for name,query in [('through','end=2026-10-31'),('day','start=2026-10-12&end=2026-10-12'),('cross-year','start=2025-12-01&end=2026-10-31'),('cross-month','start=2026-09-01&end=2026-10-31'),('empty','start=2099-01-01&end=2099-01-31'),('min','end=1900-01-02'),('max','start=9999-12-30&end=9999-12-30'),('default','')]:
        add(f'{key}-{name}',route,query)
    for name,query in [('invalid-basis','basis=%20CASH%20'),('invalid-preset','preset=%20custom%20'),('invalid-output','output=%20csv%20'),('invalid-all','basis=no&output=bad&preset=bad&start=bad&end=bad'),('invalid-missing-end','preset=custom'),('invalid-reversed','start=2026-10-31&end=2026-10-01'),('invalid-space-date','start=+'),('invalid-underflow','end=1900-01-01'),('invalid-overflow','end=9999-12-31'),('invalid-date-ignored-preset','preset=this_month&end=bad'),('invalid-output-before-cross','preset=custom&output=bad')]:
        add(f'{key}-{name}',route,query,400)
    add(f'{key}-post-refused',route,status=405,post={})
for name,query,status in [('default','',200),('prior-year','as_of=2025-12-31',200),('next-year','as_of=2027-01-01',200),('invalid-all','basis=bad&output=bad&as_of=bad',400),('invalid-padded-basis','basis=%20CASH%20',400),('invalid-blank-date','as_of=+',400),('invalid-bounds','as_of=1900-01-01',400)]:
    add('bs-'+name,'balance-sheet',query,status)
add('bs-post-refused','balance-sheet',status=405,post={})
groups=['','income','expense','pl','asset','liability','equity','le','current_assets','bank','receivable','other_current_assets','fixed_assets','other_assets','current_liabilities','payable','credit_cards','other_current_liabilities','long_term_liabilities','operating_income','cogs','operating_expense','other_income','other_expense']
for group in groups:
    add('gl-group-'+(group.replace('_','-') or 'all'),'general-ledger',period+'&group='+group)
for name,query,status in [('invalid-account','account=no',400),('invalid-group','group=%20pl%20',400),('invalid-account-before-cross','account=no&preset=custom',400),('invalid-account-group','account=no&group=no&preset=custom&output=bad',400),('invalid-account-length','account=1234567890123&group=bad&preset=custom&output=bad',400),('invalid-account-null','account=%00&group=bad&preset=custom&output=bad',400),('invalid-account-null-length','account=1234567890123%00&group=bad&preset=custom',400)]:
    add('gl-'+name,'general-ledger',query,status)
account='{account:D8 report books:090}'
for name,page in [('first','1'),('second','2'),('zero','0'),('negative','-1'),('huge','999999999999999999999999999999999999999'),('decimal','2.0'),('unicode','%D9%A2'),('underscore','0_2')]:
    add('gl-page-'+name,'general-ledger',period+'&account='+account+'&page='+page)
add('gl-account-cash','general-ledger',period+'&basis=CASH&account={account:D8 report books:400}')
for name,spec in new.items():
    if name in screens:
        assert screens[name] == spec, name
    else:
        screens[name] = spec
screens_path.write_text(json.dumps(screens,indent=2)+'\n')
print('D8 appended',len(new),'screens; total',len(screens))
