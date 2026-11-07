"""Offline calls to the frozen assistant core: no provider and no receipt bytes."""
import base64
from datetime import datetime, timezone, date
from types import SimpleNamespace
from contextlib import ExitStack
from io import BytesIO
from uuid import UUID
from assistant.client import AIError
from unittest.mock import patch
from django.test import override_settings
from django.db import transaction
from django.core.exceptions import ValidationError, PermissionDenied
from assistant import receipts, services
from assistant.models import AIConnection, ReceiptReading
from ledger.services import access
from payables_adapter import resolve, track

CLIENT_FAILURES = {
    'invalid_address': 'The OpenAI address could not be read.',
    'missing_host': 'The OpenAI address names no host.',
    'insecure_address': 'OpenAI addresses must use HTTPS.',
    'address_credentials': 'OpenAI addresses never carry credentials.',
    'redirect': 'OpenAI answered with a redirect; redirects are not followed.',
    'certificate': 'The OpenAI server certificate could not be verified.',
    'timeout': 'OpenAI did not answer in time. Try again later.',
    'unreachable': 'OpenAI could not be reached.',
    'response_unreadable': 'The OpenAI response could not be read.',
    'response_oversized': 'The OpenAI response was larger than allowed.',
    'token_incomplete': 'The token response was incomplete.',
    'metadata_no_key_set': 'OpenAI metadata names no key set.',
    'metadata_no_keys': 'OpenAI metadata holds no keys.',
    'invalid_model_list': 'OpenAI returned something that is not a model list.',
    'invalid_responses_request': 'A Responses request needs a model and an input list.',
    'stream_interrupted': 'The OpenAI stream was interrupted.',
    'stream_oversized': 'The OpenAI stream was larger than allowed.',
}

class PausedReading(Exception):
    """Stop at the provider boundary, retaining Python's pending claim unchanged."""
    def __init__(self, reading):
        self.reading = reading

def pause_reading(actor, company, connection, reading, *args, **kwargs):
    raise PausedReading(reading)

DEMO_FERNET = base64.urlsafe_b64encode(b"0" * 32).decode("ascii")

def staff(actor, co):
    access(actor, co)
    if not (actor.is_staff or actor.is_superuser):
        raise PermissionDenied('Staff access required.')

def apply_assistant(r, s, actor):
    op=s['op']
    if not op.startswith('assistant_'): return False
    r.assistant_results=getattr(r,'assistant_results',{})
    r.l2_types=getattr(r,'l2_types',{})
    co=r.company(s['company']); now=s.get('now',1792944000)
    at=datetime.fromtimestamp(now,tz=timezone.utc)
    def reading():
        access(actor,co)
        try: return ReceiptReading.objects.get(company=co,pk=r.l2_refs.get(s['reading'],-1))
        except ReceiptReading.DoesNotExist: raise ValidationError('Not a reading of this company.')
    try:
      with override_settings(FB_SECRETS_KEYS=(DEMO_FERNET,)), patch('django.utils.timezone.now', return_value=at):
        if op=='assistant_connect':
            staff(actor,co); state=s['state'];mode=state['mode'];choices=state['model_choices']
            with transaction.atomic():
                row=services._locked(co,actor)
                if mode=='api_key':
                    services._write(row,{'api_key':'synthetic-offline-key'});services._wipe_fields_keep_cipher(row)
                    row.mode=mode;row.status='connected';row.key_last4=state.get('key_last4','')
                    row.model_choices=choices;row.model=services._pick_model('',choices)
                    action='ai.key_connected';detail={'mode':mode}
                else:
                    services._write(row,{'subject':'synthetic-offline-subject'})
                    row.mode=mode;row.status='connected';row.account_email=state.get('account_email','')[:254];row.key_last4=''
                    row.model_choices=choices;row.model=services._pick_model(row.model,choices)
                    row.plan_usage=True;row.notice_pending=not row.notice_shown
                    action='ai.connected';detail={'mode':mode,'registration':state.get('registration',False),'models':len(choices)}
                row.last_error=CLIENT_FAILURES[state['models_error']][:300] if state.get('models_error') else '';row.revoke_confirmed=None;row.token_expires_at=None;row.save();services._audit(actor,co,action,detail)
        elif op=='assistant_switch':
            staff(actor,co);services.set_read_attachments(actor,co,s['enabled'])
        elif op=='assistant_model':
            staff(actor,co);services.set_model(actor,co,s['model'])
        elif op=='assistant_models':
            staff(actor,co)
            with patch('assistant.services.refresh_if_needed',return_value='synthetic-no-network'),patch('assistant.client.list_models',return_value=s['choices']):
                services.refresh_models(actor,co)
        elif op=='assistant_dismiss':
            staff(actor,co);services.dismiss_notice(actor,co)
        elif op=='assistant_disconnect':
            staff(actor,co)
            # Frozen disconnect calls revoke only for a retained refresh token; provide a stub.
            row=services.connection_for(co)
            if row and row.mode=='siwc' and row.secret_cipher is not None:
                services._write(row,{'refresh_token':'synthetic','client_id':'synthetic'});row.save()
            with patch('assistant.client.revoke',return_value=s.get('revoked')):
                services.disconnect(actor,co)
        elif op=='assistant_needs_signin':
            access(actor,co);row=services.connection_for(co)
            if row is None or row.status!='connected' or row.secret_cipher is None: raise ValidationError('Nothing is connected.')
            action=s.get('action','ai.call_failed');code=s.get('code','unauthorized')
            if action not in ('ai.call_failed','ai.refresh_failed') or code not in ('unauthorized','invalid_grant','plan_not_granted'):
                raise ValidationError('Invalid assistant sign-in failure.')
            message=services.PLAN_SCOPE_LOST if code=='plan_not_granted' else services.KEY_REJECTED_MESSAGE if row.mode=='api_key' else services.NEEDS_SIGNIN_MESSAGE
            services._mark_needs_signin(row.pk,actor,action,code,message=message)
        elif op=='assistant_metadata':
            access(actor,co)
            AIConnection.objects.filter(company=co).update(token_expires_at=datetime.fromtimestamp(s['expires'],tz=timezone.utc) if s.get('expires') is not None else None,**({'last_used_at':at} if s.get('used') else {}))
        elif op=='assistant_parse':
            import json
            access(actor,co)
            raw=s['raw'] if isinstance(s['raw'],str) else json.dumps(s['raw'])
            r.assistant_results[s['ref']]=receipts.parse_reply(raw,co,today=date.fromisoformat(s.get('today','2026-10-30')))
        elif op=='assistant_begin':
            # Call the frozen entry points. Pause at the provider boundary so later
            # harness steps can simulate a reply, a crash, expiry or a retry.
            # File bytes/storage/sniffing are synthetic; eligibility/order/claims are Python's.
            access(actor,co)
            file=s['file'];data=b'x'*file['size']
            stored=UUID(file['source_file_id']).hex+'.png'
            with ExitStack() as stubs:
                stubs.enter_context(patch('assistant.receipts._run',side_effect=pause_reading))
                stubs.enter_context(patch('assistant.receipts.storage.remove_unreferenced'))
                if file.get('bill_id'):
                    from attachments.models import Attachment
                    from payables.models import Bill
                    bill=Bill.objects.filter(company=co,pk=r.l2_refs.get(file['bill_id'])).first()
                    attachment=SimpleNamespace(company_id=co.pk,deleted=False,document_id=None)
                    if bill is not None:
                        attachment,_=Attachment.objects.get_or_create(company=co,request_key=file['source_file_id'],defaults=dict(document=bill.identity,original_name=file['original_name'],stored_name=stored,content_type=file['content_type'],size=file['size'],sha256=file['sha256'],uploaded_by=actor,uploaded_by_name=actor.get_username()))
                        r.assistant_attachments=getattr(r,'assistant_attachments',{})
                        r.assistant_attachments[attachment.pk]=file['source_file_id']
                    stubs.enter_context(patch('assistant.receipts.storage.open_read',side_effect=lambda *a:BytesIO(data)))
                    stubs.enter_context(patch('assistant.receipts.sniff.sniff',return_value=file['content_type']))
                    try: row=receipts.read_attachment(actor,co,attachment,s['request_key'])
                    except PausedReading as paused: row=paused.reading
                else:
                    upload=SimpleNamespace(size=file['size'],name=file['original_name'])
                    stubs.enter_context(patch('assistant.receipts.attachments.inspect',return_value=(file['content_type'],file['original_name'],file['sha256'])))
                    stubs.enter_context(patch('assistant.receipts.attachments.check_limits'))
                    stubs.enter_context(patch('assistant.receipts.attachments.store',return_value=stored))
                    stubs.enter_context(patch('assistant.receipts.attachments._read_all',return_value=data))
                    try: row=receipts.read_upload(actor,co,upload,s['request_key'])
                    except PausedReading as paused: row=paused.reading
            r.l2_refs.setdefault(s['ref'],row.pk);r.l2_types[s['ref']]='reading'
        elif op=='assistant_finish':
            row=reading();receipts._finish(actor,co,row,result=r.assistant_results[s['result']],target=f'bill:{row.bill_id}' if row.bill_id else 'bill:new')
        elif op=='assistant_reply':
            import json
            row=reading();raw=s['raw'] if isinstance(s['raw'],str) else json.dumps(s['raw'])
            try: result=receipts.parse_reply(raw,co,today=date.fromisoformat(s.get('today','2026-10-30')))
            except receipts.ReceiptError as exc: receipts._finish(actor,co,row,error=str(exc),code=exc.code,target=f'bill:{row.bill_id}' if row.bill_id else 'bill:new')
            else: receipts._finish(actor,co,row,result=result,target=f'bill:{row.bill_id}' if row.bill_id else 'bill:new')
        elif op=='assistant_fail':
            row=reading();code=s['code']
            if code in CLIENT_FAILURES:
                message,code=receipts._translate(AIError(CLIENT_FAILURES[code]))
            else:
                message={'unauthorized':receipts.NEEDS_SIGNIN,'invalid_grant':receipts.NEEDS_SIGNIN,'not_connected':receipts.NEEDS_SIGNIN,'limit':receipts.QUOTA,'schema':'The AI reply did not have the expected fields, so nothing was filled in.','crashed':receipts.CRASHED,'not_available':receipts.NOT_AVAILABLE,'no_model':'Choose a model on the AI assistant page first.','incomplete':'The AI reply was cut short, so nothing was filled in. Try again.','empty_reply':'The AI returned no reading for this file, so nothing was filled in.','plan_not_granted':services.PLAN_SCOPE_LOST}[code]
            receipts._finish(actor,co,row,error=message,code=code,target=f'bill:{row.bill_id}' if row.bill_id else 'bill:new')
        elif op=='assistant_initial':
            source=reading();ai=receipts.form_initial(source,co);r.assistant_results[s['ref']]=normalize_initial(r,ai,source)
        elif op=='assistant_queue':
            access(actor,co);names=reading_names(r)
            r.assistant_results[s['ref']]={'queue':[names[row.pk] for row in ReceiptReading.objects.filter(company=co,status='ok',bill__isnull=True).order_by('-created_at','-pk')]}
        elif op=='assistant_save':
            from payables import services as bills
            row=reading();p=resolve(r,s['values'])
            if row.bill_id!=p.get('bill_id'): raise ValidationError(receipts.OUT_OF_DATE)
            from payables.forms import LineForm
            forms=[]
            for value in p['lines']:
                data={**value,'expense_account':value.get('expense_account_id'),'item':value.get('item_id')}
                form=LineForm(data=data,company=co)
                if value.get('DELETE'):
                    # Deleted formset rows are ignored by check_lines and Save.
                    form.is_valid();form.cleaned_data['DELETE']=True
                elif not form.is_valid():
                    raise ValidationError(next(iter(form.errors.values()))[0])
                forms.append(form)
            kept=receipts.check_lines(SimpleNamespace(forms=forms),row)
            errors=[error for f in forms for values in f.errors.values() for error in values if not f.data.get('DELETE')]
            if errors: raise ValidationError(errors[0])
            lines=[f.values() for f in forms if not f.data.get('DELETE')]
            with transaction.atomic():
                bill=bills.save_bill(actor,co,**{**p,'lines':lines})
                receipts.audit_kept_lines(actor,co,row,bill,kept)
                row.bill=bill;row.save(update_fields=['bill'])
            track(r,s['ref'],bill,True)
        else: raise ValueError(op)
    except receipts.ReceiptError as exc:
        raise ValidationError(str(exc)) from None
    return True

def reading_names(r):
    out={}
    for name in sorted(r.l2_refs):
        if getattr(r,'l2_types',{}).get(name)=='reading': out.setdefault(r.l2_refs[name],name)
    for i,row in enumerate(ReceiptReading.objects.order_by('pk'),1): out.setdefault(row.pk,f'reading:{i}')
    return out

def extend_assistant_audit(r,ids):
    ids['attachment']={str(pk):name for pk,name in getattr(r,'assistant_attachments',{}).items()}
    ids['reading']={str(pk):name for pk,name in reading_names(r).items()}

def normalize_assistant_audit(event,ids):
    if not(event['action'].startswith('ai.') or event['action']=='bill.ai_lines_kept'): return False
    if event['detail'].get('attachment') is not None: event['detail']['attachment']=ids.get('attachment',{}).get(str(event['detail']['attachment']),'?')
    if 'reading' in event['detail']: event['detail']['reading']=ids['reading'].get(str(event['detail']['reading']),'?')
    if event['target'].startswith('bill:') and event['target']!='bill:new': event['target']='bill:'+ids.get('bill',{}).get(event['target'][5:],'?')
    return True

def normalize_initial(r,ai,source):
    from payables_adapter import names
    n=names(r)
    ai['initial']['vendor']=None if ai['initial']['vendor'] is None else n['vendor'].get(ai['initial']['vendor'],'?')
    for k in ('date','due_date'):
        if k in ai['initial']: ai['initial'][k]=str(ai['initial'][k])
    vendor=ai['vendor'];name=vendor['name']
    ai['vendor']={'name':name,'matched':n['vendor'].get(vendor['matched'].pk) if vendor['matched'] else None,'candidates':[n['vendor'][v.pk] for v in vendor['candidates']], 'new_vendor':{'display_name':name[:120],'billing_address':'\n'.join(source.result['vendor_address'])[:1000]}}
    for row in ai['lines']:
        if row['expense_account'] is not None: row['expense_account']=n['account'][row['expense_account']]
    ai['marked']=sorted(ai['marked'])
    return ai

def dump_assistant(r):
    from payables_adapter import names
    n=names(r);rn=reading_names(r);connections=[];readings=[]
    for co in AIConnection.objects.order_by('company_id'):
        connections.append({'company':n['company'][co.company_id],**{k:getattr(co,k) for k in ('mode','status','account_email','key_last4','model','model_choices','plan_usage','notice_pending','notice_shown','read_attachments','revoke_confirmed','last_error')},'credential_present':co.secret_cipher is not None,'token_expires_at':int(co.token_expires_at.timestamp()) if co.token_expires_at else None,'last_used_at':int(co.last_used_at.timestamp()) if co.last_used_at else None})
    for row in ReceiptReading.objects.order_by('company_id','pk'):
        readings.append({'id':rn[row.pk],'company':n['company'][row.company_id],'created_by_id':row.created_by.username,'request_key':str(row.request_key),'bill_id':n['bill'].get(row.bill_id) if row.bill_id else None,'original_name':row.original_name,'content_type':row.content_type,'size':row.size,'sha256':row.sha256,'status':row.status,'mode':row.mode,'model':row.model,'result':row.result,'error':row.error,'created_at':int(row.created_at.timestamp()),'finished_at':int(row.finished_at.timestamp()) if row.finished_at else None})
    return {'connections':connections,'readings':readings,'results':getattr(r,'assistant_results',{})}
