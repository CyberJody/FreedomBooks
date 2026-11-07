"""Payables adapter: all accounting commands execute the frozen Python services."""
from decimal import Decimal
from datetime import date,datetime
from payables import services,payment_services
from payables.models import Bill,BillRevision,BillLine,BillPayment,PaymentAllocation,PaymentReversal
from ledger.models import Journal

from journal_aliases import journal_names

def resolve(r,v):
    if isinstance(v,list): return [resolve(r,x) for x in v]
    if not isinstance(v,dict): return v
    out={}
    for k,x in v.items():
        if k in ('vendor_id','item_id','bill_id','supersedes_id') and isinstance(x,str): out[k]=r.l2_refs.get(x,x)
        elif k in ('expense_account_id','bank_id') and isinstance(x,str): out[k]=r.refs['account'].get(x,999999999)
        else: out[k]=resolve(r,x)
    return out

def track(r,name,obj,is_bill):
    r.l2_refs.setdefault(name,obj.pk);r.l2_types[name]='bill' if is_bill else 'billpayment'
    r.l2_refs.setdefault(name+':source',obj.identity_id);r.l2_types[name+':source']='identity'
    v=obj.current if is_bill else obj
    for field,suffix in [('journal_id','journal'),('reversal_id','reversal')]:
        if getattr(v,field,None): r.refs['journal'].setdefault(name+':'+suffix,getattr(v,field))

def apply_payables(r,s,actor):
    op=s['op']
    if not op.startswith('payables_'): return False
    r.payables_reviews=getattr(r,'payables_reviews',{});r.l2_types=getattr(r,'l2_types',{})
    co=r.company(s['company']);p=resolve(r,s.get('values',{}));expected=s.get('expected_revision')
    if op=='payables_receipt':
        from types import SimpleNamespace
        from assistant.receipts import form_initial
        from directory.models import Vendor
        vendor=Vendor.objects.get(company=co,pk=r.l2_refs[s['vendor']])
        fields={**s['fields'],'vendor_name':vendor.display_name}
        ai=form_initial(SimpleNamespace(result=fields,file_name='synthetic-receipt'),co)
        h=ai['initial']
        lines=[dict(description=l['description'],quantity=l['quantity'],rate=l['rate'],expense_account_id=l['expense_account']) for l in ai['lines']]
        track(r,s['ref'],services.save_bill(actor,co,vendor_id=vendor.pk,date=str(h['date']),due_date=str(h['due_date']),reference=h['reference'],memo='',request_key=s['request_key'],lines=lines),True)
    elif op=='payables_save': track(r,s['ref'],services.save_bill(actor,co,**p),True)
    elif op=='payables_review': r.payables_reviews[s['review']]=services.review_bill(actor,co,r.l2_refs[s['bill']],expected_revision=expected)['token']
    elif op=='payables_post':
        bid=r.l2_refs[s['bill']]
        token=r.payables_reviews[s['review']] if 'review' in s else services.review_bill(actor,co,bid,expected_revision=expected)['token']
        track(r,s['bill'],services.post_bill(actor,co,bid,expected_revision=expected,review_token=token),True)
    elif op=='payables_void': track(r,s['bill'],services.void_bill(actor,co,r.l2_refs[s['bill']],expected_revision=expected,date=s['date'],reason=s['reason']),True)
    elif op=='payables_review_payment': r.payables_reviews[s['review']]=payment_services.review_payment(actor,co,**p)['token']
    elif op=='payables_pay':
        token=r.payables_reviews[s['review']] if 'review' in s else payment_services.review_payment(actor,co,**p)['token']
        track(r,s['ref'],payment_services.record_payment(actor,co,review_token=token,**p),False)
    elif op=='payables_reverse':
        rev=payment_services.reverse_payment(actor,co,r.l2_refs[s['payment']],expected_revision=expected,date=s['date'],reason=s['reason'])
        r.refs['journal'].setdefault(s['payment']+':reversal',rev.journal_id)
    else: raise ValueError(op)
    return True

def names(r):
    out={}
    for kind in ['bill','billpayment','vendor','item','identity']:
        out[kind]={}
        for name in sorted(r.l2_refs):
            if getattr(r,'l2_types',{}).get(name)==kind: out[kind].setdefault(r.l2_refs[name],name)
    for kind in ['company','account']: out[kind]={pk:name for name,pk in r.refs[kind].items()}
    out['journal']=journal_names(r)
    out['user']={u.pk:name for name,u in r.users.items()}
    return out

def dump_payables(r):
    n=names(r);out={}
    models=[('bill',Bill),('billrevision',BillRevision),('billline',BillLine),('billpayment',BillPayment),('paymentallocation',PaymentAllocation),('paymentreversal',PaymentReversal)]
    data={key:list(model.objects.order_by('pk').values()) for key,model in models}
    for table,rows in data.items():
        n.setdefault(table,{})
        for v in rows:
            if table=='billrevision': n[table][v['id']]=n['bill'][v['bill_id']]+':revision:'+str(v['number'])
            if table=='billline': n[table][v['id']]=n['billrevision'][v['revision_id']]+':line:'+str(v['position'])
            if table=='paymentallocation': n[table][v['id']]=n['billpayment'][v['payment_id']]+':allocation:'+n['bill'][v['bill_id']]
            if table=='paymentreversal': n[table][v['id']]=n['billpayment'][v['payment_id']]+':reversal-record'
    fk={'company_id':'company','identity_id':'identity','vendor_id':'vendor','item_id':'item','bill_id':'bill','supersedes_id':'bill','revision_id':'billrevision','bill_revision_id':'billrevision','payment_id':'billpayment','journal_id':'journal','reversal_id':'journal','bank_id':'account','payables_id':'account','income_account_id':'account','expense_account_id':'account','actor_id':'user'}
    def snap(v,kind):
        if isinstance(v,dict): return {k:n[kind].get(x,'?') if k=='id' and x is not None else snap(x,'account' if k in ['income_account','expense_account'] else kind) for k,x in v.items()}
        return v
    for table,rows in data.items():
        dest=[]
        for v in rows:
            row={}
            for k,x in v.items():
                if k in ('created_at','creation_digest','review_digest'): continue
                if k=='id': x=n[table].get(x,'?')
                elif x is not None and k in fk: x=n[fk[k]].get(x,'?')
                elif k.endswith('_snapshot'): x=snap(x,{'company_snapshot':'company','vendor_snapshot':'vendor','item_snapshot':'item','account_snapshot':'account'}[k])
                elif isinstance(x,Decimal): x=int(x*(10000 if k=='quantity' else 100))
                elif isinstance(x,(date,datetime)): x=str(x)
                row[k]=x
            dest.append(row)
        out[table]=dest
    from payables.queries import current_bills
    from payables.bill_listing import compute_totals, scope_filters, apply_state_filter
    from payables.calendar import build_month
    from ledger.models import Company
    from django.contrib.auth import get_user_model
    from django.db.models import Q
    from dataclasses import asdict,is_dataclass
    def plain(v):
        if is_dataclass(v):return plain(asdict(v))
        if isinstance(v,dict):return {str(k):plain(x) for k,x in v.items()}
        if isinstance(v,(list,tuple)):return [plain(x) for x in v]
        if isinstance(v,Decimal):return f'{v:.2f}'
        if isinstance(v,date):return str(v)
        return v
    out['balances']=[];out['tracker']=[];out['calendar']=[]
    for co in Company.objects.all():
        if not get_user_model().objects.filter(Q(is_staff=True)|Q(pk__in=co.members.values('pk')),is_active=True).exists():continue
        for b in current_bills(co):
            paid_on=PaymentAllocation.objects.filter(bill_id=b.bill_id,payment__reversal__isnull=True).order_by('-payment__date').values_list('payment__date',flat=True).first()
            out['balances'].append(plain(dict(company_id=n['company'][co.pk],bill_id=n['bill'][b.bill_id],payment_state=b.payment_state,allocated=b.allocated,outstanding=b.outstanding,paid_on=paid_on)))
        filtered=[]
        for spec in r.scenario.get('payables_tracker_filters',[]):
            filters=dict(spec['filters'])
            if filters.get('vendor'): filters['vendor']=r.l2_refs[filters['vendor']]
            selected=apply_state_filter(scope_filters(current_bills(co),filters),filters.get('state'),date(2026,11,23))
            filtered.append(dict(name=spec['name'],bill_ids=sorted(n['bill'][b.bill_id] for b in selected),totals=plain(compute_totals(co,filters,today=date(2026,11,23)))))
        out['tracker'].append(dict(company_id=n['company'][co.pk],totals=plain(compute_totals(co,{},today=date(2026,11,23))),filtered=filtered))
        cal=plain(build_month(co,2026,11,today=date(2026,11,23),include_paid=True))
        def ids(v):
            if isinstance(v,list):return [ids(x) for x in v]
            if isinstance(v,dict):return {k:n['bill' if k=='bill_id' else 'vendor'].get(x,'?') if k in ['bill_id','vendor_id'] else ids(x) for k,x in v.items()}
            return v
        out['calendar'].append(dict(company_id=n['company'][co.pk],data=ids(cal)))
    for key in ['balances','tracker','calendar']:out[key].sort(key=lambda v:(v['company_id'],v.get('bill_id','')))
    return out

def payables_l2(r,v):
    n=names(r)['account']
    for row in v['snapshots']:
        if row['role'].startswith('payables:'): row['role']='payables:'+n[int(row['role'].split(':',1)[1])]
    return v


def extend_payables_audit_ids(r, ids):
    n = names(r)
    for kind in ('bill', 'billpayment', 'journal'):
        ids[kind] = {str(pk): name for pk, name in n[kind].items()}


def normalize_payables_audit(event, ids):
    def mapped(value, kind):
        return None if value is None else ids.get(kind, {}).get(str(value), f'<unknown:{kind}:{value}>')
    if event['action'] == 'document.accounts':
        for account in event['detail']['accounts']:
            if account['role'].startswith('payables:'):
                account['role'] = 'payables:' + mapped(account['role'].split(':', 1)[1], 'account')
    kind, raw = event['target'].split(':', 1)
    if kind not in ('bill', 'billpayment'):
        return False
    event['target'] = f"{kind}:{mapped(raw, kind)}"
    for field, kind in [('journal_id', 'journal'), ('supersedes_id', 'bill')]:
        if field in event['detail']:
            event['detail'][field] = mapped(event['detail'][field], kind)
    for allocation in event['detail'].get('allocations', []):
        allocation['bill_id'] = mapped(allocation['bill_id'], 'bill')
    return True
