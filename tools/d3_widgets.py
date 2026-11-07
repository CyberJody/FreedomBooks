"""Django 5.2 D3 widget extensions; no bridge or security changes."""
from pathlib import Path

def edit(name,old,new):
    p=Path(name);s=p.read_text();assert old in s,(name,old);p.write_text(s.replace(old,new))

edit('crates/fb-templates/Cargo.toml','[dependencies]','[dependencies]\nchrono = { version = "0.4", default-features = false, features = ["std"] }')
edit('crates/fb-templates/src/forms.rs','    Text,','    Text,\n    Email,\n    Number,\n    Radio { choices: Vec<(String,String)> },\n    Checkboxes { choices: Vec<(String,String)>, selected: Vec<String> },')
edit('crates/fb-templates/src/forms.rs','if !self.help_text.is_empty() {described.push', 'if !self.help_text.is_empty() && !matches!(self.widget, Widget::Radio{..}|Widget::Checkboxes{..}) {described.push')
edit('crates/fb-templates/src/forms.rs','            Widget::Text => format!', '''            Widget::Radio { choices } => self.choice_list(choices, None),
            Widget::Checkboxes { choices, selected } => self.choice_list(choices, Some(selected)),
            Widget::Email => format!(r#"<input type="email" name="{name}"{}{}>"#,value_attr(&self.value),self.attrs_html()),
            Widget::Number => format!(r#"<input type="number" name="{name}"{}{}>"#,value_attr(&self.value),self.attrs_html()),
            Widget::Text => format!''')
edit('crates/fb-templates/src/forms.rs','    /// `{{ field.label_tag }}`:', '''    fn choice_list(&self, choices: &[(String,String)], multiple: Option<&Vec<String>>) -> String {
        let mut outer=format!(r#"<div id="{}""#,escape(&self.auto_id()));
        if multiple.is_none() && !self.errors.is_empty(){outer.push_str(r#" aria-invalid="true""#);}
        outer.push('>');
        for (i,(value,label)) in choices.iter().enumerate(){
            let id=format!("{}_{i}",self.auto_id());
            let selected=multiple.map(|s|s.contains(value)).unwrap_or_else(||self.value.as_deref()==Some(value.as_str()));
            outer.push_str(&format!(r#"<div><label for="{}"><input type="{}" name="{}" value="{}"{} id="{}"{}> {}{}</label></div>"#,
                escape(&id),if multiple.is_some(){"checkbox"}else{"radio"},escape(&self.html_name()),escape(value),
                if self.required && multiple.is_none(){" required"}else{""},escape(&id),if selected{" checked"}else{""},escape(label),""));
        }
        outer.push_str("</div>");outer
    }

    /// `{{ field.label_tag }}`:''')
edit('crates/fb-templates/src/forms.rs','escape(&self.auto_id()), escape(&self.label), suffix)', 'escape(&if matches!(self.widget,Widget::Radio{..}){format!("{}_0",self.auto_id())}else if matches!(self.widget,Widget::Checkboxes{..}){String::new()}else{self.auto_id()}), escape(&self.label), suffix)')
edit('crates/fb-templates/src/forms.rs','        let suffix = if self.label.ends_with', '''        if matches!(self.widget,Widget::Checkboxes{..}) {return format!("{}{}",escape(&self.label),if self.label.ends_with([':', '?', '.', '!']){""}else{":"});}
        let suffix = if self.label.ends_with''')
# Tax list has one POST target per row: a token must be bound to that row's action.
edit('tools/d3_hand_fixes.py', "    if name == 'reminders/page':", "    if name == 'salestax/list':\n        changes.append(('{{ csrf_token|safe }}', '{{ status_tokens[item.pk]|safe }}'))\n    if name == 'reminders/page':")
edit('crates/fb-app/src/screens/d3.rs','a.get_detail_type_display()','a.detail_type.label()')
