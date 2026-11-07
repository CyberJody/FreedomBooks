"""Execute the frozen calculations and Django validators to capture answer-key vectors.
Run with py/env.sh sourced and fbv2_l2a_ref selected. Writes this crate's fixtures only.
"""
import os, sys, json, random
from decimal import Decimal
from pathlib import Path
sys.path.insert(0, os.environ['FB_REFERENCE'])
import django
django.setup()
from django.core.exceptions import ValidationError
from django.core.validators import validate_email
from django.db import connection
from documents import calculations as calc
from directory import party_display
from types import SimpleNamespace
root=Path(__file__).resolve().parents[2]/'fb-core'/'tests'
rows=[]
def capture(fn,args):
    try:
        if fn=='totals': value=calc.totals(args['lines'],**args['tax'])
        elif fn=='email':validate_email(args[0]);value=True
        elif fn in ('town_from_address','posting_role_label'):value=getattr(party_display,fn)(*args)
        else:value=getattr(calc,fn)(*args)
        value=json.loads(json.dumps(value,default=str))
        rows.append(dict(fn=fn,args=args,ok=True,value=value))
    except ValidationError as exc:rows.append(dict(fn=fn,args=args,ok=False,message='; '.join(exc.messages)))
bad=[True,False,1.2,None,'NaN','sNaN','Infinity','-Infinity','-0.001','-1','1e999999','0','0.00001','100000000000000','1.001','99999999999999.999','1e-999999','0.0001','99999999999999.99','1_0','١٢.٣٤',' 2.00 ','2.0000000000000000000000000000000000000','0e9999','1e2']
for fn in ['quantity','unit_rate']:
    for v in bad:capture(fn,[v])
for q,r in [('1.005','1.00'),('1.0049','1.00'),('0.0001','50.00'),('2','0'),('2','99999999999999.99'),('99999999999999.9999','99999999999999.99'),('99999999999999.9999','0.01')]:capture('line_amount',[q,r])
def snap(rates,mode='GROUP',description='Combined tax'):
    return dict(kind=mode,id=1,name='Group',description=description,members=[dict(id=i,name=str(i),description=str(i),rate=r,agency_id=i,agency_name=str(i))for i,r in enumerate(rates,1)])
def totals(lines,**tax):capture('totals',dict(lines=lines,tax=tax))
line=lambda amount,flag=True:dict(quantity='1',rate=amount,taxable=flag)
for c in range(1,3000,7):totals([line(str(Decimal(c)/100))],tax_mode='GROUP',tax_snapshot=snap(['2.875','4.0001','0.125']))
for c in range(1,5000,13):
    for mode in ['RATE','MANUAL']:
        kw=dict(tax_mode=mode,tax_snapshot=snap(['8.25'],'RATE')) if mode=='RATE' else dict(tax_mode=mode,tax_rate='8.25',tax_label='Tax')
        totals([line(str(Decimal(c)/100))],**kw)
for mode,snapshot in [('GROUP',snap(['3'])),('RATE',snap(['3','4'])),('GROUP',None),('GROUP',dict(kind='RATE',members=[])),('GROUP',snap(['3','abc'])),('GROUP',snap(['60','50'])),('GROUP',snap(['3','-1'])),('GROUP',snap(['3','4'],description='')),('GROUP',snap(['3','1.00001']))]:totals([line('10.00')],tax_mode=mode,tax_snapshot=snapshot)
for mode,rate,label in [(None,None,''),('AUTO','1',''),('NONE','1',''),('MANUAL','100.0001','Tax'),('MANUAL','0.00001','Tax'),('MANUAL',True,'Tax'),('MANUAL','-1','Tax'),('MANUAL','NaN','Tax'),('MANUAL','1','')]:totals([line('1.00')],tax_mode=mode,tax_rate=rate,tax_label=label)
for lines,tax in [([line('0')],dict(tax_mode='NONE',for_posting=True)),([line('99999999999999.99')]*2,dict(tax_mode='NONE')),([line('99999999999999.99')],dict(tax_mode='MANUAL',tax_rate='100',tax_label='Tax')),([line('1','yes')],dict(tax_mode='NONE'))]:totals(lines,**tax)
for address in ['x@example.com','x@localhost','a..b@example.com','.x@example.com','x.@example.com','x@-example.com','x@example-.com','x@a.c','x@a.123','x@a.xn--x','x@example.com.','"quoted"@example.com','"a b"@example.com','"a\\ b"@example.com','x@[127.0.0.1]','x@[::1]','x@[999.1.1.1]','x@exämple.test','İ@x.com','ı@x.com','ſ@x.com','K@x.com','é@x.com','x@x.éé','x@foo_bar.com','x@'+'a'*64+'.com','a@a.'+'c'*64]:capture('email',[address])
for address in ['', 'One line', 'Street\nTown, NY 12345', '\n Street\r\n Town \n US', 'Street\u0085Town\u2028State']:
    capture('town_from_address',[address])
for role in [None,'',' receivables ','payables:99','income:2','income:²','income:١٢','income:1.2','expense:3','debit:4','undeposited:123','mystery_role:42','ǳ_unknown','ß_UNKNOWN']:
    for kind in [None,'INV','BILL','CHK','SR','DEP']:
        capture('posting_role_label',[role,kind])
(root/'l2a_python_vectors.json').write_text(json.dumps(rows,separators=(',',':'))+'\n')
# Compare libc casing of non-ASCII chars and composed/decomposed names with PG17 itself.
texts=['İ','ı','I','i','ß','ẞ','Σ','σ','ς','ſ','K','é','e\u0301','Æ','ǰ']
with connection.cursor()as cur:
    cur.execute('SELECT x,lower(x),upper(x) FROM unnest(%s::text[])x',[texts]);case=cur.fetchall()
(root/'l2a_pg_case_vectors.json').write_text(json.dumps(case,ensure_ascii=False)+'\n')
print('Captured',len(rows),'Python cases and',len(case),'live PostgreSQL case-mapping cases')
