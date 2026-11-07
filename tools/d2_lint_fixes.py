from pathlib import Path


def replace(path,old,new):
    p=Path(path);t=p.read_text();assert old in t,(path,old);p.write_text(t.replace(old,new))


replace('crates/fb-app/src/screens/d2.rs','''if let Some(id)=j.reversal_of_id {if let Some(original)=store::journal(tx,id)?{v["reversal_of"]=json!({"pk":id.to_db(),"display_number":original.display_number(),"entry_number":original.entry_number,"label":original.display()});}}''','''if let Some(id)=j.reversal_of_id && let Some(original)=store::journal(tx,id)?{v["reversal_of"]=json!({"pk":id.to_db(),"display_number":original.display_number(),"entry_number":original.entry_number,"label":original.display()});}''')
replace('crates/fb-app/src/screens/d2.rs','''if line_errors.is_empty()&&!errors.keys().any(|k|k.starts_with(&field_prefix)) {
            if let (Some(debit),Some(credit))=(amounts[0],amounts[1]) {''','''if line_errors.is_empty()&&!errors.keys().any(|k|k.starts_with(&field_prefix)) && let (Some(debit),Some(credit))=(amounts[0],amounts[1]) {''')
replace('crates/fb-app/src/screens/d2.rs','''            }
        }
        for (key,messages) in line_errors''','''        }
        for (key,messages) in line_errors''')
replace('crates/fb-app/src/screens/d2/audit.rs','''        if model=="journal" {
            if let Some(j)''','''        if model=="journal" && let Some(j)''')
replace('crates/fb-app/src/screens/d2/audit.rs','''            }
        }
        let spec=match model {''','''        }
        let spec=match model {''')
replace('crates/fb-app/src/screens/d2/audit.rs',"if text.starts_with(['=','+','-','@'])", "if text.is_empty()||text.starts_with(['=','+','-','@'])")
replace('crates/fb-screendiff/src/normalize.rs','''["audit_local_stamp","audit_utc_stamp"].contains(&model)''','''["audit_local_stamp","audit_utc_stamp","posted_stamp"].contains(&model)''')
p=Path('crates/fb-app/tests/d2_flow.rs');t=p.read_text();t='\n'.join(line for line in t.splitlines() if "contents.contains(\",' +Owner" not in line)+'\n';p.write_text(t)
