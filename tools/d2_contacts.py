"""Complete rule 5 for fallback #id labels on duplicate customer choices and invoice rows."""
from pathlib import Path
import json


def replace(path,old,new):
    p=Path(path);t=p.read_text();assert old in t,(path,old);p.write_text(t.replace(old,new))


replace('crates/fb-app/src/screens/d2.rs','''["ledger_account", "directory_item"]''','''["ledger_account", "directory_item", "directory_customer", "directory_vendor"]''')
replace('crates/fb-app/src/screens.rs','''        let dups = fb_core::party_display::duplicate_active_display_names(tx, c, fb_core::directory::Model::Customer)?;''','''        let ordinals=d2::ordinals(tx,c,"directory_customer")?;
        let dups = fb_core::party_display::duplicate_active_display_names(tx, c, fb_core::directory::Model::Customer)?;''')
replace('crates/fb-app/src/screens.rs','''fb_core::party_display::contact_list_label(r, &dups, None, "")''','''{
                let id=r["id"].as_str().unwrap_or_default();
                fb_core::party_display::contact_list_label(r, &dups, None, "").replace(&format!("(#{id})"),&format!("(#{})",ordinals[id]))
            }''')
replace('crates/fb-app/src/screens.rs','''let labels = open.book.read(|tx| fb_core::party_display::contact_label_map(tx, c, fb_core::directory::Model::Customer, None))?;''','''let labels = open.book.read(|tx| -> Result<Value,FbError> {
            let mut labels=fb_core::party_display::contact_label_map(tx,c,fb_core::directory::Model::Customer,None)?;
            let ordinals=d2::ordinals(tx,c,"directory_customer")?;
            for (id,label) in labels.as_object_mut().unwrap() {
                if let Some(text)=label.as_str(){*label=json!(text.replace(&format!("(#{id})"),&format!("(#{})",ordinals[id])));}
            }
            Ok(labels)
        })?;''')
replace('crates/fb-app/src/screens.rs','''    let text = errors.join(" ");
    Ok(app.screens.render("desktop/not_ported.html", &Vm { shell, title: "Action not completed", path: text })?)''','''    let mut vm=serde_json::to_value(shell).map_err(|e|AppError::Invalid(e.to_string()))?;
    vm["errors"]=json!(errors);
    Ok(app.screens.render("ledger/error.html",&vm)?)''')
# Remove the now-unused old placeholder view model, in the failure function only.
p=Path('crates/fb-app/src/screens.rs');t=p.read_text();a=t.index('fn failure(');b=t.index('    let mut vm=',a);t=t[:a]+t[a:b].split('{',1)[0]+'{\n'+t[b:];p.write_text(t)

replace('crates/fb-screendiff/src/normalize.rs','''for (key,value) in ids {if let Some(rest)=key.strip_prefix("label:") {if let Some((model,_))=rest.split_once(':') {if let Some(raw)=ids.get(rest) {labels.insert((model.into(),raw.clone()),value.clone());}}}}''','''for (key,value) in ids {if let Some(rest)=key.strip_prefix("label:") && let Some((model,_))=rest.split_once(':') && let Some(raw)=ids.get(rest) {labels.insert((model.into(),raw.clone()),value.clone());}}''')
replace('crates/fb-screendiff/py/render_django.py','''    for customer in Customer.objects.filter(company=company):
        ids[f"customer:{customer.display_name}"] = str(customer.pk)''','''    from collections import Counter
    customers = list(Customer.objects.filter(company=company).order_by('pk'))
    names = Counter(customer.display_name for customer in customers)
    for ordinal, customer in enumerate(customers, 1):
        key = f"customer:{customer.display_name}" + (f":#{ordinal}" if names[customer.display_name] > 1 else '')
        ids[key] = str(customer.pk)
        ids['label:' + key] = str(customer.pk)''')
replace('crates/fb-screendiff/src/main.rs','''                for row in st.query_map([&company], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
                    let (k, v) = row?;
                    ids.insert(format!("{model}:{k}"), v);
                }''','''                let mut rows=st.query_map([&company], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
                if model=="customer" {rows.sort_by(|a,b|a.1.cmp(&b.1));}
                let mut counts=std::collections::HashMap::new();for (key,_) in &rows{*counts.entry(key.clone()).or_insert(0)+=1;}
                for (n,(k,v)) in rows.into_iter().enumerate() {
                    let key=format!("{model}:{k}{}",if model=="customer"&&counts[&k]>1{format!(":#{}",n+1)}else{String::new()});
                    if model=="customer" {ids.insert(format!("label:{key}"),(n+1).to_string());}
                    ids.insert(key,v);
                }''')
p=Path('crates/fb-screendiff/scenarios/screens-d2.json');s=json.loads(p.read_text());s['steps'].extend([
    dict(op='l2_save',company='c',model='customer',ref='twin1',values={'display_name':'Twin buyer'},expect_error=False),
    dict(op='l2_save',company='c',model='customer',ref='twin2',values={'display_name':'Twin buyer'},expect_error=False),
]);p.write_text(json.dumps(s,indent=2)+'\n')
p=Path('crates/fb-screendiff/screens.json');s=json.loads(p.read_text());s['register-invalid']={'path':'/c/{company}/accounts/{account:1100}/register/?start=bad','status':400};p.write_text(json.dumps(s,indent=2)+'\n')
