"""Seeded translated banking workflows and a 500-command randomized run."""
from pathlib import Path
import copy,json,random,uuid
from decimal import Decimal
root=Path(__file__).resolve().parents[1]
count=10000
def key():
    global count
    count+=1
    return str(uuid.UUID(int=count))
base=json.loads((root/'scenarios/sales-cash-sales.json').read_text())
base['steps']=base['steps'][:12]
def op(name,**kw):return dict(op=name,company='c',**kw)
def save(name,steps):
    out=copy.deepcopy(base);out['steps']+=steps;(root/f'scenarios/banking-{name}.json').write_text(json.dumps(out,indent=1)+'\n')
def sale(name,n='10.25',dest='UNDEPOSITED'):
    p=dict(request_key=key(),customer_id='buyer',date='2026-10-01',memo='Synthetic banking sale',tax_mode='NONE',lines=[dict(item_id=None,description='Service',quantity='1',rate=n,taxable=False,income_account_id='income')],method='CASH',destination=dest)
    if dest=='BANK':p['bank_id']='bank'
    return [op('sales_review_sale',values=p,review=name),op('sales_record_sale',values=p,review=name,ref=name)]
def deposit(name,receipts,date='2026-10-02',**kw):return op('banking_deposit',ref=name,values=dict(request_key=key(),bank_id='bank',date=date,reference='Synthetic deposit',memo='Banking workflow',receipt_ids=receipts),**kw)
def journal(name,n,debit=True,date='2026-10-01'):
    lines=[dict(account='bank',debit=n),dict(account='income',credit=n)] if debit else [dict(account='expense',debit=n),dict(account='bank',credit=n)]
    return [op('save_draft',ref=name,date=date,memo='Synthetic statement line',lines=lines),op('post_journal',journal=name)]
def begin(name,balance,**kw):return op('banking_begin',ref=name,account='bank',values=dict(request_key=key(),statement_date='2026-10-31',ending_balance=balance,**kw))
def finish(name,rev):return op('banking_finish',account='bank',reconciliation=name,expected_revision=rev,completion_key=key())
def undo(name,rev):return op('banking_undo',account='bank',reconciliation=name,expected_revision=rev,reason='Synthetic correction',undo_key=key())
steps=sale('sale1')+sale('sale2','30.00')
dep=deposit('deposit',['sale1','sale2']);steps+=[dep,copy.deepcopy(dep),op('banking_register',account='bank',values={},result='deposited'),op('banking_receipt_guard',receipt='sale1',date='2026-10-03',expect_error=True),op('banking_reverse',deposit='deposit',expected_revision=1,date='2026-10-05',reason='Synthetic correction')]
steps+=[deposit('early',['sale1'],expect_error=True),deposit('redeposit',['sale1'],'2026-10-05'),op('banking_register',account='bank',values={'start':'2026-10-03'},result='since-reversal')]
save('deposits',steps)
steps=journal('increase','6200.27')+journal('decrease','266.85',False)
steps+=[op('banking_clear',line='increase:1',state='C',expected_revision=0),op('banking_clear',line='decrease:2',state='C',expected_revision=0),begin('statement','5933.43')]
bad=finish('statement',1);bad['expect_error']=True;steps+=[bad,begin('statement','5933.42',expected_revision=1),finish('statement',2),op('banking_register',account='bank',values={'state':'R'},result='reconciled'),begin('second','5933.42'),finish('second',1)]
bad=undo('statement',3);bad['expect_error']=True;steps+=[bad,undo('second',2),undo('statement',3),op('banking_register',account='bank',values={'state':'C'},result='undone')];save('reconcile',steps)
steps=[begin('charges','-1.00',service_charge='2.25',service_account_id='expense',interest='1.25',interest_account_id='income')]
steps+=[copy.deepcopy(steps[0]),finish('charges',1),undo('charges',2),op('banking_register',account='bank',values={},result='charges-retained')];save('charges',steps)
steps=journal('early','100')+journal('late','5',date='2026-11-01')
steps+=[op('banking_clear',line='early:1',state='C',expected_revision=0),op('banking_clear',line='late:1',state='C',expected_revision=0),begin('date-filter','100'),op('banking_ticks',account='bank',reconciliation='date-filter',expected_revision=1,shown={},ticked=[],values={'hide_later':False}),begin('date-filter','105',expected_revision=2),finish('date-filter',3),op('banking_register',account='bank',values={'end':'2026-10-31'},result='cutoff')];save('later-lines',steps)
# 500 seeded commands, after fixed setup; stale revisions, exact retries and state changes.
rng=random.Random(3410050);steps=[];state={};amounts={};rev={}
for i in range(12):
    name=f'line{i}';amount=Decimal(rng.randint(1,20000))/100;debit=i%3!=0;steps+=journal(name,str(amount),debit);line=name+(':1' if debit else ':2');state[line]='U';rev[line]=0;amounts[line]=amount if debit else -amount
steps.append(begin('random','0.00'));rrev=1
for i in range(500):
    choice=rng.randrange(10)
    if choice<7:
        line=rng.choice(list(state));wanted=rng.choice(['U','C']);expected=rev[line] if rng.randrange(5) else max(0,rev[line]-1)
        bad=wanted!=state[line] and expected!=rev[line]
        steps.append(op('banking_clear',line=line,state=wanted,expected_revision=expected,**({'expect_error':True} if bad else {})))
        if not bad and wanted!=state[line]:state[line]=wanted;rev[line]+=1
    elif choice<9:
        balance=str(Decimal(rng.randint(-10000,10000))/100);steps.append(begin('random',balance,expected_revision=rrev));rrev+=1
    else:
        picked=rng.sample(list(state),rng.randint(1,5));ticked=[x for x in picked if rng.choice([True,False])];shown={x:rev[x] for x in picked};changes=any(state[x]!=('C' if x in ticked else 'U') for x in picked)
        steps.append(op('banking_ticks',account='bank',reconciliation='random',expected_revision=rrev,shown=shown,ticked=ticked))
        for x in picked:
            wanted='C' if x in ticked else 'U'
            if wanted!=state[x]:state[x]=wanted;rev[x]+=1
        if changes:rrev+=1
balance=sum((amounts[x] for x in state if state[x]=='C'),Decimal('0.00'));steps+=[begin('random',str(balance),expected_revision=rrev),finish('random',rrev+1),undo('random',rrev+2),op('banking_register',account='bank',values={'state':'open'},result='random-final')];save('random-500',steps)
