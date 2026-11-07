//! Django 5.2 form rendering (the default widget templates, `label_tag`, `as_p` and error lists), so a
//! ported form prints the same elements and attributes as the Django form it replaces. A view model
//! describes each field once; templates print `field.label_tag`, `field.widget` and `field.errors`
//! (all pre-escaped HTML, marked `|safe` in the templates).
use crate::django::escape;
use serde::Serialize;

/// Python int(text, 10), including Unicode digits and underscores between digits. Saturation keeps
/// arbitrary-size valid integers distinct from malformed input for page/revision bound handling.
pub fn python_integer(text: &str) -> Option<i64> {
    let text=text.trim_matches(fb_money::py_isspace);
    let (negative,digits)=if let Some(s)=text.strip_prefix('-'){(true,s)}else{(false,text.strip_prefix('+').unwrap_or(text))};
    let mut n=0u64;let mut digit_before=false;let mut any=false;let mut count=0usize;
    for c in digits.chars(){
        if c=='_' {if !digit_before{return None;}digit_before=false;continue;}
        let d=fb_money::unicode_decimal(c)? as u64;
        count+=1;if count>4300{return None;}
        n=n.saturating_mul(10).saturating_add(d);digit_before=true;any=true;
    }
    if !any||!digit_before{return None;}
    if negative {Some(if n>i64::MAX as u64{i64::MIN}else{-(n as i64)})}
    else{Some(n.min(i64::MAX as u64) as i64)}
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Widget {
    Text,
    Email,
    Number,
    Radio { choices: Vec<(String,String)> },
    Checkboxes { choices: Vec<(String,String)>, selected: Vec<String> },
    Date,
    Hidden,
    Textarea { cols: u32, rows: u32 },
    Checkbox { checked: bool },
    /// `choices` are (value, label); `selected` is the value shown as chosen.
    Select { choices: Vec<(String, String)> },
    GroupedSelect { groups: Vec<(String, Vec<(String, String)>)> },
    MultipleSelect { choices: Vec<(String, String)>, selected: Vec<String> },
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Field {
    /// Name without the form prefix (`customer`, `quantity`).
    pub name: String,
    /// Form prefix (`lines-0`), empty for unprefixed forms.
    pub prefix: String,
    pub label: String,
    pub widget: Widget,
    pub value: Option<String>,
    pub required: bool,
    /// Extra widget attributes in Django's order (e.g. `maxlength`), printed before `required` and `id`.
    pub attrs: Vec<(String, String)>,
    pub errors: Vec<String>,
    pub help_text: String,
}

impl Field {
    pub fn new(name: &str, label: &str, widget: Widget) -> Field {
        Field { name: name.into(), prefix: String::new(), label: label.into(), widget, value: None, required: false, attrs: vec![], errors: vec![], help_text: String::new() }
    }
    pub fn prefix(mut self, prefix: &str) -> Field {
        self.prefix = prefix.into();
        self
    }
    pub fn value(mut self, value: impl Into<String>) -> Field {
        self.value = Some(value.into());
        self
    }
    pub fn value_opt(mut self, value: Option<String>) -> Field {
        self.value = value;
        self
    }
    pub fn required(mut self) -> Field {
        self.required = true;
        self
    }
    pub fn attr(mut self, key: &str, value: impl Into<String>) -> Field {
        self.attrs.push((key.into(), value.into()));
        self
    }
    pub fn errors(mut self, errors: Vec<String>) -> Field {
        self.errors = errors;
        self
    }
    pub fn help(mut self, text: impl Into<String>) -> Field { self.help_text=text.into(); self }
    pub fn html_name(&self) -> String {
        if self.prefix.is_empty() { self.name.clone() } else { format!("{}-{}", self.prefix, self.name) }
    }
    pub fn auto_id(&self) -> String {
        format!("id_{}", self.html_name())
    }
    pub fn is_hidden(&self) -> bool {
        matches!(self.widget, Widget::Hidden)
    }

    fn attrs_html(&self) -> String {
        let mut out = String::new();
        for (k, v) in &self.attrs {
            out.push_str(&format!(r#" {}="{}""#, escape(k), escape(v)));
        }
        // Django: BoundField.build_widget_attrs adds `required` (not for hidden inputs), then `id`.
        // `Select.use_required_attribute`: only when the first choice is the empty placeholder.
        let select_ok = match &self.widget {
            Widget::Select { choices } => choices.first().is_some_and(|(v, _)| v.is_empty()),
            Widget::GroupedSelect { groups } => groups.first().is_some_and(|(label, choices)| {
                label.is_empty() && choices.first().is_some_and(|(v, _)| v.is_empty())
            }),
            _ => true,
        };
        if self.required && select_ok && !self.is_hidden() && !matches!(self.widget, Widget::Checkbox { .. }) {
            out.push_str(" required");
        }
        if !self.is_hidden() && !self.errors.is_empty() { out.push_str(r#" aria-invalid="true""#); }
        if !self.is_hidden() {
            let mut described=vec![];
            if !self.help_text.is_empty() && !matches!(self.widget, Widget::Radio{..}|Widget::Checkboxes{..}) {described.push(format!("{}_helptext",self.auto_id()));}
            if !self.errors.is_empty() {described.push(format!("{}_error",self.auto_id()));}
            if !described.is_empty() {out.push_str(&format!(r#" aria-describedby="{}""#,described.join(" ")));}
        }
        out.push_str(&format!(r#" id="{}""#, escape(&self.auto_id())));
        out
    }

    /// `{{ field }}`: the widget's HTML.
    pub fn widget_html(&self) -> String {
        let name = escape(&self.html_name());
        let value_attr = |v: &Option<String>| match v {
            Some(v) if !v.is_empty() => format!(r#" value="{}""#, escape(v)),
            _ => String::new(),
        };
        match &self.widget {
            Widget::Radio { choices } => self.choice_list(choices, None),
            Widget::Checkboxes { choices, selected } => self.choice_list(choices, Some(selected)),
            Widget::Email => format!(r#"<input type="email" name="{name}"{}{}>"#,value_attr(&self.value),self.attrs_html()),
            Widget::Number => format!(r#"<input type="number" name="{name}"{}{}>"#,value_attr(&self.value),self.attrs_html()),
            Widget::Text => format!(r#"<input type="text" name="{name}"{}{}>"#, value_attr(&self.value), self.attrs_html()),
            Widget::Date => format!(r#"<input type="date" name="{name}"{}{}>"#, self.value.as_ref().map(|v|format!(r#" value="{}""#,escape(v))).unwrap_or_default(), self.attrs_html()),
            Widget::Hidden => format!(r#"<input type="hidden" name="{name}"{}{}>"#, value_attr(&self.value), self.attrs_html()),
            Widget::Textarea { cols, rows } => {
                let mut attrs = format!(r#" cols="{cols}" rows="{rows}""#);
                attrs.push_str(&self.attrs_html());
                format!("<textarea name=\"{name}\"{attrs}>\n{}</textarea>", self.value.as_deref().map(escape).unwrap_or_default())
            }
            Widget::Checkbox { checked } => {
                format!(r#"<input type="checkbox" name="{name}"{}{}>"#, self.attrs_html(), if *checked { " checked" } else { "" })
            }
            Widget::MultipleSelect { choices, selected } => {
                let mut out=format!(r#"<select name="{name}"{} multiple>"#,self.attrs_html());
                for (value,label) in choices {
                    let mark=if selected.contains(value) {" selected"} else {""};
                    out.push_str(&format!("\n  <option value=\"{}\"{mark}>{}</option>\n",escape(value),escape(label)));
                }
                out.push_str("\n</select>"); out
            }
            Widget::GroupedSelect { groups } => {
                let selected=self.value.clone().unwrap_or_default();
                let mut out=format!(r#"<select name="{name}"{}>"#,self.attrs_html());
                for (group,choices) in groups {
                    if !group.is_empty(){out.push_str(&format!(r#"<optgroup label="{}">"#,escape(group)));}
                    for (value,label) in choices {
                        let mark=if *value==selected {" selected"}else{""};
                        out.push_str(&format!("\n  <option value=\"{}\"{mark}>{}</option>\n",escape(value),escape(label)));
                    }
                    if !group.is_empty(){out.push_str("</optgroup>");}
                }
                out.push_str("\n</select>"); out
            }
            Widget::Select { choices } => {
                let selected = self.value.clone().unwrap_or_default();
                let mut out = format!(r#"<select name="{name}"{}>"#, self.attrs_html());
                for (value, label) in choices {
                    let mark = if *value == selected { " selected" } else { "" };
                    out.push_str(&format!("\n  <option value=\"{}\"{mark}>{}</option>\n", escape(value), escape(label)));
                }
                out.push_str("\n</select>");
                out
            }
        }
    }

    fn choice_list(&self, choices: &[(String,String)], multiple: Option<&Vec<String>>) -> String {
        let mut outer=format!(r#"<div id="{}""#,escape(&self.auto_id()));
        outer.push('>');
        for (i,(value,label)) in choices.iter().enumerate(){
            let id=format!("{}_{i}",self.auto_id());
            let selected=multiple.map(|s|s.contains(value)).unwrap_or_else(||self.value.as_deref()==Some(value.as_str()));
            outer.push_str(&format!(r#"<div><label for="{}"><input type="{}" name="{}" value="{}"{} id="{}"{}> {}{}</label></div>"#,
                escape(&id),if multiple.is_some(){"checkbox"}else{"radio"},escape(&self.html_name()),escape(value),
                format_args!("{}{}",if self.required && multiple.is_none(){" required"}else{""},if self.errors.is_empty(){""}else{r#" aria-invalid="true""#}),escape(&id),if selected{" checked"}else{""},escape(label),""));
        }
        outer.push_str("</div>");outer
    }

    /// `{{ field.label_tag }}`: `<label for="id_x">Label:</label>` (the suffix is skipped when the label
    /// already ends in punctuation, as Django does).
    pub fn label_tag(&self) -> String {
        if matches!(self.widget,Widget::Checkboxes{..}|Widget::Radio{..}) {return format!("<label>{}{}</label>",escape(&self.label),if self.label.ends_with([':', '?', '.', '!']){""}else{":"});}
        let suffix = if self.label.ends_with([':', '?', '.', '!']) { "" } else { ":" };
        format!(r#"<label for="{}">{}{}</label>"#, escape(&if matches!(self.widget,Widget::Radio{..}){format!("{}_0",self.auto_id())}else if matches!(self.widget,Widget::Checkboxes{..}){String::new()}else{self.auto_id()}), escape(&self.label), suffix)
    }

    /// `{{ field.errors }}`: Django 5.2's `<ul class="errorlist" id="id_x_error">`, or nothing.
    pub fn errors_html(&self) -> String {
        error_list(&self.errors, Some(&format!("{}_error", self.auto_id())))
    }
}

/// An `ErrorList` as Django prints it (`nonfield` lists carry that extra class and no id).
pub fn error_list(errors: &[String], id: Option<&str>) -> String {
    if errors.is_empty() {
        return String::new();
    }
    let mut out = match id {
        Some(id) => format!(r#"<ul class="errorlist" id="{}">"#, escape(id)),
        None => r#"<ul class="errorlist nonfield">"#.to_owned(),
    };
    for e in errors {
        out.push_str(&format!("<li>{}</li>", escape(e)));
    }
    out.push_str("</ul>");
    out
}

/// What a template needs for one field, already rendered.
#[derive(Clone, Debug, Serialize)]
pub struct BoundField {
    pub name: String,
    pub html_name: String,
    pub value: Option<String>,
    pub label: String,
    pub label_tag: String,
    pub widget: String,
    pub errors: String,
    pub is_hidden: bool,
    pub id_for_label: String,
    pub help_text: String,
}

impl From<&Field> for BoundField {
    fn from(f: &Field) -> BoundField {
        BoundField {
            name: f.name.clone(),
            html_name: f.html_name(),
            value: f.value.clone(),
            label: f.label.clone(),
            label_tag: f.label_tag(),
            widget: f.widget_html(),
            errors: f.errors_html(),
            is_hidden: f.is_hidden(),
            id_for_label: f.auto_id(),
            help_text: f.help_text.clone(),
        }
    }
}

/// A rendered form: visible and hidden fields in declaration order, plus `as_p` and non-field errors.
#[derive(Clone, Debug, Serialize, Default)]
pub struct BoundForm {
    pub fields: Vec<BoundField>,
    /// Fields by name (`form.f.request_key`), for templates that print one field (`{{ form.request_key }}`).
    pub f: std::collections::BTreeMap<String, BoundField>,
    pub visible_fields: Vec<BoundField>,
    pub hidden_fields: Vec<BoundField>,
    pub non_field_errors: String,
    pub as_p: String,
    pub has_errors: bool,
}

pub fn bind(fields: &[Field], non_field_errors: &[String]) -> BoundForm {
    let bound: Vec<BoundField> = fields.iter().map(BoundField::from).collect();
    BoundForm {
        visible_fields: bound.iter().filter(|f| !f.is_hidden).cloned().collect(),
        hidden_fields: bound.iter().filter(|f| f.is_hidden).cloned().collect(),
        non_field_errors: error_list(non_field_errors, None),
        as_p: as_p(fields, non_field_errors),
        has_errors: !non_field_errors.is_empty() || fields.iter().any(|f| !f.errors.is_empty()),
        f: bound.iter().map(|b| (b.name.clone(), b.clone())).collect(),
        fields: bound,
    }
}

/// Django 5.2 `django/forms/p.html`.
pub fn as_p(fields: &[Field], non_field_errors: &[String]) -> String {
    let mut top_errors=non_field_errors.to_vec();
    for f in fields.iter().filter(|f|f.is_hidden()) {
        for e in &f.errors {top_errors.push(format!("(Hidden field {}) {e}",f.name));}
    }
    let mut out = error_list(&top_errors, None);
    let hidden: String = fields.iter().filter(|f| f.is_hidden()).map(|f| f.widget_html()).collect();
    let visible: Vec<&Field> = fields.iter().filter(|f| !f.is_hidden()).collect();
    for (i, f) in visible.iter().enumerate() {
        out.push_str(&format!("\n  {}\n  <p>\n    ", f.errors_html()));
        if !f.label.is_empty() {
            out.push_str(&f.label_tag());
        }
        out.push_str(&format!("\n    {}\n    ", f.widget_html()));
        if !f.help_text.is_empty() {
            out.push_str(&format!(r#"<span class="helptext" id="{}_helptext">{}</span>"#,escape(&f.auto_id()),f.help_text));
        }
        out.push_str("\n    ");
        if i + 1 == visible.len() {
            out.push_str(&hidden);
        }
        out.push_str("\n  </p>\n  ");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widgets_match_django_markup() {
        let f = Field::new("customer", "Customer", Widget::Select { choices: vec![("".into(), "Choose…".into()), ("1".into(), "A & B".into())] })
            .required()
            .value("");
        assert_eq!(f.label_tag(), r#"<label for="id_customer">Customer:</label>"#);
        assert_eq!(
            f.widget_html(),
            "<select name=\"customer\" required id=\"id_customer\">\n  <option value=\"\" selected>Choose…</option>\n\n  <option value=\"1\">A &amp; B</option>\n\n</select>"
        );
        let memo = Field::new("memo", "Memo", Widget::Textarea { cols: 40, rows: 2 }).attr("maxlength", "2000");
        assert_eq!(memo.widget_html(), "<textarea name=\"memo\" cols=\"40\" rows=\"2\" maxlength=\"2000\" id=\"id_memo\">\n</textarea>");
        let q = Field::new("quantity", "Quantity", Widget::Text).prefix("lines-0").attr("maxlength", "40");
        assert_eq!(q.widget_html(), r#"<input type="text" name="lines-0-quantity" maxlength="40" id="id_lines-0-quantity">"#);
        let hidden = Field::new("request_key", "Request key", Widget::Hidden).required().value("k");
        assert_eq!(hidden.widget_html(), r#"<input type="hidden" name="request_key" value="k" id="id_request_key">"#);
        let check = Field::new("taxable", "Taxable", Widget::Checkbox { checked: true }).prefix("lines-0");
        assert_eq!(check.widget_html(), r#"<input type="checkbox" name="lines-0-taxable" id="id_lines-0-taxable" checked>"#);
    }

    #[test]
    fn required_select_without_a_blank_first_choice_has_no_required_attribute() {
        let f = Field::new("tax_mode", "Sales tax", Widget::Select { choices: vec![("NONE".into(), "No tax".into())] }).required().value("NONE");
        assert!(f.widget_html().starts_with(r#"<select name="tax_mode" id="id_tax_mode">"#));
    }

    #[test]
    fn label_suffix_skips_trailing_punctuation() {
        assert_eq!(Field::new("x", "Ready?", Widget::Text).label_tag(), r#"<label for="id_x">Ready?</label>"#);
    }

    #[test]
    fn errors_render_as_django_error_lists() {
        let f = Field::new("date", "Date", Widget::Date).errors(vec!["Enter a valid date.".into()]);
        assert_eq!(f.errors_html(), r#"<ul class="errorlist" id="id_date_error"><li>Enter a valid date.</li></ul>"#);
        assert_eq!(error_list(&["<x>".into()], None), r#"<ul class="errorlist nonfield"><li>&lt;x&gt;</li></ul>"#);
    }

    #[test]
    fn invalid_dates_help_and_hidden_errors_match_django_52() {
        let date=Field::new("date","Date",Widget::Date).required().value("").help("Choose a date.").errors(vec!["Enter a valid date.".into()]);
        assert_eq!(date.widget_html(),r#"<input type="date" name="date" value="" required aria-invalid="true" aria-describedby="id_date_helptext id_date_error" id="id_date">"#);
        let fields=[Field::new("revision","Revision",Widget::Hidden).errors(vec!["Enter a whole number.".into()]),Field::new("name","Name",Widget::Text)];
        assert!(as_p(&fields,&[]).starts_with(r#"<ul class="errorlist nonfield"><li>(Hidden field revision) Enter a whole number.</li></ul>"#));
    }

    #[test]
    fn integers_accept_python_digits_signs_underscores_and_oversized_page_numbers() {
        assert_eq!(python_integer("  +١_٢  "),Some(12));
        assert_eq!(python_integer("9".repeat(100).as_str()),Some(i64::MAX));
        assert_eq!(python_integer("-999999999999999999999999999"),Some(i64::MIN));
        for invalid in ["1__2","_1","1_","+","1.0"] {assert!(python_integer(invalid).is_none(),"{invalid}");}
        assert!(python_integer(&"1".repeat(4301)).is_none());
    }
}
