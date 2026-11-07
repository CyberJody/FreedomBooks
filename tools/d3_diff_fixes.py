"""Fix differences reported by the real Django screen comparison."""
from pathlib import Path
def edit(name,old,new):
    p=Path(name);s=p.read_text();assert old in s,(name,old);p.write_text(s.replace(old,new))
edit('crates/fb-app/src/screens/d3.rs','("email","Email",320)','("email","Email",254)')
edit('crates/fb-app/src/screens/d3.rs','if name=="email"{f=f.attr("autocapitalize","none").attr("autocomplete","email");}','')
edit('crates/fb-app/src/screens/d3.rs','if m==Model::Vendor{a.label()}else{a.label()}','a.label()')
edit('crates/fb-app/src/screens/d3.rs','let ordinals=d2::ordinals(tx,c,"ledger_account")?;','')
edit('crates/fb-app/src/screens/d3.rs','}let _=ordinals;Ok(out)','}Ok(out)')
edit('crates/fb-app/src/screens/d3.rs','if ["sale_rate","purchase_cost"].contains(&f.name.as_str())&&!v.is_empty()&&!errors.contains_key(&f.name){if fb_core::services::money_json(&json!(v)).is_err(){error(&mut errors,&f.name,"Enter a valid monetary amount.");}}','if ["sale_rate","purchase_cost"].contains(&f.name.as_str())&&!v.is_empty()&&!errors.contains_key(&f.name)&&fb_core::services::money_json(&json!(v)).is_err(){error(&mut errors,&f.name,"Enter a valid monetary amount.");}')
edit('crates/fb-screendiff/src/main.rs','else if path.ends_with("/reverse/"){path.trim_end_matches("reverse/").to_owned()}else{path.clone()};','''else if path.ends_with("/reverse/"){path.trim_end_matches("reverse/").to_owned()}
                else if path.ends_with("/status/") && path.contains("/lists/"){path.trim_end_matches("status/").to_owned()}
                else if path.ends_with("/status/") && path.contains("/sales-tax/"){format!("{}/sales-tax/rates/",path.split("/sales-tax/").next().unwrap())}
                else if path.ends_with("/reminders/settings/"){path.trim_end_matches("settings/").to_owned()}else{path.clone()};''')
