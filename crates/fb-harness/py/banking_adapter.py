"""Read-only frozen-reference banking adapter. No matching or provider requests."""
from journal_aliases import journal_names
from banking import services as deposits, cleared, reconcile, register
from banking.models import Deposit,DepositLine,DepositReversal,Reconciliation,ReconciliationLine,BankLineStatus
from ledger.models import JournalLine
from django.core.exceptions import ValidationError
from l2a import stringify

DEPOSIT=('id','company_id','identity_id','bank_id','journal_id','date','amount','reference','memo','actor_name','company_snapshot')
REVERSAL=('deposit_id','journal_id','date','reason','actor_name')
REC=('id','company_id','account_id','status','statement_date','ending_balance','hide_later','request_key','revision','started_by','completed_by','undone_by','completion_key','undo_key','undo_reason','beginning_balance','cleared_debit','cleared_credit','cleared_count','service_charge','service_account_id','service_journal_id','interest','interest_account_id','interest_journal_id')


def state(r):
    if not hasattr(r,'banking_refs'):
        r.banking_refs={};r.banking_reviews={};r.banking_results={};r.banking_screens={};r.banking_reports={}

def line(r,name):
    j,p=name.rsplit(':',1)
    return JournalLine.objects.get(journal_id=r.refs['journal'][j],position=int(p)).pk

def payload(r,v):
    if isinstance(v,list):return [payload(r,x) for x in v]
    if not isinstance(v,dict):return v
    out={}
    for k,x in v.items():
        if k in ('bank_id','account_id','service_account_id','interest_account_id') and isinstance(x,str):x=r.refs['account'].get(x,999999999)
        elif k=='receipt_ids' and isinstance(x,list):x=[r.sales_refs['receipt'].get(n,999999999) if isinstance(n,str) else n for n in x]
        else:x=payload(r,x)
        out[k]=x
    return out

def track(r,name,obj):
    r.banking_refs.setdefault(name,obj.pk)
    if isinstance(obj,Deposit):
        r.l2_refs.setdefault(name+':identity',obj.identity_id);r.l2_types[name+':identity']='identity'
    for field,suffix in (('journal_id','posted'),('service_journal_id','service'),('interest_journal_id','interest')):
        key=getattr(obj,field,None)
        if key:r.refs['journal'].setdefault(name+':'+suffix,key)

def apply_banking(r,s,actor):
    if not s['op'].startswith('banking_'):return False
    state(r);op=s['op'];c=r.company(s['company']);v=payload(r,s.get('values',{}));expected=s.get('expected_revision')
    if op=='banking_receipt_guard':
        from sales.models import Receipt
        from django.db import transaction
        with transaction.atomic():
            from ledger.services import access
            access(actor,c);deposits.refuse_live_deposit(c,Receipt.objects.get(company=c,pk=r.sales_refs['receipt'][s['receipt']]),reversal_date=reconcile._date(s['date']))
    elif op=='banking_reverse_journal':
        from ledger import services
        j=services.reverse_journal(actor,c,r.refs['journal'][s['journal']],date=s['date'],reason=s['reason']);r.refs['journal'][s['ref']]=j.pk
    elif op=='banking_review':r.banking_reviews[s['review']]=deposits.review_deposit(actor,c,**v)['token']
    elif op=='banking_deposit':
        token=r.banking_reviews[s['review']] if 'review' in s else deposits.review_deposit(actor,c,**v)['token']
        obj=deposits.record_deposit(actor,c,review_token=token,**v);track(r,s['ref'],obj)
    elif op=='banking_reverse':
        obj=deposits.reverse_deposit(actor,c,r.banking_refs[s['deposit']],expected_revision=expected,date=s['date'],reason=s['reason']);r.refs['journal'].setdefault(s['deposit']+':reversed',obj.journal_id)
    elif op=='banking_clear':cleared.set_cleared(actor,c,line(r,s['line']),state=s['state'],expected_revision=expected)
    elif op=='banking_begin':
        obj=reconcile.begin(actor,c,r.refs['account'][s['account']],**v);track(r,s['ref'],obj)
    elif op=='banking_ticks':
        reconcile.save_ticks(actor,c,r.banking_refs[s['reconciliation']],account_id=r.refs['account'][s['account']],expected_revision=expected,shown={line(r,k):v for k,v in s['shown'].items()},ticked=[line(r,k) for k in s['ticked']],**v)
    elif op=='banking_finish':reconcile.reconcile_now(actor,c,r.banking_refs[s['reconciliation']],account_id=r.refs['account'][s['account']],expected_revision=expected,completion_key=s['completion_key'])
    elif op=='banking_undo':reconcile.undo_last(actor,c,r.banking_refs[s['reconciliation']],account_id=r.refs['account'][s['account']],expected_revision=expected,reason=s['reason'],undo_key=s['undo_key'])
    elif op in ('banking_screen','banking_report'):
        rec=Reconciliation.objects.get(company=c,pk=r.banking_refs[s['reconciliation']],account_id=r.refs['account'][s['account']])
        result=reconcile.screen(c,rec) if op=='banking_screen' else reconcile.report(c,rec)
        (r.banking_screens if op=='banking_screen' else r.banking_reports)[s['result']]=result
    elif op=='banking_register':
        f=register.RegisterFilterForm(v,company=c,today=__import__('datetime').date.fromisoformat(r.scenario.get('today','2026-10-30')))
        if not f.is_valid():raise ValidationError('Invalid register filters.')
        a=cleared.register_accounts(c).get(pk=r.refs['account'][s['account']]);r.banking_results[s['result']]=register.build(c,a,f.cleaned_data,page=s.get('page',1))
    else:raise ValueError(op)
    return True

def maps(r):
    state(r)
    def reverse(d):return {pk:min(k for k,v in d.items() if v==pk) for pk in d.values()}
    m={kind:reverse(r.refs[kind]) for kind in ('company','account')}
    m['journal']=journal_names(r)
    m['receipt']=reverse(getattr(r,'sales_refs',{}).get('receipt',{}));m['identity']=reverse({k:v for k,v in r.l2_refs.items() if getattr(r,'l2_types',{}).get(k)=='identity'})
    # Deposit and reconciliation integer PKs overlap; retain the type recorded by each step.
    for kind,op in [('deposit','banking_deposit'),('reconciliation','banking_begin')]:
        names={s['ref'] for s in r.scenario['steps'] if (s['op']==op or (op=='banking_deposit' and s['op']=='bankfeeds_accept' and 'ref' in s)) and s['ref'] in r.banking_refs}
        m[kind]=reverse({k:r.banking_refs[k] for k in names})
    m['line']={l.pk:f"{m['journal'].get(l.journal_id,'?')}:{l.position}" for l in JournalLine.objects.all()}
    return m

def norm(v,m,context=None):
    if isinstance(v,list):return [norm(x,m,context) for x in v]
    if not isinstance(v,dict):return v
    out={}
    fields={'company_id':'company','identity_id':'identity','bank_id':'account','journal_id':'journal','account_id':'account','receipt_id':'receipt','deposit_id':'deposit','undeposited_id':'account','reconciliation_id':'reconciliation','line_id':'line','service_account_id':'account','interest_account_id':'account','service_journal_id':'journal','interest_journal_id':'journal'}
    for k,x in v.items():
        kind=context if k=='id' else fields.get(k)
        if x is not None and kind:out[k]=m[kind].get(x,'?')
        else:out[k]=norm(x,m,'company' if k=='company_snapshot' else None)
    return out

def dump_banking(r):
    state(r);m=maps(r);out={k:[] for k in ('deposits','deposit_lines','reversals','reconciliations','statuses','reconciliation_lines')};out['registers']={};out['screens']={};out['reports']={}
    for model,fields,dest,kind in [(Deposit,DEPOSIT,'deposits','deposit'),(DepositReversal,REVERSAL,'reversals',None),(Reconciliation,REC,'reconciliations','reconciliation')]:
        for obj in model.objects.order_by('pk'):
            v={k:getattr(obj,k) for k in fields}
            for key in ('date','statement_date','request_key','completion_key','undo_key'):
                if key in v and v[key] is not None:v[key]=str(v[key])
            if kind=='reconciliation':
                v['started_by']=obj.started_by.username
                v['completed_by']=obj.completed_by.username if obj.completed_by_id else None
                v['undone_by']=obj.undone_by.username if obj.undone_by_id else None
            out[dest].append(norm(v,m,kind))
    for obj in DepositLine.objects.order_by('deposit_id','receipt_id'):out['deposit_lines'].append(norm({k:getattr(obj,k) for k in ('deposit_id','receipt_id','undeposited_id','amount')},m))
    for obj in ReconciliationLine.objects.order_by('reconciliation_id','journal_line_id'):out['reconciliation_lines'].append(norm({'reconciliation_id':obj.reconciliation_id,'line_id':obj.journal_line_id,'debit':obj.debit,'credit':obj.credit},m))
    for obj in BankLineStatus.objects.order_by('journal_line_id'):out['statuses'].append(norm({'line_id':obj.journal_line_id,'account_id':obj.account_id,'state':obj.state,'revision':obj.revision,'reconciliation_id':obj.reconciliation_id,'changed_by':obj.changed_by.username},m))
    for name,v in r.banking_results.items():
        rows=[]
        for row in v['rows']:
            l=row['line'];item={'id':l.pk,'journal_id':l.journal_id,'account_id':l.account_id,'date':str(l.journal.date),'debit':l.debit,'credit':l.credit,**{k:row[k] for k in ('state','revision','balance','number','memo','party','other','more_splits','splits')}}
            item['id']=m['line'].get(item['id'],'?');rows.append(item)
        raw={k:v[k] for k in ('total','opening','closing','totals','labels')};raw.update(account_id=v['account'].pk,page=v['page'].number,rows=rows);out['registers'][name]=norm(raw,m)
    def read_row(v):return dict(line_id=v['line'].pk,date=str(v['line'].journal.date),**{k:v[k] for k in ('type','number','memo','party','left','right','state','revision')})
    for name,v in r.banking_screens.items():
        d={k:v[k] for k in ('words','totals','total','too_many','cap','hidden_cleared','notes','sign')};d['reconciliation_id']=v['rec'].pk
        for side in ('left','right'):d[side]=[read_row(row) for row in v[side]]
        out['screens'][name]=norm(d,m)
    for name,v in r.banking_reports.items():
        sides=[]
        for side in v['sides']:
            side=dict(side);side['groups']=[{**g,'rows':[read_row(row) for row in g['rows']]} for g in side['groups']];sides.append(side)
        out['reports'][name]=norm(dict(reconciliation_id=v['rec'].pk,words=v['words'],summary=v['summary'],sides=sides),m)
    out=stringify(out)
    import json
    for key in ('deposits','deposit_lines','reversals','reconciliations','statuses','reconciliation_lines'):out[key].sort(key=lambda v:json.dumps(v,separators=(',',':'),ensure_ascii=False))
    return out

def banking_l2(r,v):
    m=maps(r)
    for row in v['snapshots']:
        role=row['role']
        if role.startswith('undeposited:'):row['role']='undeposited:'+m['account'].get(int(role.split(':')[1]),'?')
    from itertools import groupby
    rows=[]
    for _,group in groupby(v['snapshots'],key=lambda row:row['source']):
        group=list(group)
        if any(row['role'].startswith('undeposited:') for row in group):group.sort(key=lambda row:row['role'])
        rows.extend(group)
    v['snapshots']=rows
    return v

def extend_banking_audit(r,ids):
    m=maps(r)
    for k in ('deposit','reconciliation','line'):ids[k]={str(pk):name for pk,name in m[k].items()}

def normalize_banking_audit(v,ids):
    def mapped(x,kind):return None if x is None else ids.get(kind,{}).get(str(x),f'<unknown:{kind}:{x}>')
    action=v['action']
    if action=='document.accounts':
        for row in v['detail']['accounts']:
            if row['role'].startswith('undeposited:'):row['role']='undeposited:'+mapped(row['role'].split(':')[1],'account')
        return False
    if not action.startswith(('deposit.','bank_line.','reconciliation.')):return False
    kind,key=v['target'].split(':',1);v['target']=kind+':'+mapped(key,kind);d=v['detail']
    for key,kind in [('bank_id','account'),('line_id','line'),('journal_id','journal'),('reconciliation_id','reconciliation'),('service_journal_id','journal'),('interest_journal_id','journal')]:
        if key in d:d[key]=mapped(d[key],kind)
    for key,kind in [('line_ids','line'),('kept_journal_ids','journal')]:
        if key in d:d[key]=[mapped(x,kind) for x in d[key]]
    for row in d.get('receipts',[]):row['receipt_id']=mapped(row['receipt_id'],'receipt')
    return True
