"""Sales differential adapter: only calls frozen services, never writes reference source."""
from journal_aliases import journal_names
from decimal import Decimal
from sales import invoice_services as inv, receipt_services as rcp, sales_receipt_services as sr
from sales.models import Invoice, Receipt, ReceiptReversal, Estimate, EstimateConversion
from sales import services as est
from sales.receipt_queries import invoice_balances
from documents.models import DocumentIdentity
from l2a import stringify
HEADER=('number','state','action','reason','actor_name','customer_id','customer_snapshot','company_snapshot','date','due_date','memo','tax_mode','tax_item_id','tax_snapshot','tax_label','tax_rate','subtotal','taxable','tax','total','journal_id','reversal_id')
RECEIPT=('kind','actor_name','customer_id','customer_snapshot','company_snapshot','date','amount','method','reference','memo','destination','destination_account_id','journal_id')
LINE=('position','item_id','item_snapshot','description','quantity','rate','taxable','amount','income_account_id','account_snapshot')
DETAIL=('collected_tax_id','tax_item_id','tax_mode','tax_rate','tax_label','tax_snapshot','subtotal','taxable','tax','total')


def state(r):
    if not hasattr(r,'sales_refs'):
        r.sales_refs={'invoice':{},'receipt':{},'estimate':{}}
        r.sales_reviews={}

def payload(r,v):
    if isinstance(v,list): return [payload(r,x) for x in v]
    if not isinstance(v,dict): return v
    out={}
    for k,x in v.items():
        if x is not None and k in ('customer_id','item_id','tax_item_id'):
            x=r.l2_refs.get(x,999999999)
        elif x is not None and k=='invoice_id': x=r.sales_refs['invoice'].get(x,999999999)
        elif x is not None and (k.endswith('_account_id') or k=='bank_id'): x=r.refs['account'].get(x,999999999)
        else: x=payload(r,x)
        out[k]=x
    return out

def track(r,name,obj,kind):
    r.sales_refs[kind].setdefault(name,obj.pk)
    alias=name+':identity'
    r.l2_refs.setdefault(alias,obj.identity_id)
    r.l2_types=getattr(r,'l2_types',{})
    r.l2_types[alias]='identity'
    h=obj.current if kind in ('invoice','estimate') else obj
    for field,suffix in (('journal_id','posted'),('reversal_id','reversed')):
        key=getattr(h,field,None)
        if key: r.refs['journal'].setdefault(name+':'+suffix,key)

def apply_sales(r,s,actor):
    if not s['op'].startswith('sales_'): return False
    state(r)
    c=r.company(s['company']);v=payload(r,s.get('values',{}));op=s['op'];kind='estimate' if op.endswith('estimate') else 'invoice' if op.endswith('invoice') else 'receipt'
    key=r.sales_refs[kind].get(s.get('record'),999999999)
    expected=s.get('expected_revision')
    if op=='sales_save_estimate':
        v['expected_revision']=s.get('expected_revision',0)
        if 'record' in s:v['estimate_id']=key
        obj=est.save_estimate(actor,c,**v);track(r,s['ref'],obj,'estimate')
    elif op=='sales_transition_estimate':
        obj=est.transition(actor,c,key,state=s['state'],expected_revision=expected,reason=s.get('reason',''));track(r,s['record'],obj,'estimate')
    elif op=='sales_convert_estimate':
        obj=inv.convert_estimate(actor,c,key,expected_revision=expected,date=s['date'],due_date=s['due_date']);track(r,s['ref'],obj,'invoice')
    elif op=='sales_save_invoice':
        v['expected_revision']=s.get('expected_revision',0)
        if 'record' in s:v['invoice_id']=key
        obj=inv.save_invoice(actor,c,**v);track(r,s['ref'],obj,'invoice')
    elif op=='sales_review_invoice':r.sales_reviews[s['review']]=inv.review_invoice(actor,c,key,expected_revision=expected)['token']
    elif op=='sales_post_invoice':
        obj=inv.post_invoice(actor,c,key,expected_revision=expected,review_token=r.sales_reviews.get(s.get('review'),'invalid'));track(r,s['record'],obj,'invoice')
    elif op=='sales_void_invoice':
        obj=inv.void_invoice(actor,c,key,expected_revision=expected,date=s['date'],reason=s['reason']);track(r,s['record'],obj,'invoice')
    elif op in ('sales_review_receipt','sales_review_sale'):
        func=rcp.review_receipt if op=='sales_review_receipt' else sr.review_sales_receipt
        r.sales_reviews[s['review']]=func(actor,c,**v)['token']
    elif op in ('sales_record_receipt','sales_record_sale'):
        func=rcp.record_receipt if op=='sales_record_receipt' else sr.record_sales_receipt
        obj=func(actor,c,review_token=r.sales_reviews.get(s.get('review'),'invalid'),**v);track(r,s['ref'],obj,'receipt')
    elif op in ('sales_reverse_receipt','sales_void_sale'):
        func=rcp.reverse_receipt if op=='sales_reverse_receipt' else sr.void_sales_receipt
        obj=func(actor,c,key,expected_revision=expected,date=s['date'],reason=s['reason'])
        r.refs['journal'].setdefault(s['record']+':reversed',obj.journal_id)
    else:raise ValueError(op)
    return True

def normalize_l2(r,v):
    accounts={pk:ref for ref,pk in r.refs['account'].items()}
    generated_roles=set()
    for row in v['snapshots']:
        source=r.l2_refs.get(row['source'])
        kind=DocumentIdentity.objects.filter(pk=source).values_list('kind',flat=True).first()
        role=row['role']
        if kind in ('SR','RCP'):
            generated_roles.add(row['source'])
        if kind in ('SR','RCP') and ':' in role:
            prefix,key=role.split(':',1);row['role']=prefix+':'+accounts.get(int(key),'?')
    for source in generated_roles:
        indices=[i for i,row in enumerate(v['snapshots']) if row['source']==source]
        group=sorted((v['snapshots'][i] for i in indices),key=lambda row:__import__('json').dumps(row['role']))
        for i,row in zip(indices,group):v['snapshots'][i]=row
    generated=set(EstimateConversion.objects.values_list('invoice__identity_id',flat=True))
    generated={name for name,pk in r.l2_refs.items() if pk in generated and getattr(r,'l2_types',{}).get(name)=='identity'}
    for identity in v['identities']:
        if identity['id'] in generated:identity['request_key']='generated-estimate-conversion'
    return v

def dump_sales(r):
    state(r)
    def ref(key,kind):
        if key is None:return None
        if kind == 'journal':return journal_names(r).get(key,'?')
        if kind in ('company','account'):table=r.refs[kind]
        elif kind in ('invoice','receipt','estimate'):table=r.sales_refs[kind]
        else: table={k:v for k,v in r.l2_refs.items() if getattr(r,'l2_types',{}).get(k)==kind}
        return min((name for name,pk in table.items() if pk==key),default='?')
    def tax(v):
        if isinstance(v,list):return [tax(x) for x in v]
        if isinstance(v,dict):return {k:ref(x,'vendor' if k=='agency_id' else 'tax') if k in ('id','agency_id') and x is not None else tax(x) for k,x in v.items()}
        return v
    def norm(v):
        out=dict(v)
        for k,x in list(out.items()):
            if x is None:continue
            if k=='customer_snapshot':out[k]={**x,'id':ref(x['id'],'customer')}
            elif k=='company_snapshot':out[k]={**x,'id':ref(x['id'],'company')}
            elif k=='item_snapshot':out[k]={**x,'id':ref(x['id'],'item')} if x else {}
            elif k=='account_snapshot':out[k]={role:{**a,'id':ref(a['id'],'account')} for role,a in x.items()}
            elif k=='tax_snapshot':out[k]=tax(x)
            elif k=='tax_item_id':out[k]=ref(x,'tax')
            elif k=='invoice_id':out[k]=ref(x,'invoice')
            elif k=='customer_id':out[k]=ref(x,'customer')
            elif k=='item_id':out[k]=ref(x,'item')
            elif k in ('journal_id','reversal_id'):out[k]=ref(x,'journal')
            elif k.endswith('_account_id') or k in ('receivables_id','collected_tax_id'):out[k]=ref(x,'account')
            elif k in ('sale','reversal'):out[k]=norm(x)
            elif k in ('lines','allocations'):out[k]=[norm(l) for l in x]
            elif k in ('date','due_date','expiry'):out[k]=str(x)
        return out
    out=dict(estimates=[],invoices=[],receipts=[],balances=[])
    for cref in r.refs['company']:
        c=r.company(cref)
        for obj in Estimate.objects.filter(company=c).order_by('pk'):
            history=[]
            for h in obj.history.order_by('number'):
                row={k:getattr(h,k) for k in HEADER if k not in ('due_date','journal_id','reversal_id')}
                row['expiry']=h.expiry
                row['lines']=[{k:getattr(l,k) for k in LINE+('expense_account_id',)} for l in h.lines.all()]
                history.append(norm(row))
            link=EstimateConversion.objects.filter(estimate=obj).first()
            out['estimates'].append(dict(ref=ref(obj.pk,'estimate'),revision=obj.revision,history=history,conversion=dict(revision=link.revision.number,invoice=ref(link.invoice_id,'invoice')) if link else None))
        for obj in Invoice.objects.filter(company=c).order_by('pk'):
            history=[]
            for h in obj.history.order_by('number'):
                row={k:getattr(h,k) for k in HEADER}
                row['lines']=[{k:getattr(l,k) for k in LINE+('expense_account_id',)} for l in h.lines.all()]
                history.append(norm(row))
            out['invoices'].append(dict(ref=ref(obj.pk,'invoice'),revision=obj.revision,history=history))
        for obj in Receipt.objects.filter(company=c).order_by('pk'):
            row={k:getattr(obj,k) for k in RECEIPT};row.update(ref=ref(obj.pk,'receipt'),sale=None,lines=[],allocations=[],reversal=None)
            if obj.kind=='SR':
                row['sale']={k:getattr(obj.sale,k) for k in DETAIL};row['lines']=[{k:getattr(l,k) for k in LINE} for l in obj.sale.lines.all()]
            row['allocations']=[dict(invoice_id=a.invoice_id,revision=a.invoice_revision.number,receivables_id=a.receivables_id,amount=a.amount) for a in obj.allocations.all()]
            rev=ReceiptReversal.objects.filter(receipt=obj).first()
            if rev:row['reversal']={k:getattr(rev,k) for k in ('date','reason','actor_name','journal_id')}
            out['receipts'].append(norm(row))
        for h in invoice_balances(c).order_by('invoice_id'):
            out['balances'].append(norm({k:getattr(h,k) for k in ('invoice_id','customer_id','date','due_date','state','total','allocated','credited','deposited','settled','outstanding','payment_state')}))
    return stringify(out)

def sales_audit_maps(r, ids):
    state(r)
    for kind in ('invoice','receipt','estimate'):
        ids[kind]={str(pk):min(name for name,val in r.sales_refs[kind].items() if val==pk) for pk in r.sales_refs[kind].values()}
    ids['estimate_conversion_sources']={str(pk):str(key) for pk,key in EstimateConversion.objects.values_list('invoice__identity_id','invoice__identity__request_key')}
    ids['journal']={str(pk):name for pk,name in journal_names(r).items()}
    ids['sales_account_roles']={str(pk):kind for pk,kind in DocumentIdentity.objects.filter(kind__in=('SR','RCP')).values_list('pk','kind')}
    return ids

def normalize_sales_audit(event, ids):
    kind, raw=event['target'].split(':',1)
    def field(row,key,entity):
        if key in row and row[key] is not None:
            val=row[key];row[key]=ids.get(entity,{}).get(str(val),f'<unknown:{entity}:{val}>')
    if kind=='documentidentity' and raw in ids['estimate_conversion_sources'] and event['detail'].get('request_key')==ids['estimate_conversion_sources'][raw] and event['action']=='document.identified':event['detail']['request_key']='generated-estimate-conversion'
    if kind not in ('invoice','receipt','estimate'):
        if kind=='documentidentity' and raw in ids['sales_account_roles'] and event['action']=='document.accounts':
            for row in event['detail']['accounts']:
                role=row['role']
                if ':' in role:
                    prefix,key=role.split(':',1)
                    if key in ids['account']:row['role']=prefix+':'+ids['account'][key]
        return False
    event['target']=kind+':'+ids[kind].get(raw,f'<unknown:{raw}>')
    field(event['detail'],'journal_id','journal')
    field(event['detail'],'estimate_id','estimate')
    if event['action']=='invoice.converted':event['detail']['conversion_id']='conversion:'+event['detail']['estimate_id']
    for row in event['detail'].get('allocations',[]):field(row,'invoice_id','invoice')
    for row in event['detail'].get('lines',[]):field(row,'income_account_id','account')
    return True
