"""Consume global Python pks in a second company, proving ordinals remain per company."""
from pathlib import Path
import json


def replace(path,old,new):
    p=Path(path);t=p.read_text();assert old in t,(path,old);p.write_text(t.replace(old,new))


replace('crates/fb-screendiff/src/normalize.rs','''b.replace("2&amp;quot;","3&amp;quot;")''','''b.replace("2&quot;","3&quot;")''')
replace('crates/fb-screendiff/py/render_django.py','''    ids = {"company": str(company.pk)}''','''    ids = {"company": str(company.pk)}
    for member_company in user.companies.all():
        ids['company:' + member_company.name] = str(member_company.pk)''')
replace('crates/fb-screendiff/src/main.rs','''            ids.insert("company".to_owned(), company.clone());''','''            ids.insert("company".to_owned(), company.clone());
            let mut companies=tx.prepare("SELECT name,id FROM ledger_company")?;
            for row in companies.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {let (name,id)=row?;ids.insert(format!("company:{name}"),id);}''')
p=Path('crates/fb-screendiff/scenarios/screens-d2.json');s=json.loads(p.read_text());s['steps'][1:1]=[
    dict(op='create_company',ref='other',name='Other synthetic books',expect_error=False),
    dict(op='save_account',company='other',ref='other-income',code='4000',name='Other income',detail_type='INCOME',expect_error=False),
    dict(op='l2_save',company='other',model='customer',ref='other-buyer',values={'display_name':'Other buyer'},expect_error=False),
    dict(op='l2_save',company='other',model='item',ref='other-item',values={'code':'OTHER','description':'Other item','kind':'SERVICE','sale_rate':'1.00','income_account':'other-income'},expect_error=False),
];p.write_text(json.dumps(s,indent=2)+'\n')
