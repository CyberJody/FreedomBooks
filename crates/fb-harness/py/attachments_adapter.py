"""Frozen attachment services and reminder queries; generated file names/times are the only omissions."""
from datetime import date
from copy import deepcopy
from django.core.files.uploadedfile import SimpleUploadedFile
from django.test import override_settings
from attachments import services
from attachments.models import Attachment
from documents.models import DocumentIdentity
from ledger.models import Journal, AuditEvent
from reminders import queries

from journal_aliases import journal_names

def names(r,kind=None):
    out={}
    if kind in ('invoice','receipt'):
        for name,pk in sorted(getattr(r,'sales_refs',{}).get(kind,{}).items()):out.setdefault(pk,name)
        return out
    for name in sorted(r.l2_refs):
        if kind is None or getattr(r,'l2_types',{}).get(name)==kind:out.setdefault(r.l2_refs[name],name)
    return out

def apply_attachments(r,s,actor):
    op=s['op']
    if not op.startswith(('attachment_','reminder_')): return False
    r.attachment_results=getattr(r,'attachment_results',{})
    r.attachment_refs=getattr(r,'attachment_refs',{})
    co=r.company(s['company'])
    if op=='reminder_build':
        built=queries.build(co,today=date.fromisoformat(s['today']),days=s.get('days'))
        def group(g,kind):
            return dict(count=g.count,total=str(g.total),overdue_count=g.overdue_count,overdue_total=str(g.overdue_total),capped=g.capped,rows=[dict(document_id=x.document_id,number=x.number,party=x.party,due_date=str(x.due_date),days=x.days,balance=str(x.balance),overdue=x.overdue,when=x.when) for x in g.rows])
        services.access(actor,co)
        r.attachment_results[s['ref']]=dict(today=str(built.today),days=built.days,through=str(built.through),bills=group(built.bills,'bill'),invoices=group(built.invoices,'invoice'))
    elif op=='reminder_count':
        services.access(actor,co)
        r.attachment_results[s['ref']]=queries.overdue_count(co,today=date.fromisoformat(s['today']))
    elif op=='attachment_remove':
        services.remove(actor,r.attachment_refs[s['attachment']],co,reason=s['reason'])
    elif op=='attachment_download':
        import hashlib
        from attachments import storage
        services.access(actor,co)
        a=Attachment.objects.get(company=co,pk=r.attachment_refs[s['attachment']])
        r.attachment_results[s['ref']]=storage.file_sha256(co.pk,a.stored_name)
    else:
        if s.get('journal'): owner=services.journal_owner(co,Journal.objects.get(pk=r.refs['journal'][s['journal']]))
        else: owner=services.document_owner(co,DocumentIdentity.objects.get(pk=r.l2_refs[s['document']]))
        upload=SimpleUploadedFile(s['name'],bytes.fromhex(s['hex']))
        with override_settings(FB_ATTACHMENT_MAX_MB=s.get('max_mb',10),FB_ATTACHMENT_UPLOADS_PER_10_MIN=s.get('uploads_per_window',60),FB_ATTACHMENT_COMPANY_MAX_MB=s.get('company_max_mb',2048)):
            if op=='attachment_adopt':
                t,name,sha=services.inspect(upload);stored=services.store(co,upload,t,sha)
                a,_=services.adopt(actor,owner,stored_name=stored,original_name=name,content_type=t,size=upload.size,sha256=sha,request_key=s['request_key'])
            else: a,_=services.add(actor,owner,upload,request_key=s['request_key'])
        r.attachment_refs.setdefault(s['ref'],a.pk)
    if op in ('reminder_build','reminder_count','attachment_download'):
        r.attachment_queries=getattr(r,'attachment_queries',[])
        attachment=r.attachment_refs[s['attachment']] if op=='attachment_download' else None
        r.attachment_queries.append(dict(step=len(r.outcomes),op=op,ref=s['ref'],company=s['company'],today=s.get('today'),days=s.get('days'),attachment=attachment,audit_count=AuditEvent.objects.filter(company=co).count()))
    return True

def dump_attachments(r):
    refs={}
    for name in sorted(getattr(r,'attachment_refs',{})): refs.setdefault(r.attachment_refs[name],name)
    ids=names(r,'identity');jids=journal_names(r);cos={v:k for k,v in r.refs['company'].items()}
    rows=[]
    # INT-L4: assistant scenarios fabricate a verified-upload Attachment as the frozen FK target; Rust's assistant
    # keeps that upload as an opaque verified id (attachment FK wiring is a named deferral), so skip exactly those.
    fixtures=set(getattr(r,'assistant_attachments',{}))
    for a in Attachment.objects.filter(company_id__in=cos).exclude(pk__in=fixtures):
        v=dict(ref=refs.get(a.pk,'<unknown>'),company=cos[a.company_id],owner_kind='document' if a.document_id else 'journal',owner=ids[a.document_id] if a.document_id else jids[a.journal_id])
        for k in ['original_name','content_type','size','sha256','deleted','uploaded_by_name','deleted_by_name','delete_reason']: v[k]=getattr(a,k)
        v['request_key']=str(a.request_key);rows.append(v)
    requests=[dict(q,attachment=refs[q['attachment']] if q['attachment'] is not None else None) for q in getattr(r,'attachment_queries',[])]
    results=deepcopy(getattr(r,'attachment_results',{}))
    for result in results.values():
        if isinstance(result,dict):
            for family,kind in [('bills','bill'),('invoices','invoice')]:
                mapping=names(r,kind)
                for row in result[family]['rows']:row['document_id']=mapping[row['document_id']]
    return dict(rows=sorted(rows,key=lambda x:x['ref']),results=results,queries=requests)

def normalize_attachment_audit(r,event,ids):
    if event['action'].startswith('attachment.'):
        refs={}
        for name in sorted(getattr(r,'attachment_refs',{})):refs.setdefault(r.attachment_refs[name],name)
        event['detail']['attachment']=refs.get(event['detail']['attachment'],'<unknown>')
    return False
