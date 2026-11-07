"""Seeded offline assistant parity scenarios. Never touches the frozen reference."""
import copy
import json
import random
from pathlib import Path
from uuid import UUID

OUT=Path(__file__).resolve().parents[1]/'scenarios'
COUNTER=100000
def key():
    global COUNTER
    COUNTER+=1
    return str(UUID(int=COUNTER))
def file(): return dict(source_file_id=key(),original_name='synthetic-receipt.png',content_type='image/png',size=100,sha256='a'*64)
def step(op,**values): return dict(op='assistant_'+op,company='co',**values)
def raw(**updates):
    v=dict(vendor_name='Synthetic supplier',vendor_address=['Test address'],document_number='Receipt 42',document_date='2026-10-23',due_date='',currency='USD',subtotal='6.00',tax='0',total='6.00',lines=[dict(description='Widgets',quantity='3',unit_price='2.00',amount='6.00')])
    v.update(updates);return v
def base():
    p=json.loads((OUT/'payables-receipt-core.json').read_text())
    p['steps']=p['steps'][:9]
    p['users'].append(dict(username='member',is_staff=False))
    p['steps'].append(dict(op='add_member',company='co',user='member'))
    return p
def state(mode='api_key'):
    return dict(mode=mode,key_last4='demo' if mode=='api_key' else '',account_email='synthetic@example.invalid' if mode=='siwc' else '',registration=True,model_choices=[dict(slug='demo-model',display_name='Demo'),dict(slug='other-model',display_name='Other')])
def setup(p): p['steps'] += [step('connect',state=state()),step('switch',enabled=True)]
def save(name,p): (OUT/('assistant-'+name+'.json')).write_text(json.dumps(p,indent=2)+'\n')

p=base();p['steps'] += [step('switch',enabled=True,actor='member',expect_error=True),step('model',model='demo-model',expect_error=True),step('connect',state=state('siwc')),step('switch',enabled=True),step('metadata',expires=2000000000,used=True),step('model',model=' other-model '),step('model',model='bad',expect_error=True),step('dismiss'),step('models',choices=[dict(slug='third',display_name='Third')]),step('disconnect',revoked=False),step('connect',state=state('siwc')),step('needs_signin'),step('connect',state=state()),step('models',choices=[],expect_error=True),step('disconnect'),step('disconnect',expect_error=True)]
save('settings',p)

p=base();variants=[raw(),raw(currency=''),raw(currency='CAD'),raw(lines=[]),raw(subtotal='',tax='1',total='7'),raw(lines=[dict(description='',quantity='',unit_price='',amount='6')]),raw(lines=[dict(description='Rounded',quantity='1.005',unit_price='1',amount='1.00')],subtotal='1',total='1'),raw(lines=[dict(description='Mismatch',quantity='3',unit_price='2',amount='7')],subtotal='7',total='7'),raw(document_date='1989-12-31',due_date='2030-01-01'),raw(document_date='2026-10-23T12:00:00',due_date='2026-10-22'),raw(lines=[dict(description='Summed',quantity='2.5',unit_price='2',amount='')],subtotal='',total='5'),raw(vendor_name='  CAFÉ\u200b Supply\t LLC  ',vendor_address=['  a  ','','b','c','d','e']),raw(total='NaN'),raw(total='-1'),raw(total='99999999999999.991'),raw(total=6),raw(lines={}),raw(lines=[dict(description='Bad',quantity='1',unit_price='bad',amount='6')]),raw(lines=[dict(description='Fallback',quantity='-1',unit_price='',amount='6')]),raw(document_date='20261023',due_date='2026-W44-1'),raw(subtotal='$6.00 USD',tax='(0)',total=' USD $6.00 ')]
variants += [raw(total='UſD 6.00'),raw(vendor_name='A\u0897B\u034fC\u2028D'),raw(total='1e-1000'), raw(total='6.'+'0'*100), raw(total='-1e1000'),
 raw(lines=[dict(description='Large computed',quantity='2',unit_price='99999999999999',amount='')],subtotal='',total='1'),
 raw(lines=[dict(description='Large aggregate',quantity='1',unit_price='99999999999999',amount='99999999999999')]*2,subtotal='',total='1'),
 raw(lines=[dict(description='Signed zero',quantity='1',unit_price='-0',amount='-0')],subtotal='-0',tax='-0',total='-0')]
for i,v in enumerate(variants): p['steps'].append(step('parse',ref=f'parse-{i}',raw=v))
p['steps'] += [step('parse',ref='bad-json',raw='not JSON'),step('parse',ref='bad-shape',raw=[])]
save('parser',p)

p=base();setup(p);p['capture_steps']=True
p['steps'].append(step('parse',ref='fields',raw=raw(lines=[dict(description='Widgets',quantity='3',unit_price='2.00',amount='7.00')],subtotal='7',total='7')))
f=file();k=key()
p['steps'] += [step('begin',ref='reading',request_key=k,file=f,actor='member'),step('begin',ref='reading',request_key=k,file=f,actor='member'),step('finish',reading='reading',result='fields',actor='member'),step('initial',reading='reading',ref='initial',actor='member'),step('queue',ref='queue-before',actor='member')]
payload=dict(vendor_id='vendor',date='2026-10-23',due_date='2026-11-22',reference='Receipt 42',memo='',request_key=key(),lines=[dict(description='Widgets',quantity='3',rate='2.00',expense_account_id='expense',ai_line=1)])
p['steps'].append(step('save',reading='reading',ref='bill',values=copy.deepcopy(payload),actor='member',expect_error=True))
payload['lines'][0]['ai_line']=' +１ '
p['steps'].append(step('save',reading='reading',ref='bill',values=copy.deepcopy(payload),actor='member',expect_error=True))
payload['lines'][0]['ai_line']=1
payload['lines'][0]['keep_as_entered']=True
bad_rate=copy.deepcopy(payload);bad_rate['lines'][0]['rate']='2.335';bad_rate['lines'][0]['keep_as_entered']=False
p['steps'].append(step('save',reading='reading',ref='bad-rate',values=bad_rate,actor='member',expect_error=True))
p['steps'] += [step('save',reading='reading',ref='bill',values=payload,actor='member'),step('begin',ref='reading',request_key=k,file=f,actor='member'),step('queue',ref='queue-after',actor='member'),dict(op='payables_post',company='co',bill='bill',expected_revision=1),dict(op='payables_void',company='co',bill='bill',expected_revision=2,date='2026-10-24',reason='Synthetic receipt correction')]
save('draft-save-post-void',p)

p=base();setup(p);p['steps'].append(step('parse',ref='fields',raw=raw()))
f=file();k=key()
p['steps'] += [step('begin',ref='reading',request_key=k,file=f,now=1000),step('begin',ref='reading',request_key=k,file=f,now=1600),step('begin',ref='reading',request_key=k,file={**f,'source_file_id':key(),'original_name':'fresh-retry.png'},now=1601),step('finish',reading='reading',result='fields',now=1602),step('begin',ref='reading',request_key=k,file=f,now=2000),step('begin',ref='reading',request_key=k,file={**f,'sha256':'b'*64},now=2000,expect_error=True),step('begin',ref='failed',request_key=key(),file=file(),now=2000),step('fail',reading='failed',code='limit',now=2001)]
save('retry-failures',p)

p=base();setup(p)
for i in range(31): p['steps'].append(step('begin',ref=f'rate-{i}',request_key=key(),file=file(),now=1000,actor='member',expect_error=i==30))
p['steps'].append(step('begin',ref='other-actor',request_key=key(),file=file(),now=1000))
save('rate',p)

p=base();setup(p);p['steps'].append(step('parse',ref='fields',raw=raw()));rng=random.Random(427100)
pending=[];done=[];clock=1000;initial_steps=len(p['steps']);i=0
while len(p['steps'])-initial_steps < 500:
    choice=rng.randrange(13);remaining=500-(len(p['steps'])-initial_steps)
    if choice<=2:
        ref=f'random-{i}';pending=[ref];p['steps'].append(step('begin',ref=ref,request_key=key(),file=file(),now=clock));clock+=601
    elif choice==3 and pending:
        ref=pending.pop();p['steps'].append(step('finish',reading=ref,result='fields',now=clock-600));done.append((ref,False))
    elif choice==4 and pending:
        ref=pending.pop();p['steps'].append(step('fail',reading=ref,code=rng.choice(['schema','limit','crashed','timeout','plan_not_granted']),now=clock-600))
    elif choice==5 and done:
        p['steps'].append(step('initial',reading=rng.choice(done)[0],ref=f'initial-{i}'))
    elif choice==6:
        model=rng.choice(['demo-model','other-model','bad-model'])
        p['steps'].append(step('model',model=model,expect_error=model=='bad-model'))
    elif choice==7:
        p['steps'].append(step('queue',ref=f'queue-{i}'))
    elif choice==8 and pending:
        ref=pending.pop();mismatch=bool(rng.randrange(2))
        reply=raw(lines=[dict(description='Widgets',quantity='3',unit_price='2.00',amount='7.00' if mismatch else '6.00')],subtotal='7' if mismatch else '6',total='7' if mismatch else '6')
        p['steps'].append(step('reply',reading=ref,raw=reply,now=clock-600));done.append((ref,mismatch))
    elif choice in (9,10) and done:
        ref,mismatch=done.pop(rng.randrange(len(done)))
        p['steps'].append(step('save',reading=ref,ref=f'saved-{i}',actor='member',values=dict(vendor_id='vendor',date='2026-10-23',due_date='2026-11-22',reference=f'Random receipt {i}',memo='',request_key=key(),lines=[dict(description='Widgets',quantity='3',rate='2.00',expense_account_id='expense',ai_line=1,keep_as_entered=mismatch)])))
    elif choice==11 and remaining>=4:
        ref=f'member-401-{i}';pending=[]
        p['steps'] += [step('begin',ref=ref,request_key=key(),file=file(),actor='member',now=clock),step('fail',reading=ref,code='unauthorized',actor='member',now=clock+1),step('needs_signin',actor='member'),step('connect',state=state())]
        clock+=601
    else:
        p['steps'].append(step('parse',ref=f'parsed-{i}',raw=raw(tax=str(rng.randrange(3)),total=str(rng.randrange(5,10)))))
    i+=1
assert len(p['steps'])-initial_steps==500
for s in p['steps'][initial_steps:]: s.setdefault('expect_error',False)
for op in ('assistant_save','assistant_reply','assistant_needs_signin'):
    assert any(s['op']==op for s in p['steps'])
assert any(s['op']=='assistant_save' and s['values']['lines'][0].get('keep_as_entered') for s in p['steps'])
save('random-500',p)


p=base();setup(p)
for i,value in enumerate(["not JSON",raw(currency='CAD'),raw(total='-1'),raw(total='bad'),raw()]):
    name=f'reply-{i}';p['steps'] += [step('begin',ref=name,request_key=key(),file=file()),step('reply',reading=name,raw=value)]
for i,code in enumerate(['not_available','no_model','incomplete','empty_reply']):
    name=f'closed-failure-{i}';p['steps'] += [step('begin',ref=name,request_key=key(),file=file()),step('fail',reading=name,code=code)]
save('reading-errors',p)


# Every reviewed lifecycle pair and fixed client sentence is exercised against Python.
p=base();p['capture_steps']=True
for action in ('ai.call_failed','ai.refresh_failed'):
    for code in ('unauthorized','invalid_grant','plan_not_granted'):
        for mode in ('siwc','api_key'):
            p['steps'] += [step('connect',state=state(mode)),step('needs_signin',actor='member',action=action,code=code)]
p['steps'] += [step('disconnect'),step('needs_signin',actor='member',expect_error=True)]
long_state=state('siwc');long_state['account_email']='é'*260;long_state['models_error']='timeout'
p['steps'].append(step('connect',state=long_state));p['steps'].append(step('switch',enabled=True))
for code in ('invalid_address','missing_host','insecure_address','address_credentials','redirect','certificate','timeout','unreachable','response_unreadable','response_oversized','token_incomplete','metadata_no_key_set','metadata_no_keys','invalid_model_list','invalid_responses_request','stream_interrupted','stream_oversized','plan_not_granted'):
    p['steps'] += [step('begin',ref=code,request_key=key(),file=file(),actor='member'),step('fail',reading=code,code=code,actor='member')]
for s in p['steps']: s.setdefault('expect_error',False)
save('review-fix1',p)

p=base();p['capture_steps']=True
bill=dict(vendor_id='vendor',date='2026-10-23',due_date='2026-11-22',reference='Attachment ordering',memo='',request_key=key(),lines=[dict(description='Widgets',quantity='1',rate='6.00',expense_account_id='expense')])
p['steps'].append(dict(op='payables_save',company='co',ref='bill',values=bill))
setup(p)
f=file();f['bill_id']='bill';k=key()
p['steps'] += [step('begin',ref='attached',request_key=k,file=f,actor='member'),step('begin',ref='attached',request_key=k,file=f,actor='member'),step('reply',reading='attached',raw=raw(),actor='member'),dict(op='payables_post',company='co',bill='bill',expected_revision=1),step('disconnect'),step('begin',ref='not-draft',request_key=key(),file=f,actor='member',expect_error=True)]
for s in p['steps']: s.setdefault('expect_error',False)
save('attachment-entrypoints',p)
