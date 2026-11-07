"""D3 fixture states, scoped identities, and the frozen render driver additions."""
import json
from pathlib import Path

def edit(name,old,new):
    p=Path(name);s=p.read_text();assert old in s,(name,old);p.write_text(s.replace(old,new))

s=json.loads(Path('crates/fb-screendiff/scenarios/screens-d2.json').read_text());s['name']='screens-d3'
steps=[
 dict(op='l2_save',model='vendor',ref='agency',values=dict(display_name='County Revenue',business_name='County office',billing_address='1 Main Street\nPond, NY 10000',email='tax@example.invalid')),
 dict(op='l2_save',model='vendor',ref='vendor',values=dict(display_name='Pond Supply',default_expense_account='expense',default_terms='30')),
 dict(op='l2_save',model='vendor',ref='archived-vendor',values=dict(display_name='Archived supplier')),
 dict(op='l2_active',model='vendor',record='archived-vendor',active=False),
 dict(op='l2_save',model='customer',record='buyer',values=dict(billing_address='22 Maple Street\nPond, NY 10000',notes='Preferred delivery\nSide door',email='buyer@example.invalid')),
 dict(op='l2_save',model='customer',ref='exempt',values=dict(display_name='Resale buyer',tax_status='EXEMPT',exemption_number='RESALE-2026')),
 dict(op='l2_save',model='item',ref='goods',values=dict(code='SEED',description='Grass seed',kind='NONSTOCK',sale_rate='9.995',purchase_cost='4.50',income_account='income',expense_account='expense',taxable=False)),
 dict(op='l2_rate',ref='county',values=dict(name='County',description='County sales tax',rate='6.25',agency_id='agency')),
 dict(op='l2_rate',ref='city',values=dict(name='City',description='City sales tax',rate='1.5',agency_id='agency')),
 dict(op='l2_rate',ref='old-tax',values=dict(name='Old tax',description='Old rate',rate='0',agency_id='agency')),
 dict(op='l2_tax_active',record='old-tax',active=False,expected_revision=1),
 dict(op='l2_group',ref='combined',values=dict(name='County and City',description='Sales tax',member_ids=['county','city'])),
 dict(op='l2_settings',values=dict(expected_revision=0,charge_tax=True,default_item_id='combined',owed_basis='ACCRUAL',filing_frequency='QUARTERLY')),
]
for step in steps:s['steps'].append(dict(company='c',expect_error=False,**step))
Path('crates/fb-screendiff/scenarios/screens-d3.json').write_text(json.dumps(s,indent=2)+'\n')
p=Path('crates/fb-screendiff/screens.json');screens=json.loads(p.read_text())
def add(name,path,post=None,status=200,**kwargs):
    row=dict(path=path,**kwargs)
    if post is not None:row['post']=post
    if status!=200:row['status']=status
    screens[name]=row
base='/c/{company}'
for kind in ['customers','vendors','items']:
    path=base+'/lists/'+kind+'/'
    add('directory-'+kind,path)
    add('directory-'+kind+'-all',path+'?status=all&q=')
    add('directory-'+kind+'-search',path+'?q=Pond&status=all')
    add('directory-'+kind+'-invalid-filter',path+'?status=wrong',status=400)
    add('directory-'+kind+'-new',path+'new/')
    add('directory-'+kind+'-new-invalid',path+'new/',post={'display_name':'','email':'bad','code':'','kind':'wrong','sale_rate':'bad','tax_status':''},status=400)
    key={'customers':'customer:Maple & Birch Café','vendors':'vendor:Pond Supply','items':'item:MOW'}[kind]
    detail=path+'{'+key+'}/'
    add('directory-'+kind+'-detail',detail)
    add('directory-'+kind+'-edit',detail+'edit/')
    add('directory-'+kind+'-edit-invalid',detail+'edit/',post={'display_name':'','code':'','kind':'','sale_rate':'','tax_status':''},status=400)
    add('directory-'+kind+'-status-invalid',detail+'status/',post={'active':'wrong'},status=400)
add('directory-customer-duplicate',base+'/lists/customers/{customer:Twin buyer:#3}/')
add('directory-vendor-inactive',base+'/lists/vendors/{vendor:Archived supplier}/edit/')
add('tax-settings',base+'/sales-tax/')
add('tax-settings-invalid',base+'/sales-tax/',post={'expected_revision':'bad','charge_tax':'wrong','default_item':'bad','owed_basis':'wrong','filing_frequency':'wrong'},status=400)
add('tax-settings-stale',base+'/sales-tax/',post={'expected_revision':'0','charge_tax':'yes','default_item':'','owed_basis':'ACCRUAL','filing_frequency':'MONTHLY'},status=400)
for suffix,query in [('', ''),('-all','?status=all'),('-inactive','?status=inactive'),('-invalid','?status=wrong'),('-blank','?status=')]:
    add('tax-list'+suffix,base+'/sales-tax/items/'+query,status=400 if suffix=='-invalid' else 200)
for kind,key in [('rate','taxitem:County'),('group','taxitem:County and City')]:
    path=base+'/sales-tax/'+kind+'s/'
    add('tax-'+kind+'-new',path+'new/')
    add('tax-'+kind+'-new-invalid',path+'new/',post={'name':'','description':'','rate':'','expected_revision':'bad','agency':'bad','members':'bad'},status=400)
    add('tax-'+kind+'-edit',path+'{'+key+'}/edit/')
    add('tax-'+kind+'-edit-invalid',path+'{'+key+'}/edit/',post={'name':'','description':'','rate':'bad','members':'bad'},status=400)
add('tax-rate-business-invalid',base+'/sales-tax/rates/new/',post={'name':'Test rate','description':'Sales tax','rate':'bad'},status=400)
add('tax-group-business-invalid',base+'/sales-tax/groups/new/',post={'name':'One rate','description':'Sales tax','members':'{taxitem:County}'},status=400)
add('tax-rate-status-invalid',base+'/sales-tax/items/{taxitem:County}/status/',post={},status=400)
add('tax-rate-status-refused',base+'/sales-tax/items/{taxitem:County}/status/',post={'expected_revision':'1','active':'0'},status=400)
add('tax-bulk',base+'/sales-tax/first-enable/')
add('tax-bulk-invalid',base+'/sales-tax/first-enable/',post={'digest':'bad','decision':'bad'},status=400)
add('tax-bulk-stale',base+'/sales-tax/first-enable/',post={'digest':'0'*64,'decision':'apply','customers':'on','items':'on'},status=400)
add('reminders',base+'/reminders/')
add('reminders-invalid',base+'/reminders/settings/',post={'days':'91','expected':'bad'},status=400)
add('reminders-conflict',base+'/reminders/settings/',post={'days':'11','expected':'9'},status=409)
add('invoice-new-tax-off','/c/{company:Other synthetic books}/invoices/new/')
for mode in ['GROUP','RATE']:
    add('tax-members-'+mode.lower(),'/help/report/',template='salestax/members.html',context={'doc':{'tax_mode':mode,'tax_snapshot':{'name':'County and City','members':[{'name':'County','rate':'6.2500','tax':'1.25','agency_name':'County Revenue'},{'name':'City','rate':'1.5000','tax':'0.30','agency_name':'County Revenue'}]}}})
p.write_text(json.dumps(screens,indent=2)+'\n')

# Use the actual frozen route table, not guessed plural spellings.
routes=json.loads(Path('crates/fb-templates/urls.json').read_text())
for name in ['salestax:settings','salestax:list','salestax:rate_new','salestax:group_new','salestax:bulk_step']:
    print(name,routes[name])

edit('crates/fb-screendiff/run_screendiff.sh','fbv2_int_d2_ref','fbv2_int_d3_ref')
edit('crates/fb-screendiff/run_screendiff.sh','fbv2_int_d2_screens','fbv2_int_d3_screens')
edit('crates/fb-screendiff/run_screendiff.sh','screens-d2.json','screens-d3.json')
edit('crates/fb-screendiff/run_screendiff.sh','fbv2_int_d1_*|fbv2_int_d2_*','fbv2_int_d1_*|fbv2_int_d2_*|fbv2_int_d3_*')
edit('crates/fb-screendiff/py/render_django.py','"fbv2_int_d2_"','"fbv2_int_d2_", "fbv2_int_d3_"')

# Context-specific id maps for the new record families and the OFF company's invoice choices.
edit('crates/fb-screendiff/py/render_django.py','    for name, screen in SCREENS.items():','''    from directory.models import Vendor
    from salestax.models import TaxItem
    from salestax.services import bulk_preview
    for row in Vendor.objects.filter(company=company).order_by('pk'):
        ids['vendor:' + row.display_name] = str(row.pk)
        ids['label:vendor:' + row.display_name] = str(row.pk)
    for row in TaxItem.objects.filter(company=company):
        ids['taxitem:' + row.name] = str(row.pk)
    ids['bulk_digest:current'] = bulk_preview(company)['digest']
    for other in Company.objects.exclude(pk=company.pk):
        for model, field, cls in [('account','code',Account),('customer','display_name',Customer),('item','code',Item)]:
            for row in cls.objects.filter(company=other).order_by('pk'):
                key = model + ':' + other.name + ':' + getattr(row,field)
                ids[key] = str(row.pk)
                ids['label:' + key] = str(row.pk)
    from unittest.mock import patch
    for name, screen in SCREENS.items():''')

edit('crates/fb-screendiff/src/main.rs','            let queries: [(&str, &str); 6]', '            let queries: [(&str, &str); 8]')
edit('crates/fb-screendiff/src/main.rs','                ("account", "SELECT code, id', '                ("vendor", "SELECT display_name, id FROM directory_vendor WHERE company_id=?1"),\n                ("taxitem", "SELECT name, id FROM salestax_taxitem WHERE company_id=?1"),\n                ("account", "SELECT code, id')
edit('crates/fb-screendiff/src/main.rs','            Ok(ids)\n', '''            ids.insert("bulk_digest:current".into(),fb_core::salestax::bulk_preview(tx,fb_core::CompanyId::parse(&company).unwrap()).unwrap()["digest"].as_str().unwrap().into());
            let mut st=tx.prepare("SELECT id,name FROM ledger_company WHERE id<>?1")?;
            for row in st.query_map([&company],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {
                let (co,name)=row?;
                for (model,table,column) in [("account","ledger_account","code"),("customer","directory_customer","display_name"),("item","directory_item","code")] {
                    let mut st=tx.prepare(&format!("SELECT id,{column} FROM {table} WHERE company_id=?1 ORDER BY id"))?;
                    for (n,row) in st.query_map([&co],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.enumerate() {let (id,value)=row?;let key=format!("{model}:{name}:{value}");ids.insert(format!("label:{key}"),(n+1).to_string());ids.insert(key,id);}
                }
            }
            Ok(ids)
''')
# Fix the first-run invoice's tax-ON picker/default. OFF behavior is unchanged.
edit('crates/fb-app/src/screens.rs','    tax: Vec<(String, String)>,','    tax: Vec<(String, String)>,\n    tax_initial: String,')
edit('crates/fb-app/src/screens.rs','let mut t = vec![("NONE".to_owned(), "No tax".to_owned())];','let mut t = vec![(String::new(), "Choose sales tax".into()), ("NONE".to_owned(), "No tax".to_owned())];')
edit('crates/fb-app/src/screens.rs','            tax,\n            catalog:', '            tax_initial: if settings["charge_tax"]==true {settings["default_item_id"].as_str().filter(|id|tax.iter().any(|(v,_)|v==&format!("T{id}"))).map(|id|format!("T{id}")).unwrap_or_default()}else{"NONE".into()},\n            tax,\n            catalog:')
edit('crates/fb-app/src/screens.rs','d("tax_mode").unwrap_or_else(|| ch.tax[0].0.clone())','d("tax_mode").unwrap_or_else(|| ch.tax_initial.clone())')
