//! W0-DOCS: the approved empty business-details extension on the existing company-edit screen.
use fb_templates::forms::{Field, Widget, bind};

pub fn django(text: String) -> String {
    let mut text = text;
    if text.contains("<h1>Audit history</h1>") {
        for selected in ["", " selected"] {
            let old =
                format!("<option value=\"company.created\"{selected}>Company created</option>");
            let new = format!(
                "{old}<option value=\"company.document_details_updated\">Company document details changed</option>"
            );
            text = text.replacen(&old, &new, 1);
        }
    }
    if !text.contains("<h1>Edit company</h1>")
        || text
            .matches("<button type=\"submit\">Save</button>")
            .count()
            != 1
    {
        return text;
    }
    let specs = [
        ("street1", "Street address", 200),
        ("street2", "Address line 2", 200),
        ("city", "City", 100),
        ("region", "State / region", 100),
        ("postal_code", "Postal code", 32),
        ("country", "Country", 100),
        ("phone", "Phone", 50),
        ("email", "Email", 254),
        ("website", "Website", 500),
        ("tax_id", "Tax ID (optional)", 100),
        ("payment_instructions", "Payment instructions", 2000),
    ];
    let mut fields = Vec::new();
    for (key, label, max) in specs {
        let widget = if key == "payment_instructions" {
            Widget::Textarea { cols: 40, rows: 5 }
        } else {
            Widget::Text
        };
        fields.push(Field::new(key, label, widget).attr("maxlength", max.to_string()));
    }
    fields.push(Field::new("logo_upload", "", Widget::Hidden));
    fields.push(Field::new(
        "remove_logo",
        "Remove current logo",
        Widget::Checkbox { checked: false },
    ));
    let fragment = bind(&fields, &[]).as_p
        + r#"<p><label for="company-logo-file">Logo</label><input id="company-logo-file" type="file" accept="image/png,image/jpeg"><span>PNG or JPEG, up to 128 KB and 1200 × 1200 pixels. Stored in these books.</span></p>"#;
    let fragment = super::normalize(
        &fragment,
        &super::IdMap::new(&std::collections::BTreeMap::new()),
    );
    text.replacen(
        "<button type=\"submit\">Save</button>",
        &format!("{fragment}<button type=\"submit\">Save</button>"),
        1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn company_document_adapter_never_rewrites_the_desktop_or_user_text() {
        let old = "<h1>Edit company</h1><form><input name=\"name\" value=\"Street address\"><button type=\"submit\">Save</button></form>";
        let new = django(old.into());
        assert!(new.contains("name=\"street1\""));
        assert!(new.contains("value=\"Street address\""));
        let reference = super::super::normalize(
            old,
            &super::super::IdMap::new(&std::collections::BTreeMap::new()).reference(),
        );
        let desktop = super::super::normalize(
            &new,
            &super::super::IdMap::new(&std::collections::BTreeMap::new()),
        );
        assert_eq!(reference, desktop);
        assert_ne!(
            reference,
            super::super::normalize(
                &new.replace("maxlength=\"200\"", "maxlength=\"199\""),
                &super::super::IdMap::new(&std::collections::BTreeMap::new())
            )
        );
        assert_eq!(
            django("<h1>Other form</h1>Street address".into()),
            "<h1>Other form</h1>Street address"
        );
    }
}
