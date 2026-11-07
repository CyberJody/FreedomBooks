"""Second comparison corrections, plus the strict reminder result shape."""
from pathlib import Path
def edit(name,old,new):
    p=Path(name);s=p.read_text();assert old in s,(name,old);p.write_text(s.replace(old,new))
edit('crates/fb-screendiff/src/normalize.rs','Some(("customer", v)) => format!', 'Some(("vendor", v)) => format!("vendor={}", ids.get("vendor", v).unwrap_or(v)),\n                    Some(("customer", v)) => format!')
edit('crates/fb-templates/src/forms.rs','if multiple.is_none() && !self.errors.is_empty(){outer.push_str(r#" aria-invalid="true""#);}','')
edit('crates/fb-templates/src/forms.rs','if self.required && multiple.is_none(){" required"}else{""},escape(&id)', 'format!("{}{}",if self.required && multiple.is_none(){" required"}else{""},if self.errors.is_empty(){""}else{r#" aria-invalid="true""#}),escape(&id)')
edit('crates/fb-templates/src/forms.rs','return format!("{}{}",escape(&self.label),if self.label.ends_with', 'return format!("<label>{}{}</label>",escape(&self.label),if self.label.ends_with')
edit('crates/fb-app/src/screens/d3/tax.rs','.required().attr("minlength","64").attr("maxlength","64").value(data_value', '.required().value(data_value')
edit('crates/fb-screendiff/src/main.rs','vm["note"]=serde_json::json!(fb_templates::django::print_toolbar_note(vm["audience"].as_str().unwrap(),false));','if let Some(audience)=vm["audience"].as_str(){vm["note"]=serde_json::json!(fb_templates::django::print_toolbar_note(audience,false));}')
edit('crates/fb-harness/src/dump_shape.rs','            ResultValue if value.is_boolean() => Ok(()),','''            ResultValue if value.is_boolean() => Ok(()),
            ResultValue if value.get("today").is_some() => Object(&[
                ("today",String),("days",Int),("through",String),
                ("bills",REMINDER_GROUP),("invoices",REMINDER_GROUP),
            ]).validate(value,path),''')
edit('crates/fb-harness/src/dump_shape.rs','const LINE: Shape = Object(&[','''const REMINDER_GROUP: Shape = Object(&[
    ("count",Int),("total",String),("overdue_count",Int),("overdue_total",String),("capped",Bool),
    ("rows",Array(&Object(&[("document_id",String),("number",String),("party",String),("due_date",String),("days",Int),("balance",String),("overdue",Bool),("when",String)]))),
]);
const LINE: Shape = Object(&[''')
