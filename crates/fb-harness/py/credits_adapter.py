"""Credits adapter. Commands always call the frozen reference services."""
from journal_aliases import journal_names
from decimal import Decimal
from datetime import date, datetime
from uuid import UUID
from credits import services, application_services as apps, refund_services as refunds
from credits import models
from documents.models import DocumentIdentity

TABLES={name:getattr(models,cls) for name,cls in [
 ('creditmemo','CreditMemo'),('creditmemoline','CreditMemoLine'),('creditmemovoid','CreditMemoVoid'),
 ('creditapplication','CreditApplication'),('creditapplicationline','CreditApplicationLine'),
 ('creditunapplication','CreditUnapplication'),('creditrefund','CreditRefund'),('creditrefundvoid','CreditRefundVoid')]}


def resolve(r,v):
    if isinstance(v,list):return [resolve(r,x) for x in v]
    if not isinstance(v,dict):return v
    out={}
    for k,x in v.items():
        if isinstance(x,str) and k=='invoice_id':out[k]=getattr(r,'sales_refs',{}).get('invoice',{}).get(x,999999999)
        elif isinstance(x,str) and k in ('customer_id','item_id','tax_item_id'):out[k]=r.l2_refs.get(x,x)
        elif isinstance(x,str) and k in ('income_account_id','bank_id'):out[k]=r.refs['account'].get(x,999999999)
        else:out[k]=resolve(r,x)
    return out

def apply_credits(r,s,actor):
    op=s['op']
    if not op.startswith('credits_'):return False
    r.credits_reviews=getattr(r,'credits_reviews',{})
    co=r.company(s['company']);v=resolve(r,s.get('values',{}));key=r.l2_refs.get(s.get('record'),s.get('record'));expected=s.get('expected_revision')
    if op.startswith('credits_review_'):
        fn={'credits_review_memo':services.review_credit_memo,'credits_review_application':apps.review_application,'credits_review_refund':refunds.review_refund}[op]
        r.credits_reviews[s['review']]=fn(actor,co,*([] if op=='credits_review_memo' else [key]),**v)['token']
    elif op in ('credits_record_memo','credits_apply','credits_refund'):
        fn,kind={'credits_record_memo':(services.record_credit_memo,'creditmemo'),'credits_apply':(apps.apply_credit,'creditapplication'),'credits_refund':(refunds.record_refund,'creditrefund')}[op]
        obj=fn(actor,co,*([] if op=='credits_record_memo' else [key]),review_token=r.credits_reviews.get(s.get('review'),'invalid'),**v)
        name=s['ref'];r.l2_refs.setdefault(name,obj.pk);r.l2_types[name]=kind
        if hasattr(obj,'identity_id'):r.l2_refs.setdefault(name+':source',obj.identity_id);r.l2_types[name+':source']='identity'
        if hasattr(obj,'journal_id') and obj.journal_id not in r.refs['journal'].values():r.refs['journal'].setdefault(name+':journal',obj.journal_id)
    else:
        fn={'credits_void_memo':services.void_credit_memo,'credits_unapply':apps.unapply_credit,'credits_void_refund':refunds.void_refund}[op]
        obj=fn(actor,co,key,expected_revision=expected,date=s['date'],reason=s['reason'])
        if hasattr(obj,'journal_id') and obj.journal_id not in r.refs['journal'].values():r.refs['journal'].setdefault(s['record']+':reversal',obj.journal_id)
    return True

def names(r):
    from sales.models import Invoice,InvoiceRevision
    from directory.models import Customer,Item,Vendor
    from salestax.models import TaxItem
    n={kind:{pk:name for name,pk in r.refs[kind].items()} for kind in ('company','account')}
    n['journal']=journal_names(r)
    n['user']={u.pk:name for name,u in r.users.items()}
    for kind,model,typ in [('creditmemo',models.CreditMemo,'creditmemo'),('creditapplication',models.CreditApplication,'creditapplication'),('creditrefund',models.CreditRefund,'creditrefund'),('invoice',Invoice,'invoice'),('customer',Customer,'customer'),('item',Item,'item'),('taxitem',TaxItem,'tax'),('vendor',Vendor,'vendor'),('identity',DocumentIdentity,'identity')]:
        n[kind]={}
        for rank,pk in enumerate(model.objects.order_by('pk').values_list('pk',flat=True)):
            aliases=getattr(r,'sales_refs',{}).get('invoice',{}) if kind=='invoice' else {name:key for name,key in r.l2_refs.items() if r.l2_types.get(name)==typ}
            n[kind][pk]=min((name for name,key in aliases.items() if key==pk),default=f'unreferenced-{kind}:{rank}')
    for kind,model,parent_field,parent_kind,suffix in [
      ('creditmemovoid',models.CreditMemoVoid,'credit_memo_id','creditmemo','void'),
      ('creditrefundvoid',models.CreditRefundVoid,'refund_id','creditrefund','void'),
      ('creditunapplication',models.CreditUnapplication,'application_id','creditapplication','unapply'),
      ('creditmemoline',models.CreditMemoLine,'credit_memo_id','creditmemo','line'),
      ('creditapplicationline',models.CreditApplicationLine,'application_id','creditapplication','invoice'),
      ('invoicerevision',InvoiceRevision,'invoice_id','invoice','revision')]:
        n[kind]={}
        for row in model.objects.order_by('pk').values():
            extra=''
            if kind=='creditmemoline':extra=':'+str(row['position'])
            if kind=='creditapplicationline':extra=':'+n['invoice'][row['invoice_id']]
            if kind=='invoicerevision':extra=':'+str(row['number'])
            n[kind][row['id']]=n[parent_kind][row[parent_field]]+':'+suffix+extra
    return n

FK={'company_id':'company','identity_id':'identity','customer_id':'customer','item_id':'item','tax_item_id':'taxitem','income_account_id':'account','receivables_id':'account','collected_tax_id':'account','bank_id':'account','journal_id':'journal','actor_id':'user','credit_memo_id':'creditmemo','application_id':'creditapplication','refund_id':'creditrefund','invoice_id':'invoice','invoice_revision_id':'invoicerevision'}
def norm(v,kind,n):
    out={}
    def ref(x,k):return None if x is None else n[k].get(x,f'<unknown:{k}:{x}>')
    def tax(x):
        if isinstance(x,list):return [tax(v) for v in x]
        if isinstance(x,dict):return {k:ref(v,'taxitem' if k=='id' else 'vendor') if k in ('id','agency_id') else tax(v) for k,v in x.items()}
        return x
    for k,x in v.items():
        if k in ('created_at','writer_xid','creation_digest','review_digest'):continue
        if k=='id':x=ref(x,kind)
        elif k in FK:x=ref(x,FK[k])
        elif k=='tax_snapshot':x=tax(x)
        elif k=='account_snapshot':x={role:{**a,'id':ref(a['id'],'account')} for role,a in x.items()}
        elif k in ('customer_snapshot','company_snapshot','item_snapshot') and x:x={**x,'id':ref(x['id'],{'customer_snapshot':'customer','company_snapshot':'company','item_snapshot':'item'}[k])}
        elif isinstance(x,(Decimal,date,datetime,UUID)):x=str(x)
        out[k]=x
    return out

def dump_credits(r):
    n=names(r);out={}
    for kind,model in TABLES.items():out[kind]=sorted((norm(v,kind,n) for v in model.objects.order_by('pk').values()),key=lambda v:v['id'])
    from credits.queries import memo_balances
    out['balances']=[]
    for ref,pk in r.refs['company'].items():
        for m in memo_balances(r.company(ref)):
            out['balances'].append(dict(id=n['creditmemo'][m.pk],company_id=ref,customer_id=n['customer'][m.customer_id],total=str(m.total),applied=str(m.applied),refunded=str(m.refunded),available=str(m.available),credit_state=m.credit_state))
    out['balances'].sort(key=lambda v:v['id'])
    return out

def credits_l2(r,v):
    accounts={pk:ref for ref,pk in r.refs['account'].items()}
    sources=set(models.CreditMemo.objects.values_list('identity_id',flat=True));groups=set()
    for row in v['snapshots']:
        if r.l2_refs.get(row['source']) not in sources:continue
        groups.add(row['source'])
        if row['role'].startswith('income:') and row['role'].split(':',1)[1].isdigit():row['role']='income:'+accounts[int(row['role'].split(':',1)[1])]
    for source in groups:
        indices=[i for i,row in enumerate(v['snapshots']) if row['source']==source]
        group=sorted((v['snapshots'][i] for i in indices),key=lambda v:v['role'])
        for i,row in zip(indices,group):v['snapshots'][i]=row
    return v

def extend_credits_audit_ids(r,ids):
    n=names(r)
    for kind in ('creditmemo','creditapplication','creditrefund','invoice','journal'):ids[kind]={str(k):v for k,v in n[kind].items()}
    ids['credits_sources']={str(pk):'' for pk in models.CreditMemo.objects.values_list('identity_id',flat=True)}

def normalize_credits_audit(event,ids):
    def mapped(v,kind):return None if v is None else ids.get(kind,{}).get(str(v),f'<unknown:{kind}:{v}>')
    if event['action']=='document.accounts' and event['target'].split(':',1)[1] in ids['credits_sources']:
        for a in event['detail']['accounts']:
            if a['role'].startswith('income:'):a['role']='income:'+mapped(a['role'].split(':',1)[1],'account')
        event['detail']['accounts'].sort(key=lambda a:a['role'])
    kind,raw=event['target'].split(':',1)
    if kind not in ('creditmemo','creditrefund'):return False
    event['target']=f'{kind}:{mapped(raw,kind)}'
    d=event['detail']
    for k,kind in [('journal_id','journal'),('receivables_id','account'),('bank_id','account'),('application_id','creditapplication')]:
        if k in d:d[k]=mapped(d[k],kind)
    for k,field,kind in [('lines','income_account_id','account'),('invoices','invoice_id','invoice')]:
        for row in d.get(k,[]):row[field]=mapped(row[field],kind)
    return True
