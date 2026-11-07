"""Checks/transfer differential adapter. The frozen answer key is never modified."""
from decimal import Decimal
from datetime import date, datetime
from checks import services as checks
from checks.models import Check, CheckRevision, CheckLine
from transfers import services as transfers
from transfers.models import Transfer, TransferVoid
from payables import payment_services
from payables_adapter import track as track_payment

from journal_aliases import journal_names

def resolve(r,v):
    if isinstance(v,list): return [resolve(r,x) for x in v]
    if not isinstance(v,dict): return v
    out={}
    for k,x in v.items():
        if k in ('vendor_id','item_id','check_id','bill_id') and isinstance(x,str):out[k]=r.l2_refs.get(x,x)
        elif k in ('account_id','bank_id','from_account_id','to_account_id') and isinstance(x,str):out[k]=r.refs['account'].get(x,999999999)
        else:out[k]=resolve(r,x)
    return out

def track(r,name,obj,is_check):
    r.l2_refs.setdefault(name,obj.pk);r.l2_types[name]='check' if is_check else 'transfer'
    r.l2_refs.setdefault(name+':source',obj.identity_id);r.l2_types[name+':source']='identity'
    v=obj.current if is_check else obj
    for field,suffix in [('journal_id','journal'),('reversal_id','reversal')]:
        if getattr(v,field,None):r.refs['journal'].setdefault(name+':'+suffix,getattr(v,field))

def apply_checks(r,s,actor):
    op=s['op']
    if not op.startswith(('checks_','transfers_')):return False
    r.checks_reviews=getattr(r,'checks_reviews',{});r.transfers_reviews=getattr(r,'transfers_reviews',{});r.payables_reviews=getattr(r,'payables_reviews',{});r.l2_types=getattr(r,'l2_types',{})
    r.checks_numbers=getattr(r,'checks_numbers',{})
    co=r.company(s['company']);p=resolve(r,s.get('values',{}));expected=s.get('expected_revision')
    if op=='checks_save':track(r,s['ref'],checks.save_check(actor,co,**p),True)
    elif op=='checks_review':r.checks_reviews[s['review']]=checks.review_check(actor,co,r.l2_refs[s['check']],expected_revision=expected)['token']
    elif op=='checks_record':
        cid=r.l2_refs[s['check']];token=r.checks_reviews[s['review']] if 'review' in s else checks.review_check(actor,co,cid,expected_revision=expected)['token']
        track(r,s['check'],checks.record_check(actor,co,cid,expected_revision=expected,review_token=token),True)
    elif op=='checks_void':track(r,s['check'],checks.void_check(actor,co,r.l2_refs[s['check']],expected_revision=expected,date=s['date'],reason=s['reason']),True)
    elif op=='checks_number':track(r,s['check'],checks.assign_number(actor,co,r.l2_refs[s['check']],expected_revision=expected,check_number=s['number']),True)
    elif op=='checks_next':r.checks_numbers[s['ref']]=checks.next_number(co,r.refs['account'][s['bank']])
    elif op=='checks_review_payment':r.payables_reviews[s['review']]=payment_services.review_payment(actor,co,**p)['token']
    elif op=='checks_pay':
        token=r.payables_reviews[s['review']] if 'review' in s else payment_services.review_payment(actor,co,**p)['token']
        track_payment(r,s['ref'],payment_services.record_payment(actor,co,review_token=token,**p),False)
    elif op=='transfers_review':r.transfers_reviews[s['review']]=transfers.review_transfer(actor,co,**p)['token']
    elif op=='transfers_record':
        token=r.transfers_reviews[s['review']] if 'review' in s else transfers.review_transfer(actor,co,**p)['token']
        track(r,s['ref'],transfers.record_transfer(actor,co,review_token=token,**p),False)
    elif op=='transfers_void':
        v=transfers.void_transfer(actor,co,r.l2_refs[s['transfer']],expected_revision=expected,date=s['date'],reason=s['reason']);r.refs['journal'].setdefault(s['transfer']+':reversal',v.journal_id)
    else:raise ValueError(op)
    return True

def names(r):
    out={kind:{} for kind in ['check','transfer','vendor','item','identity']}
    for kind in out:
        for name in sorted(r.l2_refs):
            if getattr(r,'l2_types',{}).get(name)==kind:out[kind].setdefault(r.l2_refs[name],name)
    for kind in ['company','account']:out[kind]={pk:name for name,pk in r.refs[kind].items()}
    out['journal']=journal_names(r)
    out['user']={u.pk:name for name,u in r.users.items()}
    return out

def dump_checks(r):
    n=names(r);out={}
    models=[('check',Check),('checkrevision',CheckRevision),('checkline',CheckLine),('transfer',Transfer),('transfervoid',TransferVoid)]
    data={key:list(model.objects.order_by('pk').values()) for key,model in models}
    for table,rows in data.items():
        n.setdefault(table,{})
        for v in rows:
            if table=='checkrevision':n[table][v['id']]=n['check'][v['document_id']]+':revision:'+str(v['number'])
            if table=='checkline':n[table][v['id']]=n['checkrevision'][v['revision_id']]+':line:'+str(v['position'])
            if table=='transfervoid':n[table][v['id']]=n['transfer'][v['transfer_id']]+':void-record'
    fk={'company_id':'company','identity_id':'identity','vendor_id':'vendor','item_id':'item','document_id':'check','revision_id':'checkrevision','transfer_id':'transfer','journal_id':'journal','reversal_id':'journal','bank_id':'account','account_id':'account','expense_account_id':'account','from_account_id':'account','to_account_id':'account','actor_id':'user'}
    def snap(v,kind):
        if isinstance(v,dict):return {k:n[kind].get(x,'?') if k=='id' and x is not None else n['account'].get(x,'?') if k=='expense_account_id' and x is not None else snap(x,kind) for k,x in v.items()}
        return v
    for table,rows in data.items():
        dest=[]
        for v in rows:
            row={}
            for k,x in v.items():
                if k in ('created_at','creation_digest','review_digest'):continue
                if k=='id':x=n[table].get(x,'?')
                elif x is not None and k in fk:x=n[fk[k]].get(x,'?')
                elif k.endswith('_snapshot'):x=snap(x,{'company_snapshot':'company','payee_snapshot':'vendor','item_snapshot':'item','account_snapshot':'account','bank_snapshot':'account','from_snapshot':'account','to_snapshot':'account'}[k])
                elif isinstance(x,Decimal):x=int(x*(10000 if k=='quantity' else 100))
                elif isinstance(x,(date,datetime)):x=str(x)
                elif k=='print_later':x=int(x)
                row[k]=x
            dest.append(row)
        out[table]=sorted(dest,key=lambda v:v['id'])
    out['numbers']=getattr(r,'checks_numbers',{})
    return out

def extend_checks_audit_ids(r,ids):
    n=names(r)
    for kind in ('check','transfer','journal'):ids[kind]={str(pk):name for pk,name in n[kind].items()}

def normalize_checks_audit(event,ids):
    def mapped(v,kind):return None if v is None else ids.get(kind,{}).get(str(v),f'<unknown:{kind}:{v}>')
    kind,raw=event['target'].split(':',1)
    if kind not in ('check','transfer'):return False
    event['target']=f'{kind}:{mapped(raw,kind)}'
    for field,kind in [('journal_id','journal'),('bank_id','account'),('from_account_id','account'),('to_account_id','account')]:
        if field in event['detail']:event['detail'][field]=mapped(event['detail'][field],kind)
    return True
