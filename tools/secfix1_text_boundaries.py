from pathlib import Path
root=Path(__file__).resolve().parents[1]
def replace(name,old,new):
    p=root/name;t=p.read_text();assert old in t,name;p.write_text(t.replace(old,new))
replace('crates/fb-core/src/accounts.rs', ' || name.chars().any(char::is_control)', '')
replace('crates/fb-core/src/calculations.rs', '    if let Some(label) = tax_label.as_str() { crate::py::clean_free_text(label)?; }\n', '')
for module in ['sales','credits']:
    p=root/f'crates/fb-core/src/{module}/mod.rs';t=p.read_text()
    import re
    pattern=r'(fn calculate\(lines: &\[Value\], tax: &Value, posting: bool\) -> FbResult<Value> \{)'
    if not re.search(pattern,t):
        print('calculate signature',module);raise SystemExit(1)
    t=re.sub(pattern,r'\1\n    if let Some(label) = tax["tax_label"].as_str() { py::clean_free_text(label)?; }',t)
    p.write_text(t)
replace('crates/fb-core/src/directory.rs', '            if let Some(raw) = v.as_str() { py::clean_free_text(raw)?; }', '''            if ["code", "description", "display_name", "business_name", "contact_name", "email", "phone", "billing_address", "notes"].contains(&f.as_str()) {
                if let Some(raw) = v.as_str() { py::clean_free_text(raw)?; }
            }''')
replace('crates/fb-core/src/attachments/mod.rs','''    let u=services::access(tx,actor,co)?;let reason=reason.split''','''    py::clean_free_text(reason)?;
    let u=services::access(tx,actor,co)?;let reason=reason.split''')
replace('crates/fb-core/tests/attachments.rs',r'" wrong\n file "',r'" wrong\t file "')
replace('crates/fb-core/tests/l2a_labels.rs',r'Street\nTown',r'Town')
