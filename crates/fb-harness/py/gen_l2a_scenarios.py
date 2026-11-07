"""Generate L2a scenarios (seeded; no application or database writes)."""
import json, random
from pathlib import Path
out = Path(__file__).resolve().parents[1] / 'scenarios'
base = [dict(op='create_company', ref='c', name='Synthetic L2a'), dict(op='create_company', ref='o', name='Other synthetic')]
for ref, kind in [('asset','ASSET'), ('bank','ASSET'), ('ap','LIABILITY'), ('taxacct','LIABILITY'), ('income','INCOME'), ('expense','EXPENSE')]:
    base.append(dict(op='save_account', company='c', ref=ref, code=ref, name=ref, kind=kind))
def save(name, steps):
    (out / (name+'.json')).write_text(json.dumps(dict(users=[dict(username='l2', is_staff=True),dict(username='outsider')], steps=base+steps), indent=1)+'\n')
def d(model, ref, **v):
    return dict(op='l2_save', company='c', model=model, ref=ref, values=v)
def tax(op, ref, **v):
    return dict(op=op, company='c', ref=ref, values=v)
def settings(rev, **kw):
    v=dict(charge_tax=True,default_item_id=None,owed_basis='ACCRUAL',filing_frequency='MONTHLY',expected_revision=rev);v.update(kw)
    return dict(op='l2_settings',company='c',values=v)
steps=[d('customer','buyer',display_name=' Buyer ',tax_status='EXEMPT',exemption_number=' RESALE '),d('vendor','agency',display_name='Agency',default_terms='30',default_expense_account='expense'),d('item','service',code='Service',kind='SERVICE',sale_rate='12.345',income_account='income'),d('item','goods',code='Goods',kind='NONSTOCK',sale_rate='112.50',taxable=False)]
steps += [dict(op='l2_save',company='c',model='item',record='service',values=dict(kind='NONSTOCK')),
          dict(op='l2_active',company='c',model='item',record='service',active=False),d('item','duplicate',code=' service ',kind='SERVICE',sale_rate='1',expect_error=True)]
# expect_error is a step annotation, not a master-data field.
steps[-1]['values'].pop('expect_error');steps[-1]['expect_error']=True
steps += [d('customer','bad',display_name='Bad',tax_status='NO'),d('vendor','badterms',display_name='Bad',default_terms=30),d('customer','blank',display_name=' '),d('vendor','bademail',display_name='Bad',email='not-an-email'),d('item','badkind',code='x',kind='INVENTORY',sale_rate='1')]
for v in ['NaN','Infinity','-0.001','100000000000000',True,1.2]:steps.append(d('item','badmoney',code='bad',kind='SERVICE',sale_rate=v))
steps += [settings(0),dict(op='l2_bulk',company='c',customers=True,items=True,stale=True),dict(op='l2_bulk',company='c',customers=True,items=True),dict(op='l2_bulk',company='c',customers=True,items=True)]
save('l2a-directory',steps)
steps=[d('vendor','agency',display_name='Agency'),tax('l2_rate','county',name='County',description='County tax',rate='3',agency_id='agency'),tax('l2_rate','state',name='State',description='State tax',rate='4',agency_id='agency'),tax('l2_group','group',name='County-State',description='Combined tax',member_ids=['county','state']),settings(0,default_item_id='group')]
for amount in ['112.50','1.08','18.50','0','0.01','999999999999.99']:
    steps.append(dict(op='l2_totals',company='c',resolve=True,values=dict(tax_mode='ITEM',tax_item_id='group'),lines=[dict(quantity='1',rate=amount,taxable=True)],result='tax-'+amount))
steps += [tax('l2_group','dup',name='Dup',description='Dup',member_ids=['county','county']),tax('l2_group','nested',name='Nested',description='Nested',member_ids=['county','group']),tax('l2_rate','over',name='Over',description='Over',rate='100.0001',agency_id='agency'),dict(op='l2_tax_active',company='c',record='county',active=False,expected_revision=1),dict(op='l2_tax_active',company='c',record='group',active=False,expected_revision=1),tax('l2_rate','county',item_id='county',expected_revision=1,name='County',description='County tax now',rate='5',agency_id='agency'),dict(op='l2_totals',company='c',resolve=True,values=dict(tax_mode='ITEM',tax_item_id='group'),lines=[],result='edited'),settings(0)]
save('l2a-tax-rounding',steps)
steps=[]
for i,k in enumerate(['EST','INV','RCP','BILL','BPAY','DEP','CHK','SR','TRF','CM','RFD','CDP','CDA','CDR']):
    v=dict(op='l2_identity',company='c',kind=k,request_key=f'00000000-0000-0000-0000-{i:012}',ref='id-'+k);steps += [v,dict(v)]
steps += [dict(op='l2_identity',company='c',kind='INV',request_key='00000000-0000-0000-0000-000000000000',ref='wrongkind'),dict(op='l2_identity',company='c',kind='BAD',request_key='bad',ref='bad'),dict(op='edit_company',company='c',name='Renamed',currency='EUR'),dict(op='l2_setup',company='c',values=dict(receivables='asset',payables='ap',collected_tax='taxacct',banks=['bank']),expected_revision=0),dict(op='l2_setup',company='c',values={},expected_revision=0),dict(op='l2_snapshot',company='c',source='id-INV',values={'receivables':'asset','income':'income'}),dict(op='l2_snapshot',company='c',source='id-INV',values={'receivables':'asset','income':'income'})]
# snapshot keys are arbitrary roles; use role names recognized by account mapper, income needs explicit special mapping below.
steps[-2]['values']={'receivables':'asset'};steps[-1]['values']={'receivables':'asset'}
save('l2a-numbering',steps)
rng=random.Random(20261030)
steps=[d('vendor','agency',display_name='Agency'),tax('l2_rate','r0',name='Rate 0',description='Rate zero',rate='3',agency_id='agency'),tax('l2_rate','r1',name='Rate 1',description='Rate one',rate='4',agency_id='agency'),tax('l2_group','g',name='Combined',description='Combined tax',member_ids=['r0','r1']),settings(0)]
# Exactly 500 randomized commands, in addition to the fixed seed setup.
customers=[];items=[];revs=[1,1];settings_rev=1
for i in range(500):
    choice=rng.randrange(8)
    if choice in (0,1):
        ref=f'c{i}';customers.append(ref);steps.append(d('customer',ref,display_name=f'Customer {rng.randrange(20)}',tax_status=rng.choice(['TAXABLE','EXEMPT']),exemption_number=str(i)))
    elif choice==2:
        ref=f'i{i}';items.append(ref);steps.append(d('item',ref,code=f'Item {i}',kind=rng.choice(['SERVICE','NONSTOCK']),sale_rate=f'{rng.randrange(10000)}.{rng.randrange(100):02}',taxable=rng.choice([True,False])))
    elif choice==3 and customers:
        steps.append(dict(op='l2_save',company='c',model='customer',record=rng.choice(customers),values=dict(tax_status=rng.choice(['EXEMPT','TAXABLE']),notes=f'edit {i}')))
    elif choice==4 and items:
        steps.append(dict(op='l2_active',company='c',model='item',record=rng.choice(items),active=rng.choice([True,False])))
    elif choice==5:
        j=rng.randrange(2);steps.append(tax('l2_rate',f'r{j}',item_id=f'r{j}',expected_revision=revs[j],name=f'Rate {j}',description=f'Rate {j}',rate=str(rng.randrange(50000)/10000),agency_id='agency'));revs[j]+=1
    elif choice==6:
        steps.append(dict(op='l2_totals',company='c',resolve=True,values=dict(tax_mode='ITEM',tax_item_id='g'),lines=[dict(quantity=f'{rng.randrange(1,1000)}.{rng.randrange(10000):04}',rate=f'{rng.randrange(1000)}.{rng.randrange(100):02}',taxable=rng.choice([True,False])) for _ in range(rng.randrange(1,8))],result=f'calc{i}'))
    else:
        steps.append(settings(settings_rev,owed_basis=rng.choice(['CASH','ACCRUAL']),filing_frequency=rng.choice(['MONTHLY','QUARTERLY','ANNUALLY'])));settings_rev+=1
save('l2a-random-500',steps)
# L1 chart+trial-balance ordering, including punctuation, case, accents and deterministic ties.
steps=[]
for i,code in enumerate(['100-1','1000','a','A','á','a-b','ab','a b','a_b','é','e\u0301','É','Æ','AE','ß','ss','Z','z','1â','1,â']):
    steps.append(dict(op='save_account',company='c',ref=f'coll{i}',code=code,name=code,kind='ASSET'))
    steps.append(d('customer',f'contact{i}',display_name=code))
for i, code in enumerate(['I','i','İ','ı','K','k','ſ','s','ß','SS']):
    steps.append(d('item',f'case{i}',code=code,kind='SERVICE',sale_rate='1'))
    steps.append(tax('l2_rate',f'taxcase{i}',name=code,description=code,rate='1',new_agency_name='Agency'))
save('l2a-collation',steps)
steps=[d('vendor','agency',display_name='Agency'),tax('l2_rate','a',name='A',description='A',rate='50',agency_id='agency'),tax('l2_rate','b',name='B',description='B',rate='50',agency_id='agency'),tax('l2_group','group',name='Group',description='Group',member_ids=['a','b']),tax('l2_rate','a',item_id='a',expected_revision=1,name='A',description='A',rate='60',agency_id='agency'),settings(0),dict(op='l2_totals',company='c',resolve=True,values=dict(tax_mode='ITEM',tax_item_id='group'),lines=[],result='too-much')]
save('l2a-rate-edit-oddity',steps)
