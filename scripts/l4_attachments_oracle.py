"""Generate frozen Python sniff vectors and deterministic differential scenarios. No reference writes."""
import os,sys,json,random,copy,struct
from pathlib import Path
sys.path.insert(0,os.environ['FB_REFERENCE'])
import django
django.setup()
from attachments import sniff
from attachments.tests import pdf,png,jpeg,webp,PDF,PNG,JPEG,WEBP,HTML,HEIC,EXE

def write(path,obj):Path(path).write_text(json.dumps(obj,ensure_ascii=False,indent=2)+'\n')
files=[]
def vector(label,b):
    t=sniff.sniff(b);files.append(dict(label=label,hex=b.hex(),type=t,problem=sniff.problem(b,t) if t else None))
for label,b in [('pdf',PDF),('png',PNG),('jpeg',JPEG),('webp',WEBP)]:
    vector(label,b)
    for cut in range(len(b)+1):vector(f'{label}-truncated-{cut}',b[:cut])
    for tail in [b'\0',b'\xff',HTML,EXE,b'x']:vector(label+'-tail',b+tail)
    for i in range(len(b)):
        changed=bytearray(b);changed[i]^=255;vector(f'{label}-mutated-{i}',bytes(changed))
for b in [png(HTML),jpeg(HTML),webp(HTML),pdf(HTML),png(b'<SvG}\xf4\xfe\x97\xe7~\x9f\xfd'),jpeg()+jpeg(),jpeg()+b'\0'*5,HEIC,EXE,HTML,b'%PDF-1.4\n%%EOF'+b' '*1020]:vector('special',b)
names=[]
for raw in ['../../x.pdf','C:\\Users\\receipt.png','\u202escan\r\n.pdf',' a\t b .pdf ','x.exe','scan','x.jpeg','.pdf','...','x%<>:"|?*;.pdf','e\u0301.pdf','\x00\u200b','a'*130+'.pdf','a'*120+'.longextension','\ue000x\U0010ffff.pdf']:
    for t in sniff.TYPES:
        v=dict(raw=raw,type=t,clean=sniff.clean_name(raw))
        try:v['display']=sniff.display_name(raw,t)
        except sniff.Refused as e:v['error']=str(e)
        names.append(v)
write('crates/fb-core/tests/attachments_vectors.json',dict(files=files,names=names))
root=Path('crates/fb-harness/scenarios')
n=900000
def key():
    global n;n+=1;return f'00000000-0000-0000-0000-{n:012x}'
def add(ref,name='scan.pdf',b=PDF,**kw):return dict(op='attachment_add',company='co',journal='j',ref=ref,name=name,hex=b.hex(),request_key=key(),**kw)
setup=[dict(op='create_company',ref='co',name='Synthetic attachment book'),dict(op='save_account',company='co',ref='cash',code='100',name='Cash',kind='ASSET'),dict(op='save_account',company='co',ref='equity',code='300',name='Equity',kind='EQUITY'),dict(op='save_draft',company='co',ref='j',date='2026-01-01',memo='Test',lines=[dict(account='cash',debit='1.00',credit='0'),dict(account='equity',debit='0',credit='1.00')])]
users=[dict(username='test',is_staff=True),dict(username='outsider')]
steps=copy.deepcopy(setup)
a=add('a','scan');steps += [a,copy.deepcopy(a),{**a,'name':'different.pdf','expect_error':True},dict(op='post_journal',company='co',journal='j'),add('png','image.png',PNG),add('jpeg','photo.jpg',JPEG),add('webp','image.webp',WEBP),dict(op='attachment_remove',company='co',attachment='a',reason='wrong file'),dict(op='attachment_remove',company='co',attachment='a',reason='retry'),{**a,'expect_error':True},dict(op='attachment_download',company='co',attachment='a',ref='removed-hash'),dict(op='reverse_journal',company='co',journal='j',ref='reversal',date='2026-01-02',reason='wrong'),add('aftervoid'),{**add('badreverse'),'journal':'reversal','expect_error':True},{**add('outsider'),'actor':'outsider','expect_error':True},dict(op='create_company',ref='other',name='Other'),{**add('cross'),'company':'other','expect_error':True},{**add('adopt','adopt.pdf',pdf(b'adopt')),'op':'attachment_adopt'}]
write(root/'attachments-lifecycle.json',dict(users=users,steps=steps,capture_steps=True))
steps=copy.deepcopy(setup)
for i,(name,b) in enumerate([('fake.pdf',EXE),('fake.pdf',HTML),('photo.heic',HEIC),('fake.exe',PDF),('invalid.png',PNG+HTML),('invalid.pdf',pdf(HTML)),('empty.pdf',b'')]):steps.append(add(f'bad{i}',name,b,expect_error=True))
for i in range(20):steps.append(add(f'file{i}'))
steps+=[add('capped',expect_error=True),dict(op='attachment_remove',company='co',attachment='file0',reason='remove'),add('room'),add('throttled',uploads_per_window=1,expect_error=True),dict(op='attachment_remove',company='co',attachment='room',reason=' '*4,expect_error=True),dict(op='attachment_remove',company='co',attachment='room',reason='r'*501,expect_error=True)]
write(root/'attachments-refusals.json',dict(users=users,steps=steps,capture_steps=True))
steps=copy.deepcopy(setup);rng=random.Random(39881);live=[];removed=[];counter=0
for i in range(500):
    choice=rng.randrange(5)
    if choice<=1 and len(live)<20:
        ref=f'file{counter}';counter+=1;step=add(ref,b=pdf(str(i).encode()),uploads_per_window=1000);steps.append(step);live.append((ref,step))
    elif choice==2 and live:
        ref,step=rng.choice(live);steps.append(dict(op='attachment_remove',company='co',attachment=ref,reason=f'correct {i}'));live=[v for v in live if v[0]!=ref];removed.append((ref,step))
    elif choice==3 and live:steps.append(copy.deepcopy(rng.choice(live)[1]))
    elif removed:
        ref,step=rng.choice(removed);steps.append(dict(op='attachment_remove',company='co',attachment=ref,reason='repeat'))
    else:steps.append(add('bad'+str(i),'bad.exe',PDF,expect_error=True))
write(root/'attachments-random-500.json',dict(users=users,steps=steps,capture_steps=True))
# Attachment document lifecycle: every existing checkpoint is exercised between bill saves/posts/voids.
base=json.loads((root/'payables-lifecycle.json').read_text());steps=[];counter=0
for s in base['steps']:
    steps.append(s)
    if s['op'] in ['payables_save','payables_post','payables_void'] and not s.get('expect_error'):
        counter+=1;v=add(f'docfile{counter}');v.pop('journal');v['document']=s.get('bill',s.get('ref'))+':source';steps.append(v)
write(root/'attachments-document-lifecycle.json',{**base,'steps':steps,'capture_steps':True})
# Apply reminder reads to existing complete lifecycle and settlement fixtures.
for source in ['payables-payments','sales-invoice-payments','credits-lifecycle','prepayments-1200-less-500']:
    p=root/(source+'.json')
    if not p.exists():continue
    base=json.loads(p.read_text());company=next(s['ref'] for s in base['steps'] if s['op']=='create_company');steps=[]
    for i,s in enumerate(base['steps']):
        steps.append(s)
        if s['op'] in ['payables_post','payables_pay','payables_reverse','payables_void','sales_post_invoice','sales_record_receipt','sales_reverse_receipt','credits_apply','pp_apply'] and not s.get('expect_error'):
            for days in [0,10,11]:steps.append(dict(op='reminder_build',company=company,today='2026-12-01',days=days,ref=f'rem{i}-{days}'))
            steps.append(dict(op='reminder_count',company=company,today='2026-12-01',ref=f'count{i}'))
    steps.append(dict(op='reminder_build',company=company,today='2026-12-01',ref='final-reminder'))
    write(root/('attachments-reminders-'+source+'.json'),{**base,'steps':steps,'capture_steps':True})
