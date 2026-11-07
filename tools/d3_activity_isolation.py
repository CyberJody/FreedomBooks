"""Put later-wave activity documents in their own synthetic company.

D1 register fixtures keep their original provenance inventory; D7b owns the
remaining register source resolvers. D3 still compares fully populated customer
and vendor activity, without expanding that separate wave's implementation.
"""
from pathlib import Path
import json

p=Path('crates/fb-screendiff/scenarios/screens-d3.json');s=json.loads(p.read_text());i=next(i for i,r in enumerate(s['steps']) if r.get('ref')=='ap');extra=s['steps'][i+3:];s['steps']=s['steps'][:i]
def step(op,**kw):s['steps'].append(dict(op=op,company='activity',expect_error=False,**kw))
step('create_company',ref='activity',name='Activity synthetic books')
for ref,code,name,typ in [('bank','1000','Checking','BANK'),('ar','1100','Receivables','RECEIVABLE'),('uf','1200','Undeposited','OTHER_CURRENT_ASSET'),('ap','2000','Payables','PAYABLE'),('tax','2200','Tax','OTHER_CURRENT_LIABILITY'),('cdp','2300','Customer deposits','OTHER_CURRENT_LIABILITY'),('income','4000','Income','INCOME'),('expense','5000','Expense','EXPENSE')]:step('save_account',ref='act-'+ref,code=code,name=name,detail_type=typ)
step('l2_save',model='customer',ref='act-buyer',values=dict(display_name='Activity buyer'))
step('l2_save',model='vendor',ref='act-vendor',values=dict(display_name='Activity vendor'))
step('l2_setup',expected_revision=0,values=dict(receivables='act-ar',undeposited='act-uf',payables='act-ap',collected_tax='act-tax',customer_deposits='act-cdp',banks=['act-bank']))
step('sales_save_invoice',ref='act-inv1',values=dict(request_key='00000000-0000-0000-0000-000000000afe',customer_id='act-buyer',date='2026-10-01',due_date='2026-10-31',memo='Activity invoice',tax_mode='NONE',lines=[dict(item_id=None,description='Work',quantity='1',rate='100.00',income_account_id='act-income',taxable=False)]))
step('sales_review_invoice',record='act-inv1',expected_revision=1,review='act-inv-review');step('sales_post_invoice',record='act-inv1',expected_revision=1,review='act-inv-review')
mapping={ref:'act-'+ref for ref in ['bank','ar','uf','ap','tax','cdp','income','expense','buyer','vendor','bill','billpay','check','memo','memo-review','memo-application','apply-review','memo-refund','refund-review','deposit','deposit-review','inv1']};mapping['c']='activity'
def map_refs(v):
    if isinstance(v,dict):return {k:map_refs(x) for k,x in v.items()}
    if isinstance(v,list):return [map_refs(x) for x in v]
    return mapping.get(v,v) if isinstance(v,str) else v
s['steps'] += [map_refs(r) for r in extra]
p.write_text(json.dumps(s,indent=2)+'\n')
p=Path('crates/fb-screendiff/screens.json');screens=json.loads(p.read_text())
for m,plural,record in [('customer','customers','Activity buyer'),('vendor','vendors','Activity vendor')]:screens['directory-'+m+'-activity']={'path':'/c/{company:Activity synthetic books}/lists/'+plural+'/{'+m+':Activity synthetic books:'+record+'}/'}
p.write_text(json.dumps(screens,indent=2)+'\n')

p=Path('crates/fb-screendiff/py/render_django.py');t=p.read_text();old="[('account','code',Account),('customer','display_name',Customer),('item','code',Item)]";assert old in t
t=t.replace(old,"[('account','code',Account),('customer','display_name',Customer),('item','code',Item),('vendor','display_name',Vendor)]")
old="    for name, screen in SCREENS.items():";assert old in t
t=t.replace(old,"""    for other in Company.objects.exclude(pk=company.pk):
        for model, cls in [('bill', Bill), ('billpayment', BillPayment), ('check', Check), ('creditmemo', CreditMemo), ('creditrefund', CreditRefund), ('receipt', Receipt), ('invoice', Invoice)]:
            for row in cls.objects.filter(company=other).select_related('identity'):
                ids[model + ':' + other.name + ':' + row.identity.number] = str(row.pk)
    for name, screen in SCREENS.items():""")
p.write_text(t)
p=Path('crates/fb-screendiff/src/main.rs');t=p.read_text();old='("item","directory_item","code")]';assert old in t;t=t.replace(old,'("item","directory_item","code"),("vendor","directory_vendor","display_name")]')
old='            Ok(ids)\n';assert old in t
t=t.replace(old,'''
            let mut st=tx.prepare("SELECT id,name FROM ledger_company WHERE id<>?1")?;
            for row in st.query_map([&company],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {
                let (co,name)=row?;
                for (model,table) in [("bill","payables_bill"),("billpayment","payables_billpayment"),("check","checks_check"),("creditmemo","credits_creditmemo"),("creditrefund","credits_creditrefund"),("receipt","sales_receipt"),("invoice","sales_invoice")] {
                    let mut st=tx.prepare(&format!("SELECT d.id,i.number FROM {table} d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=?1"))?;
                    for row in st.query_map([&co],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {let (id,number)=row?;ids.insert(format!("{model}:{name}:{number}"),id);}
                }
            }
            Ok(ids)
''');p.write_text(t)
p=Path('crates/fb-screendiff/src/normalize.rs');t=p.read_text();old='        "memo_id" => "creditmemo",';assert old in t;t=t.replace(old,'        "credit_memo_id" => "creditmemo",\n        "customer_deposit_id" => "receipt",');p.write_text(t)
