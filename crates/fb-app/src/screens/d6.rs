//! D6: frozen payables and transfer HTTP boundaries; all writes use typed commands.
use super::d2::{
    checked, clean_date, clean_integer, clean_text, data_value, decorate, draw, error, nonfield,
    put, token,
};
use super::*;
use crate::{
    Command,
    commands::{BillPaymentValues, BillValues, TransferValues},
};
use chrono::Datelike;
use fb_core::{
    directory::{self, Model},
    documents, payables, py, transfers,
};
use fb_db::{Query, rusqlite::types::ValueRef};
type Errors = HashMap<String, Vec<String>>;
const REQUIRED: &str = "This field is required.";
const CHOICE: &str = "Select a valid choice. That choice is not one of the available choices.";
mod bills;
mod payments;
mod transfer;

#[derive(Default)]
pub(crate) struct Reviews {
    bills: HashMap<String, payables::Review>,
    payments: HashMap<String, payables::Review>,
    transfers: HashMap<String, transfers::Review>,
}
fn keep<T>(map: &mut HashMap<String, T>, review: T) -> String {
    if map.len() >= 256 {
        map.clear();
    }
    let key = uuid::Uuid::new_v4().to_string();
    map.insert(key.clone(), review);
    key
}
fn expired() -> AppError {
    FbError::validation("Invalid review. Open a new posting review.").into()
}
pub(crate) fn is_d6_command(c: &Command) -> bool {
    matches!(
        c,
        Command::SaveBill { .. }
            | Command::ReviewBill { .. }
            | Command::PostBill { .. }
            | Command::VoidBill { .. }
            | Command::ReviewBillPayment { .. }
            | Command::RecordBillPayment { .. }
            | Command::ReverseBillPayment { .. }
            | Command::ReviewTransfer { .. }
            | Command::RecordTransfer { .. }
            | Command::VoidTransfer { .. }
    )
}
pub(crate) fn run_d6_command(app: &mut App, cmd: Command) -> AppResult<Outcome> {
    let actor = app.open_mut()?.owner;
    let data = match cmd {
        Command::SaveBill { company_id, values } => payables::save_bill(
            &mut app.open_mut()?.book,
            actor,
            crate::session::company(&company_id)?,
            &serde_json::to_value(values).unwrap(),
        )?,
        Command::ReviewBill {
            company_id,
            bill_id,
            expected_revision,
        } => {
            let r = payables::review_bill(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                &bill_id,
                Some(expected_revision),
            )?;
            let mut d = r.display.clone();
            d["token"] = json!(keep(&mut app.d6_reviews.bills, r));
            d
        }
        Command::PostBill {
            company_id,
            bill_id,
            expected_revision,
            review_token,
        } => {
            let r = app
                .d6_reviews
                .bills
                .remove(&review_token)
                .ok_or_else(expired)?;
            payables::post_bill(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                &bill_id,
                Some(expected_revision),
                &r,
            )?
        }
        Command::VoidBill {
            company_id,
            bill_id,
            expected_revision,
            date,
            reason,
        } => payables::void_bill(
            &mut app.open_mut()?.book,
            actor,
            crate::session::company(&company_id)?,
            &bill_id,
            Some(expected_revision),
            &json!(date),
            &json!(reason),
        )?,
        Command::ReviewBillPayment { company_id, values } => {
            let p = serde_json::to_value(values).unwrap();
            let r = payables::review_payment(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                &p,
            )?;
            let mut d = r.display.clone();
            d["data"] = p;
            d["data"]["amount"] = json!(fb_core::services::money_json(&d["data"]["amount"])?.to_string());
            for a in d["allocations"].as_array_mut().unwrap() {
                a["amount"] = json!(cash(a["amount"].as_i64().unwrap() as i128));
            }
            d["token"] = json!(keep(&mut app.d6_reviews.payments, r));
            d
        }
        Command::RecordBillPayment {
            company_id,
            values,
            review_token,
        } => {
            let r = app
                .d6_reviews
                .payments
                .remove(&review_token)
                .ok_or_else(|| {
                    AppError::from(FbError::validation(
                        "Invalid review. Open a fresh payment review.",
                    ))
                })?;
            payables::record_payment(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                &serde_json::to_value(values).unwrap(),
                &r,
            )?
        }
        Command::ReverseBillPayment {
            company_id,
            payment_id,
            expected_revision,
            date,
            reason,
        } => payables::reverse_payment(
            &mut app.open_mut()?.book,
            actor,
            crate::session::company(&company_id)?,
            &payment_id,
            Some(expected_revision),
            &json!(date),
            &json!(reason),
        )?,
        Command::ReviewTransfer { company_id, values } => {
            let p = serde_json::to_value(values).unwrap();
            let r = transfers::review_transfer(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                &p,
            )?;
            let mut d = r.display.clone();
            d["data"] = p;
            d["data"]["amount"] = json!(fb_core::services::money_json(&d["data"]["amount"])?.to_string());
            for a in d["entries"].as_array_mut().unwrap() {
                for k in ["debit", "credit"] {
                    if a[k] == "0.00" {
                        a[k] = Value::Null;
                    }
                }
            }
            d["token"] = json!(keep(&mut app.d6_reviews.transfers, r));
            d
        }
        Command::RecordTransfer {
            company_id,
            values,
            review_token,
        } => {
            let r = app
                .d6_reviews
                .transfers
                .remove(&review_token)
                .ok_or_else(|| {
                    AppError::from(FbError::validation(
                        "Invalid review. Open a fresh transfer review.",
                    ))
                })?;
            transfers::record_transfer(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                &serde_json::to_value(values).unwrap(),
                &r,
            )?
        }
        Command::VoidTransfer {
            company_id,
            transfer_id,
            expected_revision,
            date,
            reason,
        } => transfers::void_transfer(
            &mut app.open_mut()?.book,
            actor,
            crate::session::company(&company_id)?,
            &transfer_id,
            Some(expected_revision),
            &json!(date),
            &json!(reason),
        )?,
        _ => unreachable!(),
    };
    Ok(Outcome::Done { data })
}
pub(super) fn is_route(name: &str) -> bool {
    name.starts_with("bills:") || name.starts_with("payments:") || name.starts_with("transfers:")
}
pub(super) fn render(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    if r.name.starts_with("bills:") {
        bills::render(app, r, p, path)
    } else if r.name.starts_with("payments:") {
        payments::render(app, r, p, path)
    } else {
        transfer::render(app, r, p, path)
    }
}
pub(super) fn submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    if r.name.starts_with("bills:") {
        bills::submit(app, r, path, p)
    } else if r.name.starts_with("payments:") {
        payments::submit(app, r, path, p)
    } else {
        transfer::submit(app, r, path, p)
    }
}
pub(super) fn ctx(app: &mut App, r: &Route) -> AppResult<Value> {
    let c = arg_company(r)?;
    Ok(serde_json::to_value(shell(
        app,
        Some(c),
        &Current {
            route: &r.name,
            kind: None,
        },
    )?)
    .unwrap())
}
pub(super) fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
fn rows(
    tx: &(impl Query + ?Sized),
    sql: &str,
    args: &[&dyn fb_db::rusqlite::ToSql],
) -> Result<Vec<Value>, FbError> {
    let mut st = tx.prepare(sql)?;
    let names = st
        .column_names()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    Ok(st
        .query_map(args, |r| {
            let mut v = json!({});
            for (n, k) in names.iter().enumerate() {
                v[k] = match r.get_ref(n)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(n) => json!(n),
                    ValueRef::Real(n) => json!(n),
                    ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                    ValueRef::Blob(_) => Value::Null,
                };
            }
            Ok(v)
        })?
        .collect::<Result<_, _>>()?)
}
pub(super) fn scoped_rows(
    app: &mut App,
    c: CompanyId,
    sql: &str,
    args: &[&dyn fb_db::rusqlite::ToSql],
) -> AppResult<Vec<Value>> {
    let o = app.open_mut()?;
    let actor = o.owner;
    Ok(o.book.read(|tx| {
        fb_core::services::access(tx, actor, c)?;
        rows(tx, sql, args)
    })?)
}
pub(super) fn contacts(
    app: &mut App,
    c: CompanyId,
    active: bool,
    retained: Option<&str>,
    archived: &str,
) -> AppResult<Vec<(String, String)>> {
    Ok(app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let ord=super::d2::ordinals(tx,c,"directory_vendor")?;
        let duplicates=fb_core::party_display::duplicate_active_display_names(tx,c,Model::Vendor)?;
        let offered=directory::list(tx,c,Model::Vendor)?.into_iter().filter(|r| !active || r["active"] == true || Some(s(r, "id")) == retained).collect::<Vec<_>>();
        Ok(offered.iter().map(|r| {
                let id = s(r, "id").to_owned();
                let peers=offered.iter().filter(|p|p["display_name"]==r["display_name"]).cloned().collect::<Vec<_>>();
                let mut label=fb_core::party_display::contact_list_label(r,&duplicates,Some(&peers),"");
                if let Some(base)=label.strip_suffix(&format!("(#{id})")){label=format!("{base}(#{})",ord[&id]);}
                if r["active"] != true {
                    label += if archived == "confirm" {
                        " (archived — confirm retention or replace)"
                    } else {
                        " (archived)"
                    };
                }
                (id, label)
            })
            .collect())
    })?)
}
fn short_labels(tx: &(impl Query + ?Sized), c: CompanyId) -> Result<Value, FbError> {
    let ord = super::d2::ordinals(tx, c, "directory_vendor")?;
    let mut labels = fb_core::party_display::contact_label_map(tx, c, Model::Vendor, None)?;
    for (id, label) in labels.as_object_mut().unwrap() {
        let raw = label.as_str().unwrap();
        let mark = format!("(#{id})");
        if let Some(base) = raw.strip_suffix(&mark) {
            *label = json!(format!("{base}(#{})", ord[id]));
        }
    }
    Ok(labels)
}
pub(super) fn labels(app: &mut App, c: CompanyId) -> AppResult<Value> {
    Ok(app.open_mut()?.book.read(|tx| short_labels(tx, c))?)
}
pub(super) fn banks(app: &mut App, c: CompanyId, extra: &[&str]) -> AppResult<Vec<Account>> {
    Ok(app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let setup = documents::setup_snapshot(tx, c)?;
        let mut a = fb_core::store::accounts(tx, c)?;
        a.retain(|a| {
            setup["banks"]
                .as_array()
                .is_some_and(|b| b.contains(&json!(a.id.to_db())))
                || extra.contains(&a.id.to_db().as_str())
        });
        a.sort_by(|a, b| a.code.cmp(&b.code).then(a.id.to_db().cmp(&b.id.to_db())));
        Ok(a)
    })?)
}
pub(super) fn select(
    name: &str,
    label: &str,
    placeholder: &str,
    opts: Vec<(String, String)>,
    required: bool,
) -> Field {
    let mut choices = vec![(String::new(), placeholder.into())];
    choices.extend(opts);
    let f = Field::new(name, label, Widget::Select { choices });
    if required { f.required() } else { f }
}
pub(super) fn choice(p: Posted<'_>, f: &Field, errors: &mut Errors, model: bool) -> String {
    let raw = get(p, &f.html_name()).unwrap_or("");
    let v = raw;
    let valid = match &f.widget {
        Widget::Select { choices } => choices.iter().any(|(id, _)| id == v),
        Widget::GroupedSelect { groups } => groups
            .iter()
            .any(|(_, cs)| cs.iter().any(|(id, _)| id == v)),
        _ => false,
    };
    if v.is_empty() {
        if f.required {
            error(errors, &f.html_name(), REQUIRED);
        }
    } else if !valid {
        error(
            errors,
            &f.html_name(),
            if model {
                CHOICE.into()
            } else {
                format!("Select a valid choice. {v} is not one of the available choices.")
            },
        );
    }
    v.into()
}
pub(super) fn integer(
    p: Posted<'_>,
    name: &str,
    min: i64,
    max: Option<i64>,
    required: bool,
    e: &mut Errors,
) -> Option<i64> {
    let raw = get(p, name).unwrap_or("");
    if raw.is_empty() && !required {
        return None;
    }
    match clean_integer(raw) {
        Some(n) if n < min => {
            error(
                e,
                name,
                format!("Ensure this value is greater than or equal to {min}."),
            );
            None
        }
        Some(n) if max.is_some_and(|m| n > m) => {
            error(
                e,
                name,
                format!(
                    "Ensure this value is less than or equal to {}.",
                    max.unwrap()
                ),
            );
            None
        }
        Some(n) => Some(n),
        None => {
            error(
                e,
                name,
                if raw.is_empty() {
                    REQUIRED
                } else {
                    "Enter a whole number."
                },
            );
            None
        }
    }
}
/// Django `forms.UUIDField`: strip (CharField), then `uuid.UUID`.
pub(super) fn key(p: Posted<'_>, required: bool, e: &mut Errors) -> String {
    let raw = get(p, "request_key").unwrap_or("");
    if py::py_strip(raw).is_empty() {
        if required {
            error(e, "request_key", REQUIRED);
        }
        return String::new();
    }
    match py::py_uuid(py::py_strip(raw)) {
        Some(id) => id.0,
        None => {
            error(e, "request_key", "Enter a valid UUID.");
            raw.into()
        }
    }
}
pub(super) fn bind_fields(
    mut fs: Vec<Field>,
    p: Option<Posted<'_>>,
    initial: &Value,
    e: &Errors,
    required: bool,
) -> forms::BoundForm {
    for f in &mut fs {
        let name = f.html_name();
        let v = initial[&f.name]
            .as_str()
            .map(str::to_owned)
            .or_else(|| initial[&f.name].as_i64().map(|n| n.to_string()))
            .or_else(|| f.value.clone());
        f.value = if matches!(f.widget,Widget::Date) && p.is_some() {p.and_then(|p|get(p,&name)).map(str::to_owned)} else {data_value(p, &name, v)};
        if matches!(f.widget, Widget::Checkbox { .. }) {
            f.widget = Widget::Checkbox {
                checked: p
                    .map(|p| checked(p, &name))
                    .unwrap_or(initial[&f.name] == true),
            };
        }
        if !required {
            f.required = false;
        }
    }
    let top=if fs.first().is_none_or(|f|f.prefix.is_empty()){nonfield(e,"__all__")}else{vec![]};
    forms::bind(&decorate(fs, e), &top)
}
pub(super) fn ordered(rows: &mut [Value], date: &str, id: &str) {
    rows.sort_by(|a, b| s(b, date).cmp(s(a, date)).then(s(b, id).cmp(s(a, id))));
}
pub(super) fn formset_errors(errors:&[String])->String {
    forms::error_list(errors,None).replacen("class=\"errorlist nonfield\"","class=\"errorlist nonform\"",1)
}
fn formset_count(p: Option<Posted<'_>>, prefix: &str, initial: usize) -> usize {
    p.map(|p| {
        get(p, &format!("{prefix}-TOTAL_FORMS"))
            .and_then(clean_integer)
            .unwrap_or(0)
            .clamp(0, 50) as usize
    })
    .unwrap_or(initial.max(1) + 2)
}
pub(super) fn management(p: Option<Posted<'_>>, prefix: &str, total: usize, initial: usize) -> String {
    let fields = [("TOTAL_FORMS",total.to_string()),("INITIAL_FORMS",initial.to_string()),("MIN_NUM_FORMS","1".into()),("MAX_NUM_FORMS","50".into())]
        .into_iter().map(|(name,default)| (name,p.map(|p|get(p,&format!("{prefix}-{name}")).unwrap_or("").to_owned()).unwrap_or(default))).collect::<Vec<_>>();
    super::d2::management_html(prefix,&fields,p.is_some())
}
pub(super) fn set_errors(p: Posted<'_>, prefix: &str, meaningful: usize, _any_errors: bool) -> Vec<String> {
    let total = get(p, &format!("{prefix}-TOTAL_FORMS")).and_then(clean_integer);
    let initial = get(p, &format!("{prefix}-INITIAL_FORMS")).and_then(clean_integer);
    if total.is_none() || initial.is_none() {
        let missing = [("TOTAL_FORMS", total), ("INITIAL_FORMS", initial)]
            .into_iter()
            .filter(|(_, n)| n.is_none())
            .map(|(k, _)| format!("{prefix}-{k}"))
            .collect::<Vec<_>>()
            .join(", ");
        return vec![format!(
            "ManagementForm data is missing or has been tampered with. Missing fields: {missing}. You may need to file a bug report if the issue persists."
        )];
    }
    if total.unwrap() > 50 {
        vec!["Please submit at most 50 forms.".into()]
    } else if meaningful == 0 {
        vec!["Please submit at least 1 form.".into()]
    } else {
        vec![]
    }
}
pub(super) fn reversal_fields() -> Vec<Field> {
    vec![
        Field::new("expected_revision", "Expected revision", Widget::Hidden).required(),
        Field::new("date", "Reversal date", Widget::Date).required(),
        Field::new("reason", "Reason", Widget::Textarea { cols: 40, rows: 2 }).required(),
    ]
}
pub(super) fn reverse_values(p: Posted<'_>, e: &mut Errors) -> (i64, String, String) {
    let rev = integer(p, "expected_revision", 1, None, true, e).unwrap_or(0);
    let date = clean_date(p, "date", true, e).unwrap_or_default();
    let reason = clean_text(p, "reason", 500, true, true, e);
    (rev, date, reason)
}
pub(super) fn result_error(result: AppResult<Outcome>, e: &mut Errors) -> AppResult<Option<Value>> {
    match result {
        Ok(Outcome::Done { data }) => Ok(Some(data)),
        Err(AppError::Core(FbError::Validation(messages))) => {
            error(e, "__all__", messages.join("; "));
            Ok(None)
        }
        Err(e) => Err(e),
        _ => unreachable!(),
    }
}
fn json_columns(v: &mut Value) {
    for k in [
        "vendor_snapshot",
        "company_snapshot",
        "from_snapshot",
        "to_snapshot",
        "item_snapshot",
    ] {
        if let Some(raw) = v[k].as_str() {
            v[k] = serde_json::from_str(raw).expect("stored snapshot");
        }
    }
}
pub(super) fn evidence(app: &mut App, c: CompanyId, vm: &mut Value) -> AppResult<()> {
    let all = scoped_rows(
        app,
        c,
        "SELECT id,number FROM ledger_journal WHERE company_id=? ORDER BY id",
        &[&c.to_db()],
    )?;
    let mut labels = json!({});
    for j in all {
        labels[s(&j, "id")] = json!(format!("Journal J{:06}", j["number"].as_i64().unwrap()));
    }
    put(vm, "evidence", labels);
    Ok(())
}
pub(super) fn references(app: &mut App, c: CompanyId, identity: &str) -> AppResult<Vec<Value>> {
    scoped_rows(
        app,
        c,
        "SELECT * FROM documents_documentaccount WHERE source_id=? ORDER BY role COLLATE fb_text",
        &[&identity],
    )
}
pub(super) fn attachments(app: &mut App, c: CompanyId, identity: &str, vm: &mut Value) -> AppResult<()> {
    // D9 owns attachments: the real panel (files, removed rows, notes, per-file remove tokens).
    super::d9::attachments::panel(app, c, identity, vm)
}
