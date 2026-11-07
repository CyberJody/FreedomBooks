"""Asserted D2 shared form-widget extension and template registration."""
from pathlib import Path


def replace(path, old, new):
    p = Path(path)
    text = p.read_text()
    assert old in text, (path, old)
    p.write_text(text.replace(old, new))


replace('crates/fb-core/src/lib.rs','pub mod first_run;','pub mod first_run;\nmod first_run_words;')
entries = ['ledger/' + name for name in ['home', 'company', 'form', 'error', 'events', 'history',
         'journal', 'journal_form', 'report_problem', 'setup_start', 'setup_fill', 'trial_balance',
         'includes/print_toolbar_note']] + ['documents/setup']
replace('crates/fb-templates/src/lib.rs', 'const TEMPLATES: &[(&str, &str)] = &[',
        'const TEMPLATES: &[(&str, &str)] = &[\n' + ''.join(f'    ("{name}.html", include_str!("../templates/{name}.html")),\n' for name in entries))
f='crates/fb-templates/src/forms.rs'
replace(f,'    Select { choices: Vec<(String, String)> },','''    Select { choices: Vec<(String, String)> },
    GroupedSelect { groups: Vec<(String, Vec<(String, String)>)> },
    MultipleSelect { choices: Vec<(String, String)>, selected: Vec<String> },''')
replace(f,'    pub errors: Vec<String>,','    pub errors: Vec<String>,\n    pub help_text: String,')
replace(f,'attrs: vec![], errors: vec![]','attrs: vec![], errors: vec![], help_text: String::new()')
replace(f,'    pub fn html_name(&self)', '''    pub fn help(mut self, text: impl Into<String>) -> Field { self.help_text=text.into(); self }
    pub fn html_name(&self)''')
replace(f,'            Widget::Select { choices } => choices.first().is_some_and(|(v, _)| v.is_empty()),','''            Widget::Select { choices } => choices.first().is_some_and(|(v, _)| v.is_empty()),
            Widget::GroupedSelect { groups } => groups.first().is_some_and(|(label, choices)| label.is_empty() && choices.first().is_some_and(|(v,_)|v.is_empty())),''')
replace(f,'        out.push_str(&format!(r#" id="{}""#, escape(&self.auto_id())));','''        if !self.is_hidden() && !self.errors.is_empty() { out.push_str(r#" aria-invalid="true""#); }
        if !self.is_hidden() {
            let mut described=vec![];
            if !self.help_text.is_empty() {described.push(format!("{}_helptext",self.auto_id()));}
            if !self.errors.is_empty() {described.push(format!("{}_error",self.auto_id()));}
            if !described.is_empty() {out.push_str(&format!(r#" aria-describedby="{}""#,described.join(" ")));}
        }
        out.push_str(&format!(r#" id="{}""#, escape(&self.auto_id())));''')
replace(f,'            Widget::Select { choices } => {','''            Widget::MultipleSelect { choices, selected } => {
                let mut out=format!(r#"<select name="{name}"{} multiple>"#,self.attrs_html());
                for (value,label) in choices {
                    let mark=if selected.contains(value) {" selected"} else {""};
                    out.push_str(&format!("\\n  <option value=\\"{}\\"{mark}>{}</option>\\n",escape(value),escape(label)));
                }
                out.push_str("\\n</select>"); out
            }
            Widget::GroupedSelect { groups } => {
                let selected=self.value.clone().unwrap_or_default();
                let mut out=format!(r#"<select name="{name}"{}>"#,self.attrs_html());
                for (group,choices) in groups {
                    if !group.is_empty(){out.push_str(&format!(r#"<optgroup label="{}">"#,escape(group)));}
                    for (value,label) in choices {
                        let mark=if *value==selected {" selected"}else{""};
                        out.push_str(&format!("\\n  <option value=\\"{}\\"{mark}>{}</option>\\n",escape(value),escape(label)));
                    }
                    if !group.is_empty(){out.push_str("</optgroup>");}
                }
                out.push_str("\\n</select>"); out
            }
            Widget::Select { choices } => {''')
replace(f,'    pub is_hidden: bool,','    pub is_hidden: bool,\n    pub id_for_label: String,\n    pub help_text: String,')
replace(f,'            is_hidden: f.is_hidden(),','            is_hidden: f.is_hidden(),\n            id_for_label: f.auto_id(),\n            help_text: f.help_text.clone(),')
replace(f,'        out.push_str(&format!("\\n    {}\\n    \\n    ", f.widget_html()));','''        out.push_str(&format!("\\n    {}\\n    ", f.widget_html()));
        if !f.help_text.is_empty() {
            out.push_str(&format!(r#"<span class="helptext" id="{}_helptext">{}</span>"#,escape(&f.auto_id()),f.help_text));
        }
        out.push_str("\\n    ");''')
