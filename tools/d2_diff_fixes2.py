from pathlib import Path


def replace(path,old,new):
    p=Path(path);t=p.read_text();assert old in t,(path,old);p.write_text(t.replace(old,new))


replace('crates/fb-app/src/screens/d2.rs','''Field::new("postable","Postable",Widget::Checkbox{checked:on("postable",a.as_ref().is_none_or(|a|a.postable))}),''','''Field::new("postable","Postable",Widget::Checkbox{checked:on("postable",a.as_ref().is_none_or(|a|a.postable))}).help("Turn off for summary-only accounts."),''')
for name in ['start','end']:
    replace('crates/fb-app/src/screens.rs',f'.value(get(params, "{name}").unwrap_or(""))',f'.value_opt(get(params, "{name}").map(str::to_owned))')
replace('crates/fb-app/src/screens.rs','''("MIN_NUM_FORMS", "1".into()), ("MAX_NUM_FORMS", "50".into())''','''("MIN_NUM_FORMS", if posted.is_some() {d("lines-MIN_NUM_FORMS").unwrap_or_default()} else {"1".into()}), ("MAX_NUM_FORMS", if posted.is_some() {d("lines-MAX_NUM_FORMS").unwrap_or_default()} else {"50".into()})''')
replace('crates/fb-harness/src/l2a.rs','''proposal["chosen"]["banks"].as_array_mut()''','''proposal.get_mut("chosen").and_then(|v|v.get_mut("banks")).and_then(Value::as_array_mut)''')
