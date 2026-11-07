"""Final provenance, shared-widget regressions, and local build-artifact exclusions."""
from pathlib import Path


def replace(path,old,new):
    p=Path(path);t=p.read_text();assert old in t,(path,old);p.write_text(t.replace(old,new))


replace('crates/fb-templates/src/forms.rs','''let mut n=0u64;let mut digit_before=false;let mut any=false;''','''let mut n=0u64;let mut digit_before=false;let mut any=false;let mut count=0usize;''')
replace('crates/fb-templates/src/forms.rs','''let d=fb_money::unicode_decimal(c)? as u64;''','''let d=fb_money::unicode_decimal(c)? as u64;
        count+=1;if count>4300{return None;}''')
replace('crates/fb-screendiff/src/normalize.rs','''Regex::new(r"(?s)<pre>(.*?)</pre>")''','''Regex::new(r"(?s)(<details><summary>View change</summary><pre>)(.*?)(</pre>)")''')
replace('crates/fb-screendiff/src/normalize.rs','''let plain=c[1].replace("&#x27;","'")''','''let plain=c[2].replace("&#x27;","'")''')
replace('crates/fb-screendiff/src/normalize.rs','''format!("<pre>{}</pre>",fb_templates::django::escape(&serde_json::to_string(&canonical(value)).unwrap()))''','''format!("{}{}{}",&c[1],fb_templates::django::escape(&serde_json::to_string(&canonical(value)).unwrap()),&c[3])''')
replace('crates/fb-app/src/screens/d2.rs','''j.entry_number.to_lowercase().contains(&q.to_lowercase())''','''fb_db::collation::upper(&j.entry_number).contains(&fb_db::collation::upper(&q))''')
replace('crates/fb-app/src/screens/d2.rs','''j.memo.to_lowercase().contains(&q.to_lowercase())''','''fb_db::collation::upper(&j.memo).contains(&fb_db::collation::upper(&q))''')
replace('crates/fb-app/src/commands.rs','''["SaveAccount", "SaveCustomer", "SaveItem", "SaveSetup", "SaveInvoice", "PostInvoice", "SubmitForm", "Register"]''','''["SaveAccount", "SaveCustomer", "SaveItem", "SaveSetup", "SaveInvoice", "PostInvoice", "SubmitForm", "Register", "SaveCompany", "SaveJournal", "PostJournal", "ReverseJournal", "FillAccountingSetup", "DismissSetup", "ExportHistory"]''')
replace('electron/scripts/smoke.cjs',"    await page.waitForFunction(async () => true);\n",'')
with Path('.gitignore').open('a') as f:f.write('\n# D2 local test books and generated Python caches\n/staging/\n__pycache__/\n')
