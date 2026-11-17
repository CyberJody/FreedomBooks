use super::*;

pub(super) fn fields(app: &mut App, c: CompanyId, vm: &mut Value, posted: Option<Posted<'_>>) -> AppResult<()> {
    crate::invoice_stripe::settings(app,c,vm)?;
    if app.invoice_delivery {
        let connection=app.open_mut()?.book.read(|q|fb_core::invoice_email::connection(q,c))?;
        vm["gmail_enabled"]=json!(true);
        vm["gmail_connection"]=serde_json::to_value(connection).expect("connection");
        vm["gmail_revision"]=json!(vm["gmail_connection"]["revision"].as_i64().unwrap_or(0));
        vm["gmail_keyring"]=json!(app.os_store_usable && app.vault_key.is_some());
    }
    let stored = app.open_mut()?.book.read(|q| fb_core::invoice_preferences::get(q, c))?;
    let initial = serde_json::to_value(stored.settings).expect("preferences");
    vm["invoice_setup"]=json!({"zelle":initial["zelle_enabled"]==true && initial["zelle_recipient"].as_str().is_some_and(|v|!v.is_empty()),"stripe_mode":initial["stripe_mode"],"email_mode":initial["email_mode"]});
    let posted = posted.filter(|p| get(p, "invoice_preferences_present") == Some("1"));
    let mut fields = vec![
        Field::new("invoice_preferences_present", "", Widget::Hidden).value("1"),
        Field::new("invoice_preferences_revision", "", Widget::Hidden)
            .value_opt(data_value(posted, "invoice_preferences_revision", Some(stored.revision.to_string()))),
    ];
    for &(key, label, max, multiline) in fb_core::invoice_preferences::TEXT_FIELDS {
        let name = format!("invoice_{key}");
        fields.push(Field::new(&name, label, if multiline { Widget::Textarea { cols:40, rows:4 } } else { Widget::Text })
            .attr("maxlength", max.to_string())
            .value_opt(data_value(posted, &name, initial[key].as_str().map(str::to_owned))));
    }
    for (key, label) in [("zelle_enabled", "Show Zelle payment instructions"), ("stripe_enabled", "Offer Stripe payment links when connected")] {
        let name = format!("invoice_{key}");
        fields.push(Field::new(&name, label, Widget::Checkbox { checked: posted.map_or(initial[key] == true, |p| checked(p, &name)) }));
    }
    for (key, label, choices) in [
        ("stripe_mode", "Stripe environment", vec![("TEST".into(), "Test / sandbox".into()), ("LIVE".into(), "Live".into())]),
        ("email_mode", "Default email action", vec![("DRAFT".into(), "Save to Gmail Drafts".into()), ("SEND".into(), "Send email".into())]),
    ] {
        let name = format!("invoice_{key}");
        fields.push(Field::new(&name, label, Widget::Select { choices }).required()
            .value_opt(data_value(posted, &name, initial[key].as_str().map(str::to_owned))));
    }
    for (name, keys) in [
        ("invoice_preferences_form", vec!["invoice_preferences_present", "invoice_preferences_revision", "invoice_footer"]),
        ("invoice_zelle_form", vec!["invoice_zelle_enabled", "invoice_zelle_name", "invoice_zelle_recipient", "invoice_zelle_instructions"]),
        ("invoice_stripe_form", vec!["invoice_stripe_enabled", "invoice_stripe_mode"]),
        ("invoice_email_form", vec!["invoice_email_mode", "invoice_email_subject", "invoice_email_greeting", "invoice_email_signature", "invoice_reply_to"]),
    ] {
        let section:Vec<Field>=keys.iter().map(|key|fields.iter().find(|f|f.name==*key).expect("preference field").clone()).collect();
        put(vm,name,forms::bind(&section,&[]));
    }
    Ok(())
}

pub(super) fn clean_post(p: Posted<'_>, errors: &mut Errors) -> Option<Box<crate::commands::InvoicePreferenceValues>> {
    if get(p, "invoice_preferences_present") != Some("1") { return None; }
    let mut value = serde_json::Map::new();
    for &(key, _, max, _) in fb_core::invoice_preferences::TEXT_FIELDS {
        value.insert(key.into(), json!(clean_text(p,&format!("invoice_{key}"),max,false,true,errors)));
    }
    for key in ["zelle_enabled", "stripe_enabled"] {
        value.insert(key.into(), json!(checked(p,&format!("invoice_{key}"))));
    }
    for key in ["stripe_mode", "email_mode"] {
        value.insert(key.into(),json!(get(p,&format!("invoice_{key}")).unwrap_or("")));
    }
    let settings = serde_json::from_value(Value::Object(value.clone())).expect("preferences");
    if let Err(error_message) = fb_core::invoice_preferences::clean(settings) {
        error(errors, "__all__", error_message.message());
    }
    let revision = get(p,"invoice_preferences_revision").and_then(clean_integer).filter(|n| *n>=0);
    if revision.is_none() { error(errors,"__all__","Reload the company settings before saving."); }
    value.insert("expected_revision".into(),json!(revision.unwrap_or(-1)));
    Some(serde_json::from_value(Value::Object(value)).expect("preference values"))
}
