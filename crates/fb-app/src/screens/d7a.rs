//! D7a: frozen check and deposit boundaries. Review capabilities stay in the unlocked session.
use super::d2::{checked, clean_date, clean_text, draw, error, nonfield, put, token};
use super::d6::{
    attachments, banks, bind_fields, choice as raw_choice, contacts, ctx, evidence, formset_errors,
    integer, management, ordered, references, result_error, reversal_fields, reverse_values, s,
    scoped_rows, select, set_errors,
};
use super::*;
use crate::Command;
use crate::commands::{CheckLineValues, CheckValues, DepositValues};
use fb_core::{
    banking, checks,
    directory::{self, Model},
    documents, py,
};
type Errors = HashMap<String, Vec<String>>;
const CHOICE: &str = "Select a valid choice. That choice is not one of the available choices.";
mod check;
mod deposit;

// Django ModelChoiceField delegates to an integer model PK (which accepts surrounding
// whitespace). Plain ChoiceField and TypedMultipleChoiceField compare the original strings.
fn choice(p: Posted<'_>, f: &Field, e: &mut Errors, model: bool) -> String {
    if !model {
        return raw_choice(p, f, e, false);
    }
    let raw = get(p, &f.html_name()).unwrap_or("");
    let cleaned = py::py_strip(raw);
    if !raw.is_empty() && cleaned.is_empty() {
        error(e, &f.html_name(), CHOICE);
        return raw.into();
    }
    let mut fields = p.to_vec();
    fields.retain(|(k, _)| k != &f.html_name());
    fields.push((f.html_name(), cleaned.into()));
    raw_choice(&fields, f, e, true)
}
fn key(p: Posted<'_>, required: bool, e: &mut Errors) -> String {
    let raw = py::py_strip(get(p, "request_key").unwrap_or(""));
    if raw.is_empty() {
        if required {
            error(e, "request_key", "This field is required.");
        }
        return String::new();
    }
    match py::py_uuid(raw) {
        Some(k) => k.0,
        None => {
            error(e, "request_key", "Enter a valid UUID.");
            raw.into()
        }
    }
}

#[derive(Default)]
pub(crate) struct Reviews {
    checks: HashMap<String, checks::Review>,
    deposits: HashMap<String, banking::Review>,
}
fn keep<T>(map: &mut HashMap<String, T>, review: T) -> String {
    if map.len() >= 256 {
        map.clear();
    }
    let token = uuid::Uuid::new_v4().to_string();
    map.insert(token.clone(), review);
    token
}
pub(crate) fn is_d7a_command(c: &Command) -> bool {
    matches!(
        c,
        Command::SaveCheck { .. }
            | Command::ReviewCheck { .. }
            | Command::RecordCheck { .. }
            | Command::AssignCheckNumber { .. }
            | Command::VoidCheck { .. }
            | Command::ReviewDeposit { .. }
            | Command::RecordDeposit { .. }
            | Command::ReverseDeposit { .. }
    )
}
pub(crate) fn run_d7a_command(app: &mut App, cmd: Command) -> AppResult<Outcome> {
    let actor = app.open_mut()?.owner;
    let data = match cmd {
        Command::SaveCheck { company_id, values } => checks::save_check(
            &mut app.open_mut()?.book,
            actor,
            crate::session::company(&company_id)?,
            &serde_json::to_value(values).unwrap(),
        )?,
        Command::ReviewCheck {
            company_id,
            check_id,
            expected_revision,
        } => {
            let r = checks::review_check(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                &check_id,
                Some(expected_revision),
            )?;
            let mut d = r.display.clone();
            d["token"] = json!(keep(&mut app.d7a_reviews.checks, r));
            d
        }
        Command::RecordCheck {
            company_id,
            check_id,
            expected_revision,
            review_token,
        } => {
            let r = app
                .d7a_reviews
                .checks
                .remove(&review_token)
                .ok_or_else(|| FbError::validation("Invalid review. Open a new recording review."))?;
            checks::record_check(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                &check_id,
                Some(expected_revision),
                &r,
            )?
        }
        Command::AssignCheckNumber {
            company_id,
            check_id,
            expected_revision,
            check_number,
        } => checks::assign_number(
            &mut app.open_mut()?.book,
            actor,
            crate::session::company(&company_id)?,
            &check_id,
            Some(expected_revision),
            &json!(check_number),
        )?,
        Command::VoidCheck {
            company_id,
            check_id,
            expected_revision,
            date,
            reason,
        } => checks::void_check(
            &mut app.open_mut()?.book,
            actor,
            crate::session::company(&company_id)?,
            &check_id,
            Some(expected_revision),
            &json!(date),
            &json!(reason),
        )?,
        Command::ReviewDeposit { company_id, values } => {
            let r = banking::review_deposit(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                &serde_json::to_value(values).unwrap(),
            )?;
            let mut d = r.display.clone();
            d["token"] = json!(keep(&mut app.d7a_reviews.deposits, r));
            d
        }
        Command::RecordDeposit {
            company_id,
            values,
            review_token,
        } => {
            let r = app
                .d7a_reviews
                .deposits
                .remove(&review_token)
                .ok_or_else(|| FbError::validation("Invalid review. Open a fresh deposit review."))?;
            banking::record_deposit(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                &serde_json::to_value(values).unwrap(),
                &r,
            )?
        }
        Command::ReverseDeposit {
            company_id,
            deposit_id,
            expected_revision,
            date,
            reason,
        } => banking::reverse_deposit(
            &mut app.open_mut()?.book,
            actor,
            crate::session::company(&company_id)?,
            &deposit_id,
            Some(expected_revision),
            &json!(date),
            &json!(reason),
        )?,
        _ => unreachable!(),
    };
    Ok(Outcome::Done { data })
}
pub(super) fn is_route(name: &str) -> bool {
    name.starts_with("checks:") || name.starts_with("deposits:")
}
pub(super) fn render(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    if r.name.starts_with("checks:") {
        check::render(app, r, p, path)
    } else {
        deposit::render(app, r, p, path)
    }
}
pub(super) fn submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    if r.name.starts_with("checks:") {
        check::submit(app, r, path, p)
    } else {
        deposit::submit(app, r, path, p)
    }
}
fn json_columns(v: &mut Value) {
    for k in [
        "company_snapshot",
        "bank_snapshot",
        "payee_snapshot",
        "item_snapshot",
        "account_snapshot",
        "customer_snapshot",
    ] {
        if let Some(raw) = v[k].as_str() {
            v[k] = serde_json::from_str(raw).expect("stored snapshot");
        }
    }
}
fn query_string(p: Posted<'_>) -> String {
    urlencode(
        &p.iter()
            .filter(|(k, _)| k != "page")
            .cloned()
            .collect::<Vec<_>>(),
    )
}
