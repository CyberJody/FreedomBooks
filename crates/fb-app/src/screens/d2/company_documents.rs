//! Company-document additions to the shared company form. Onboarding owns chart/sequence controls.
use super::*;

pub(super) fn fields(
    app: &mut App,
    c: CompanyId,
    vm: &mut Value,
    posted: Option<Posted<'_>>,
) -> AppResult<Vec<Field>> {
    let o = app.open_mut()?;
    let details = serde_json::to_value(o.book.read(|tx| fb_core::company_documents::get(tx, c))?)
        .expect("details");
    vm["has_company_logo"] = json!(
        o.book
            .read(|tx| fb_core::company_documents::logo(tx, c))?
            .is_some()
    );
    vm["company_logo_url"] = json!(format!("/c/{c}/document-logo/"));
    let mut fields = Vec::new();
    for &(key, label, max) in fb_core::company_documents::FIELDS {
        let widget = if key == "payment_instructions" {
            Widget::Textarea { cols: 40, rows: 5 }
        } else {
            Widget::Text
        };
        fields.push(
            Field::new(key, label, widget)
                .attr("maxlength", max.to_string())
                .value_opt(data_value(
                    posted,
                    key,
                    details[key].as_str().map(str::to_owned),
                )),
        );
    }
    fields.push(
        Field::new("logo_upload", "", Widget::Hidden).value_opt(data_value(
            posted,
            "logo_upload",
            None,
        )),
    );
    fields.push(Field::new(
        "remove_logo",
        "Remove current logo",
        Widget::Checkbox {
            checked: posted.is_some_and(|p| checked(p, "remove_logo")),
        },
    ));
    Ok(fields)
}

pub(super) fn clean_post(
    fields: Posted<'_>,
    errors: &mut Errors,
) -> crate::commands::CompanyDocumentValues {
    let mut data = serde_json::Map::new();
    for &(key, _, max) in fb_core::company_documents::FIELDS {
        data.insert(
            key.into(),
            json!(clean_text(fields, key, max, false, true, errors)),
        );
    }
    let upload = get(fields, "logo_upload").filter(|s| !s.is_empty());
    let details = serde_json::from_value(Value::Object(data.clone())).expect("details");
    if let Err(e) = fb_core::company_documents::clean(details) {
        error(errors, "__all__", e.message());
    }
    if let Some(encoded) = upload
        && let Err(e) = fb_core::company_documents::clean_logo(encoded)
    {
        error(errors, "__all__", e.message());
    }
    data.insert("logo_upload".into(), json!(upload));
    data.insert("remove_logo".into(), json!(checked(fields, "remove_logo")));
    serde_json::from_value(Value::Object(data)).expect("form values")
}
