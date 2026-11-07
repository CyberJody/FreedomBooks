"""Frozen Python service calls and typed normalization for customer prepayments."""
from journal_aliases import journal_names
from prepayments import services as dep, application_services as app, refund_services as refund
from prepayments.models import DepositApplication, DepositRefund
from prepayments.queries import deposit_balances
from sales_adapter import state, payload, track
from l2a import stringify

APP=('deposit_id','invoice_id','receivables_id','liability_id','date','amount','actor_name','journal_id')
REFUND=('deposit_id','customer_id','customer_snapshot','company_snapshot','date','amount','bank_id','liability_id','method','check_number','memo','journal_id','actor_name')

def pp_state(r):
    state(r)
    if not hasattr(r,'pp_refs'):r.pp_refs={'depositapplication':{},'depositrefund':{}};r.pp_reviews={}
def apply_prepayments(r,s,actor):
    if not s['op'].startswith('pp_'):return False
    pp_state(r);op=s['op'];c=r.company(s['company']);v=payload(r,s.get('values',{}));deposit=r.sales_refs['receipt'].get(s.get('deposit'),999999999)
    token=r.pp_reviews.get(s.get('review'),'invalid')
    if op.startswith('pp_review_'):
        f={'pp_review_deposit':dep.review_deposit,'pp_review_application':app.review_application,'pp_review_refund':refund.review_refund}[op]
        args=() if op=='pp_review_deposit' else (deposit,)
        r.pp_reviews[s['review']]=f(actor,c,*args,**v)['token']
    elif op=='pp_record_deposit':
        obj=dep.record_deposit(actor,c,review_token=token,**v);track(r,s['ref'],obj,'receipt')
    elif op in ('pp_apply','pp_record_refund'):
        f,kind=(app.apply_deposit,'depositapplication') if op=='pp_apply' else (refund.record_refund,'depositrefund')
        obj=f(actor,c,deposit,review_token=token,**v);name=s['ref'];r.pp_refs[kind].setdefault(name,obj.pk)
        r.l2_refs.setdefault(name+':identity',obj.identity_id);r.l2_types[name+':identity']='identity';r.refs['journal'].setdefault(name+':posted',obj.journal_id)
    else:
        f,kind={'pp_void_deposit':(dep.void_deposit,'receipt'),'pp_unapply':(app.unapply_deposit,'depositapplication'),'pp_void_refund':(refund.void_refund,'depositrefund')}[op]
        table=r.sales_refs[kind] if kind=='receipt' else r.pp_refs[kind]
        obj=f(actor,c,table.get(s.get('record'),999999999),expected_revision=s.get('expected_revision'),date=s['date'],reason=s['reason'])
        r.refs['journal'].setdefault(s['record']+':reversed',obj.journal_id)
    return True
def extend_pp_audit_ids(r,ids):
    pp_state(r)
    for kind,refs in r.pp_refs.items():ids[kind]={str(pk):min(n for n,v in refs.items() if v==pk) for pk in refs.values()}
def normalize_pp_audit(v,ids):
    if not v['action'].startswith(('customer_deposit.','deposit_refund.')):return False
    kind,raw=v['target'].split(':',1);v['target']=kind+':'+ids.get(kind,{}).get(raw,f'<unknown:{kind}:{raw}>')
    for k,kind in [('destination_account_id','account'),('liability_id','account'),('bank_id','account'),('application_id','depositapplication'),('invoice_id','invoice'),('journal_id','journal')]:
        if k in v['detail']:
            raw=v['detail'][k];v['detail'][k]=ids.get(kind,{}).get(str(raw),f'<unknown:{kind}:{raw}>')
    return True
def dump_prepayments(r):
    pp_state(r)
    def ref(pk,kind):
        if pk is None:return None
        if kind == 'journal':return journal_names(r).get(pk,'?')
        table=r.refs[kind] if kind in r.refs else r.sales_refs[kind] if kind in r.sales_refs else r.pp_refs[kind] if kind in r.pp_refs else {k:v for k,v in r.l2_refs.items() if getattr(r,'l2_types',{}).get(k)==kind}
        return min((n for n,v in table.items() if v==pk),default='?')
    def norm(v):
        out=dict(v)
        for k,x in v.items():
            if x is None:continue
            if k=='deposit_id':out[k]=ref(x,'receipt')
            elif k=='invoice_id':out[k]=ref(x,'invoice')
            elif k=='customer_id':out[k]=ref(x,'customer')
            elif k in ('receivables_id','liability_id','bank_id'):out[k]=ref(x,'account')
            elif k=='journal_id':out[k]=ref(x,'journal')
            elif k in ('customer_snapshot','company_snapshot'):out[k]={**x,'id':ref(x['id'],k.split('_')[0])}
            elif k=='reversal':out[k]=norm(x)
            elif k=='date':out[k]=str(x)
        return out
    out=dict(details=[],applications=[],refunds=[])
    for cref in r.refs['company']:
        c=r.company(cref)
        for row in deposit_balances(c).order_by('date','pk'):
            out['details'].append(norm(dict(ref=ref(row.pk,'receipt'),liability_id=row.prepayment.liability_id,estimate_id=row.prepayment.estimate_id,order_reference=row.prepayment.order_reference,applied=row.applied,refunded=row.refunded,remaining=row.remaining,deposit_state=row.deposit_state)))
        for key,model,fields,kind,revname in [('applications',DepositApplication,APP,'depositapplication','unapplication'),('refunds',DepositRefund,REFUND,'depositrefund','void')]:
            for row in model.objects.filter(company=c).order_by('pk'):
                v={k:getattr(row,k) for k in fields};v['ref']=ref(row.pk,kind)
                if key=='applications':v['invoice_revision']=row.invoice_revision.number
                rev=getattr(row,revname,None);v['reversal']={k:getattr(rev,k) for k in ('date','reason','actor_name','journal_id')} if rev else None
                out[key].append(norm(v))
    return stringify(out)
