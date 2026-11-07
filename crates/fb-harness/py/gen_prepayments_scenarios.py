"""Generate fixed workflows and 500 seeded commands through the frozen Python services.

Source py/env.sh with FBV2_DB pointing at a fresh migrated dedicated harness DB,
then run with $FB_PY. FB_SCENARIO_OUT optionally selects an existing output directory.
Generation refuses unexpected results; it never learns expectations from the run.
"""
from pathlib import Path
import json,random,uuid,copy,os
OUT=Path(os.environ.get('FB_SCENARIO_OUT',Path(__file__).resolve().parents[1]/'scenarios'))
def key(n):return str(uuid.UUID(int=700000+n))
def cmd(op,**kw):return dict(op='pp_'+op,company='c',**kw)
def setup():
 s=[dict(op='create_company',ref='c',name='Synthetic prepayments')]
 for ref,code,kind,detail in [('bank','100','ASSET','BANK'),('ar','110','ASSET','RECEIVABLE'),('uf','120','ASSET','OTHER_CURRENT_ASSET'),('liability','230','LIABILITY','OTHER_CURRENT_LIABILITY'),('liability2','231','LIABILITY','OTHER_CURRENT_LIABILITY'),('income','400','INCOME','INCOME')]:
  s.append(dict(op='save_account',company='c',ref=ref,code=code,name=ref,kind=kind,detail_type=detail))
 s += [dict(op='l2_save',company='c',model='customer',ref='buyer',values=dict(display_name='Buyer')),dict(op='l2_save',company='c',model='customer',ref='other',values=dict(display_name='Other buyer')),dict(op='l2_setup',company='c',expected_revision=0,values=dict(receivables='ar',undeposited='uf',customer_deposits='liability',banks=['bank']))]
 return s
def dep(n,**kw):
 v=dict(request_key=key(n),customer_id='buyer',date='2026-10-01',amount='500.00',method='CHECK',destination='UNDEPOSITED',order_reference='SO-1');v.update(kw);return v
def invoice(s,n=1,amount='1200.00',customer='buyer'):
 name='inv'+str(n);v=dict(request_key=key(n),customer_id=customer,date='2026-10-02',tax_mode='NONE',lines=[dict(description='Synthetic work',quantity='1',rate=amount,taxable=False,income_account_id='income')])
 s += [dict(op='sales_save_invoice',company='c',ref=name,values=v),dict(op='sales_review_invoice',company='c',record=name,expected_revision=1,review=name),dict(op='sales_post_invoice',company='c',record=name,expected_revision=1,review=name)]
 return name
def receive(s,n,**kw):
 v=dep(n,**kw);name='dep'+str(n);s += [cmd('review_deposit',review=name,values=v),cmd('record_deposit',review=name,values=v,ref=name)];return name,v
def apply(s,n,d,i,amount='500.00'):
 name='app'+str(n);v=dict(request_key=key(n),date='2026-10-03',invoice_id=i,amount=amount);s += [cmd('review_application',deposit=d,review=name,values=v),cmd('apply',deposit=d,review=name,values=v,ref=name)];return name,v
def refund(s,n,d,**kw):
 name='refund'+str(n);v=dict(request_key=key(n),date='2026-10-04',amount='100.00',bank_id='bank',method='CHECK',check_number='4001');v.update(kw);s += [cmd('review_refund',deposit=d,review=name,values=v),cmd('record_refund',deposit=d,review=name,values=v,ref=name)];return name,v

# Reviewed intended refusals for fixed workflows; never learned during generation.
REFUSALS = {'1200-less-500': {},
 'lifecycle-retries': {19: {'expect_error': True,
                            'expect_kind': 'ValidationError',
                            'expect_message': 'Unapply this deposit from invoice INV-000001 before voiding '
                                              'it.'},
                       24: {'expect_error': True,
                            'expect_kind': 'ValidationError',
                            'expect_message': 'Void date cannot precede 2026-10-11, when this deposit was '
                                              'last released by an unapply or refund void.'}},
 'refusals': {19: {'expect_error': True,
                   'expect_kind': 'ValidationError',
                   'expect_message': 'The amount to apply (300.01) exceeds the deposit’s remaining balance '
                                     '300.00.'},
              20: {'expect_error': True,
                   'expect_kind': 'ValidationError',
                   'expect_message': 'Deposit amount must be positive.'},
              21: {'expect_error': True,
                   'expect_kind': 'ValidationError',
                   'expect_message': 'Enter a finite nonnegative value with at most 2 decimal places within '
                                     'the supported limit.'},
              22: {'expect_error': True,
                   'expect_kind': 'ValidationError',
                   'expect_message': 'Enter a finite nonnegative value with at most 2 decimal places within '
                                     'the supported limit.'},
              23: {'expect_error': True,
                   'expect_kind': 'ValidationError',
                   'expect_message': 'Choose a payment method (record only).'},
              24: {'expect_error': True,
                   'expect_kind': 'ValidationError',
                   'expect_message': 'Choose where the deposit goes.'},
              25: {'expect_error': True,
                   'expect_kind': 'ValidationError',
                   'expect_message': 'Do not select a bank for undeposited funds.'},
              26: {'expect_error': True,
                   'expect_kind': 'ValidationError',
                   'expect_message': 'Supply a stable UUID request key.'},
              27: {'expect_error': True,
                   'expect_kind': 'ValidationError',
                   'expect_message': 'Enter text up to 120 characters.'},
              28: {'expect_error': True,
                   'expect_kind': 'ValidationError',
                   'expect_message': 'The deposit destination must be an active postable ASSET account '
                                     '(undeposited funds or a designated bank).'},
              29: {'expect_error': True,
                   'expect_kind': 'ValidationError',
                   'expect_message': 'Refund 301.00 exceeds the deposit’s remaining balance 300.00.'},
              30: {'expect_error': True,
                   'expect_kind': 'ValidationError',
                   'expect_message': 'The refund date cannot precede the deposit date.'},
              31: {'expect_error': True,
                   'expect_kind': 'ValidationError',
                   'expect_message': 'Choose a refund method (record only).'},
              32: {'expect_error': True,
                   'expect_kind': 'ValidationError',
                   'expect_message': 'Only a refund by check carries a check number.'},
              33: {'expect_error': True,
                   'expect_kind': 'ValidationError',
                   'expect_message': 'A deposit refund is paid from a designated bank, never from a '
                                     'receivables, undeposited-funds or other control account.'},
              34: {'expect_error': True,
                   'expect_kind': 'ValidationError',
                   'expect_message': 'Invalid review. Open a fresh customer deposit review.'}},
 'stale-and-frozen-accounts': {18: {'expect_error': True,
                                    'expect_kind': 'ValidationError',
                                    'expect_message': 'The invoice balance or the deposit’s remaining '
                                                      'balance changed. Open a fresh review before '
                                                      'applying.'},
                               21: {'expect_error': True,
                                    'expect_kind': 'ValidationError',
                                    'expect_message': 'Customer deposit or posting mapping changed. Open a '
                                                      'fresh review before recording.'}},
 'refund-number-release': {14: {'expect_error': True,
                                'expect_kind': 'ValidationError',
                                'expect_message': 'Check number 4001 is already used by deposit refund '
                                                  'CDR-000001 on this bank account. Choose another number or '
                                                  'void that refund first.'}},
 'scope-dates': {15: {'expect_error': True,
                      'expect_kind': 'ValidationError',
                      'expect_message': 'Apply a deposit only to this customer’s posted, unvoided invoices '
                                        '(INV-000001 is not one).'},
                 16: {'expect_error': True,
                      'expect_kind': 'ValidationError',
                      'expect_message': 'The application date cannot precede the deposit date.'},
                 17: {'expect_error': True,
                      'expect_kind': 'PermissionDenied',
                      'expect_message': 'Company access denied.'},
                 18: {'expect_error': True,
                      'expect_kind': 'ValidationError',
                      'expect_message': 'This record changed since you opened it. Reload and review before '
                                        'saving.'}}}

def save(name,s):
 for index,step in enumerate(s):
  step.update(REFUSALS.get(name,{}).get(index,{}))
  step.setdefault('expect_error',False)
  if step['expect_error']:step.setdefault('expect_kind','PermissionDenied' if step.get('actor')=='outsider' else 'ValidationError')
 (OUT/('prepayments-'+name+'.json')).write_text(json.dumps(dict(users=[dict(username='prepayments',is_staff=True),dict(username='outsider')],steps=s,as_of=['2026-10-01','2026-10-03','2026-10-31']),ensure_ascii=False,indent=2)+'\n')

s=setup();i=invoice(s);d,dv=receive(s,10);a,av=apply(s,20,d,i)
v=dict(request_key=key(21),customer_id='buyer',date='2026-10-04',amount='700.00',method='CASH',destination='BANK',bank_id='bank',allocations=[dict(invoice_id=i,amount='700.00')])
s += [dict(op='sales_review_receipt',company='c',review='rest',values=v),dict(op='sales_record_receipt',company='c',review='rest',values=v,ref='rest'),cmd('record_deposit',review=d,values=dv,ref=d),cmd('apply',deposit=d,review=a,values=av,ref=a)]
save('1200-less-500',s)

s=setup();i=invoice(s);d,dv=receive(s,30,destination='BANK',bank_id='bank');a,av=apply(s,31,d,i,'300.00');r,rv=refund(s,32,d)
s += [cmd('void_deposit',record=d,expected_revision=1,date='2026-10-10',reason='Correct',expect_error=True),cmd('unapply',record=a,expected_revision=1,date='2026-10-10',reason='Correct'),cmd('unapply',record=a,expected_revision=1,date='2026-10-10',reason='Correct'),cmd('void_refund',record=r,expected_revision=1,date='2026-10-11',reason='Correct'),cmd('void_refund',record=r,expected_revision=1,date='2026-10-11',reason='Correct'),cmd('void_deposit',record=d,expected_revision=1,date='2026-10-10',reason='Correct',expect_error=True),cmd('void_deposit',record=d,expected_revision=1,date='2026-10-11',reason='Correct'),cmd('void_deposit',record=d,expected_revision=1,date='2026-10-11',reason='Correct'),cmd('record_deposit',review=d,values=dv,ref=d),cmd('apply',deposit=d,review=a,values=av,ref=a),cmd('record_refund',deposit=d,review=r,values=rv,ref=r)]
save('lifecycle-retries',s)

s=setup();i=invoice(s);d,dv=receive(s,40);a,av=apply(s,41,d,i,'100.00');r,rv=refund(s,42,d)
s += [cmd('review_application',deposit=d,review='too-much',values=dict(request_key=key(43),date='2026-10-03',invoice_id=i,amount='300.01'),expect_error=True)]
for n,kw in enumerate([dict(amount='0'),dict(amount='-1'),dict(amount='abc'),dict(method='BITCOIN'),dict(destination='MOON'),dict(destination='UNDEPOSITED',bank_id='bank'),dict(request_key='nope'),dict(order_reference='x'*121),dict(destination='BANK',bank_id='income')],50):
 s.append(cmd('review_deposit',review='bad'+str(n),values=dep(n,**kw),expect_error=True))
for n,kw in enumerate([dict(amount='301.00'),dict(date='2026-09-30'),dict(method='CARD'),dict(method='CASH',check_number='x'),dict(bank_id='ar')],70):
 v=dict(rv,request_key=key(n),check_number='');v.update(kw);s.append(cmd('review_refund',deposit=d,review='bad'+str(n),values=v,expect_error=True))
s.append(cmd('record_deposit',review='forged',ref='bad',values=dep(80),expect_error=True))
save('refusals',s)

s=setup();i=invoice(s);d,dv=receive(s,90);v=dict(request_key=key(91),date='2026-10-03',invoice_id=i,amount='100.00')
s.append(cmd('review_application',deposit=d,review='stale',values=v));apply(s,92,d,i,'100.00');s.append(cmd('apply',deposit=d,review='stale',ref='stale',values=v,expect_error=True))
v=dep(93);s.append(cmd('review_deposit',review='mapping',values=v));s.append(dict(op='l2_setup',company='c',expected_revision=1,values=dict(receivables='ar',undeposited='uf',customer_deposits='liability2',banks=['bank'])));s.append(cmd('record_deposit',review='mapping',ref='bad',values=v,expect_error=True));refund(s,94,d)
save('stale-and-frozen-accounts',s)

s=setup();d,dv=receive(s,100);r,rv=refund(s,101,d);v=dict(rv,request_key=key(102));s.append(cmd('review_refund',deposit=d,review='same-number',values=v,expect_error=True));s.append(cmd('void_refund',record=r,expected_revision=1,date='2026-10-05',reason='Correct'));refund(s,103,d);save('refund-number-release',s)

s=setup();i=invoice(s,2,customer='other');d,dv=receive(s,110)
for n,kw in enumerate([dict(invoice_id=i,date='2026-10-03'),dict(invoice_id=i,date='2026-09-30')],111):s.append(cmd('review_application',deposit=d,review='wrong'+str(n),values=dict(request_key=key(n),amount='1.00',**kw),expect_error=True))
s.append(cmd('review_deposit',actor='outsider',review='denied',values=dep(114),expect_error=True));s.append(cmd('void_deposit',record=d,expected_revision=0,date='2026-10-02',reason='stale',expect_error=True));save('scope-dates',s)

# Exactly 500 seeded workflow commands after setup. Generation executes the frozen
# Python services on a fresh dedicated harness DB (source py/env.sh first). Each
# emitted command has an authored success/refusal contract; an unexpected result
# aborts generation, never changes the contract to match a broken implementation.
# Live pools are updated only after the actual record/release command succeeds.
from collections import Counter
from run_scenario import Runner, connection

with connection.cursor() as cursor:
 cursor.execute('SELECT count(*) FROM ledger_company')
 if cursor.fetchone()[0]:raise SystemExit('generation needs a fresh migrated harness DB')
s=setup();i=invoice(s,200,amount='99999.00')
scenario=dict(users=[dict(username='prepayments',is_staff=True),dict(username='outsider')],steps=s)
runner=Runner(scenario);runner.run()
assert all(o['ok'] for o in runner.outcomes),runner.outcomes
rng=random.Random(7030);live={'deposits':set(),'applications':set(),'refunds':set()};counts=Counter()
def emit(step,pool=None,release=False):
 step.setdefault('expect_error',False)
 if step['expect_error']:
  step.setdefault('expect_kind','ValidationError')
  step.setdefault('expect_message','Unapply this deposit from invoice INV-000001 before voiding it.')
 outcome=runner.run_step(step)
 assert not outcome.get('unexpected'),(step,outcome)
 s.append(step)
 if outcome['ok']:
  counts[step['op']]+=1
  if pool is not None:
   if release:live[pool].remove(step['record'])
   else:live[pool].add(step['ref'])

# Fifty randomized complete lifecycles: random amounts, dates, destinations,
# payment methods, application/refund order and release order. Each includes a
# genuine blocked void, then successful unapply, refund void and deposit void.
for cycle in range(50):
 n=1000+cycle*10;d='randomdep'+str(n);a='randomapp'+str(n);r='randomrefund'+str(n)
 day=rng.randint(2,20);date=lambda offset:f'2026-10-{day+offset:02}'
 amount=rng.randint(100,900);applied=rng.randint(1,amount//2);refunded=rng.randint(1,amount-applied)
 dest=rng.choice(['BANK','UNDEPOSITED'])
 v=dep(n,date=date(0),amount=f'{amount}.00',destination=dest,method=rng.choice(['CASH','CHECK','OTHER']))
 if dest=='BANK':v['bank_id']='bank'
 emit(cmd('review_deposit',review=d,values=v))
 emit(cmd('record_deposit',review=d,values=v,ref=d),'deposits')
 assert d in live['deposits']
 av=dict(request_key=key(n+1),date=date(1),invoice_id=i,amount=f'{applied}.00')
 method=rng.choice(['CASH','CHECK','OTHER'])
 rv=dict(request_key=key(n+2),date=date(1),amount=f'{refunded}.00',bank_id='bank',method=method,check_number=str(n) if method=='CHECK' else '')
 pairs=[('review_application','apply',a,av,'applications'),('review_refund','record_refund',r,rv,'refunds')]
 rng.shuffle(pairs)
 for review,record,name,values,pool in pairs:
  emit(cmd(review,deposit=d,review=name,values=values))
  emit(cmd(record,deposit=d,review=name,values=values,ref=name),pool)
 emit(cmd('void_deposit',record=d,expected_revision=1,date=date(2),reason='Blocked by live settlements',expect_error=True))
 releases=[('unapply',a,'applications'),('void_refund',r,'refunds')];rng.shuffle(releases)
 for op,name,pool in releases:
  assert name in live[pool]
  emit(cmd(op,record=name,expected_revision=1,date=date(3),reason='Random correction'),pool,True)
 emit(cmd('void_deposit',record=d,expected_revision=1,date=date(4),reason='Random correction'),'deposits',True)
assert len(s)==513,len(s)
assert all(not pool for pool in live.values()),live
for op in ('pp_apply','pp_unapply','pp_record_refund','pp_void_refund','pp_void_deposit'):
 assert counts[op]==50,(op,counts)
save('random-500',s)
print('prepayments random successful commands:',dict(counts))
