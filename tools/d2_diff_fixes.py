"""Corrections exposed by the first complete 41-state screen comparison."""
from pathlib import Path


def replace(path,old,new):
    p=Path(path);t=p.read_text();assert old in t,(path,old);p.write_text(t.replace(old,new))


replace('crates/fb-templates/tools/d2_hand_fixes.py',"        ('{{ line.account }}', '{{ line.account.label }}'),", "        ('{{ line.account }}', '{{ line.account.label }}') if name == 'ledger/journal' else ('', ''),")
replace('crates/fb-templates/src/forms.rs','''            Widget::Date => format!(r#"<input type="date" name="{name}"{}{}>"#, value_attr(&self.value), self.attrs_html()),''','''            Widget::Date => format!(r#"<input type="date" name="{name}"{}{}>"#, self.value.as_ref().map(|v|format!(r#" value="{}""#,escape(v))).unwrap_or_default(), self.attrs_html()),''')
replace('crates/fb-templates/src/forms.rs','''if *checked { " checked" } else { "" }, self.attrs_html()''','''self.attrs_html(), if *checked { " checked" } else { "" }''')
replace('crates/fb-templates/src/forms.rs','''r#"<input type="checkbox" name="lines-0-taxable" checked id="id_lines-0-taxable">"#''','''r#"<input type="checkbox" name="lines-0-taxable" id="id_lines-0-taxable" checked>"#''')
replace('crates/fb-templates/src/forms.rs','''    let mut out = error_list(non_field_errors, None);
    let hidden:''','''    let mut top_errors=non_field_errors.to_vec();
    for f in fields.iter().filter(|f|f.is_hidden()) {
        for e in &f.errors {top_errors.push(format!("(Hidden field {}) {e}",f.name));}
    }
    let mut out = error_list(&top_errors, None);
    let hidden:''')
replace('crates/fb-app/src/screens/d2.rs','''("MIN_NUM_FORMS","0".into()),("MAX_NUM_FORMS","50".into())''','''("MIN_NUM_FORMS",posted.map(|p|get(p,"lines-MIN_NUM_FORMS").unwrap_or("").to_owned()).unwrap_or("0".into())),("MAX_NUM_FORMS",posted.map(|p|get(p,"lines-MAX_NUM_FORMS").unwrap_or("").to_owned()).unwrap_or("50".into()))''')
replace('crates/fb-screendiff/src/normalize.rs','''match arg.and_then(arg_model).and_then(|m| ids.get(m, seg))''','''match arg.and_then(|arg|if arg=="record_id" {
                    if path.contains("/lists/items/"){Some("item")}else if path.contains("/lists/customers/"){Some("customer")}else if path.contains("/lists/vendors/"){Some("vendor")}else{None}
                }else{arg_model(arg)}).and_then(|m| ids.get(m, seg))''')
replace('crates/fb-harness/src/dump_shape.rs','''            ResultValue if value.is_boolean() => Ok(()),''','''            ResultValue if value.is_boolean() => Ok(()),
            ResultValue if value.get("checklist").is_some() => Object(&[
                ("empty",Bool),("revision",Int),
                ("chosen",Object(&[("receivables",Optional(&String)),("payables",Optional(&String)),("undeposited",Optional(&String)),("collected_tax",Optional(&String)),("banks",Optional(&Array(&String)))])),
                ("reasons",Object(&[("receivables",Optional(&String)),("payables",Optional(&String)),("undeposited",Optional(&String)),("collected_tax",Optional(&String)),("banks",Optional(&String))])),
                ("checklist",Object(&[("chart",Bool),("special",Bool),("tax",Bool),("customer",Bool),("vendor",Bool),("item",Bool)])),
            ]).validate(value,path),''')
