//! D5: frozen credit memo and customer deposit views and typed form commands.
use super::d2::{clean_date, clean_text, draw, error, put, token};
use super::d6::{
    attachments, banks, bind_fields, choice as raw_choice, ctx, evidence, key as raw_key,
    references, result_error, reversal_fields, reverse_values, s, scoped_rows, select,
};
use super::*;
use crate::{Command, commands::*};
use fb_core::{credits, prepayments, py};
type Errors = HashMap<String, Vec<String>>;
// UUIDField inherits CharField's strip=True, independently of the view's raw retry lookup.
fn key(p: Posted<'_>, required: bool, e: &mut Errors) -> String {
    let mut cleaned = p.to_vec();
    cleaned.push((
        "request_key".into(),
        py::py_strip(get(p, "request_key").unwrap_or("")).into(),
    ));
    raw_key(&cleaned, required, e)
}

// ModelChoiceField resolves a padded integer PK on the reference backend; ChoiceField
// compares the original text. Only a known model choice is canonicalized here, and
// the bound widget continues to display the original posted value.
fn choice(p: Posted<'_>, f: &Field, e: &mut Errors, model: bool) -> String {
    if model && let Widget::Select { choices } = &f.widget {
        let raw = get(p, &f.html_name()).unwrap_or("");
        let trimmed = py::py_strip(raw);
        if !trimmed.is_empty() && choices.iter().any(|(id, _)| id == trimmed) {
            let mut cleaned = p.to_vec();
            cleaned.push((f.html_name(), trimmed.into()));
            return raw_choice(&cleaned, f, e, true);
        }
    }
    raw_choice(p, f, e, model)
}

mod forms;
pub(super) mod records;
#[derive(Default)]
pub(crate) struct Reviews {
    credit: HashMap<String, credits::Review>,
    deposit: HashMap<String, prepayments::Review>,
}
fn remember<T>(map: &mut HashMap<String, T>, cap: T) -> String {
    if map.len() >= 256 {
        map.clear();
    }
    let token = uuid::Uuid::new_v4().to_string();
    map.insert(token.clone(), cap);
    token
}
pub(super) fn is_route(name: &str) -> bool {
    name.starts_with("credits:")
        || (name.starts_with("prepayments:") && name != "prepayments:print")
}
pub(crate) fn is_d5_command(c: &Command) -> bool {
    matches!(
        c,
        Command::ReviewCreditMemo { .. }
            | Command::RecordCreditMemo { .. }
            | Command::ReviewCustomerDeposit { .. }
            | Command::RecordCustomerDeposit { .. }
            | Command::ReviewCreditApplication { .. }
            | Command::RecordCreditApplication { .. }
            | Command::ReviewDepositApplication { .. }
            | Command::RecordDepositApplication { .. }
            | Command::ReviewCustomerRefund { .. }
            | Command::RecordCustomerRefund { .. }
            | Command::VoidCreditMemo { .. }
            | Command::VoidCustomerDeposit { .. }
            | Command::UnapplyCredit { .. }
            | Command::UnapplyDeposit { .. }
            | Command::VoidCustomerRefund { .. }
    )
}
pub(crate) fn run_d5_command(app: &mut App, cmd: Command) -> AppResult<Outcome> {
    let actor = app.open_mut()?.owner;
    let data = match cmd {
        Command::ReviewCreditMemo { company_id, values } => {
            let r = credits::review_credit_memo(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                &memo_input(values),
            )?;
            let mut d = r.display.clone();
            d["token"] = json!(remember(&mut app.d5_reviews.credit, r));
            d
        }
        Command::RecordCreditMemo {
            company_id,
            values,
            review_token,
        } => {
            let cap = app.d5_reviews.credit.remove(&review_token);
            credits::record_credit_memo(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                &memo_input(values),
                cap.as_ref(),
            )?
        }
        Command::ReviewCustomerDeposit { company_id, values } => {
            let r = prepayments::review_deposit(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                &json!(values),
            )?;
            let mut d = r.display.clone();
            d["token"] = json!(remember(&mut app.d5_reviews.deposit, r));
            d
        }
        Command::RecordCustomerDeposit {
            company_id,
            values,
            review_token,
        } => {
            let cap = app.d5_reviews.deposit.remove(&review_token);
            prepayments::record_deposit(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                cap.as_ref(),
                &json!(values),
            )?
        }
        Command::ReviewCreditApplication {
            company_id,
            source_id,
            values,
        } => {
            let r = credits::review_application(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                &source_id,
                &json!(values),
            )?;
            let mut d = r.display.clone();
            d["token"] = json!(remember(&mut app.d5_reviews.credit, r));
            d
        }
        Command::RecordCreditApplication {
            company_id,
            source_id,
            values,
            review_token,
        } => {
            let cap = app.d5_reviews.credit.remove(&review_token);
            credits::apply_credit(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                &source_id,
                &json!(values),
                cap.as_ref(),
            )?
        }
        Command::ReviewDepositApplication {
            company_id,
            source_id,
            values,
        } => {
            let r = prepayments::review_application(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                &source_id,
                &json!(values),
            )?;
            let mut d = r.display.clone();
            d["token"] = json!(remember(&mut app.d5_reviews.deposit, r));
            d
        }
        Command::RecordDepositApplication {
            company_id,
            source_id,
            values,
            review_token,
        } => {
            let cap = app.d5_reviews.deposit.remove(&review_token);
            prepayments::apply_deposit(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                &source_id,
                cap.as_ref(),
                &json!(values),
            )?
        }
        Command::ReviewCustomerRefund {
            company_id,
            source_id,
            deposit,
            values,
        } => {
            let c = crate::session::company(&company_id)?;
            if deposit {
                let r = prepayments::review_refund(
                    &mut app.open_mut()?.book,
                    actor,
                    c,
                    &source_id,
                    &json!(values),
                )?;
                let mut d = r.display.clone();
                d["token"] = json!(remember(&mut app.d5_reviews.deposit, r));
                d
            } else {
                let r = credits::review_refund(
                    &mut app.open_mut()?.book,
                    actor,
                    c,
                    &source_id,
                    &json!(values),
                )?;
                let mut d = r.display.clone();
                d["token"] = json!(remember(&mut app.d5_reviews.credit, r));
                d
            }
        }
        Command::RecordCustomerRefund {
            company_id,
            source_id,
            deposit,
            values,
            review_token,
        } => {
            let c = crate::session::company(&company_id)?;
            if deposit {
                let cap = app.d5_reviews.deposit.remove(&review_token);
                prepayments::record_refund(
                    &mut app.open_mut()?.book,
                    actor,
                    c,
                    &source_id,
                    cap.as_ref(),
                    &json!(values),
                )?
            } else {
                let cap = app.d5_reviews.credit.remove(&review_token);
                credits::record_refund(
                    &mut app.open_mut()?.book,
                    actor,
                    c,
                    &source_id,
                    &json!(values),
                    cap.as_ref(),
                )?
            }
        }
        Command::VoidCreditMemo {
            company_id,
            record_id,
            expected_revision,
            date,
            reason,
        } => credits::void_credit_memo(
            &mut app.open_mut()?.book,
            actor,
            crate::session::company(&company_id)?,
            &record_id,
            Some(expected_revision),
            &json!(date),
            &json!(reason),
        )?,
        Command::VoidCustomerDeposit {
            company_id,
            record_id,
            expected_revision,
            date,
            reason,
        } => prepayments::void_deposit(
            &mut app.open_mut()?.book,
            actor,
            crate::session::company(&company_id)?,
            &record_id,
            Some(expected_revision),
            &json!(date),
            &json!(reason),
        )?,
        Command::UnapplyCredit {
            company_id,
            record_id,
            expected_revision,
            date,
            reason,
        } => credits::unapply_credit(
            &mut app.open_mut()?.book,
            actor,
            crate::session::company(&company_id)?,
            &record_id,
            Some(expected_revision),
            &json!(date),
            &json!(reason),
        )?,
        Command::UnapplyDeposit {
            company_id,
            record_id,
            expected_revision,
            date,
            reason,
        } => prepayments::unapply_deposit(
            &mut app.open_mut()?.book,
            actor,
            crate::session::company(&company_id)?,
            &record_id,
            Some(expected_revision),
            &json!(date),
            &json!(reason),
        )?,
        Command::VoidCustomerRefund {
            company_id,
            record_id,
            deposit,
            expected_revision,
            date,
            reason,
        } => {
            let f = if deposit {
                prepayments::void_refund
            } else {
                credits::void_refund
            };
            f(
                &mut app.open_mut()?.book,
                actor,
                crate::session::company(&company_id)?,
                &record_id,
                Some(expected_revision),
                &json!(date),
                &json!(reason),
            )?
        }
        _ => unreachable!(),
    };
    Ok(Outcome::Done { data })
}
fn memo_input(v: CreditMemoValues) -> Value {
    let mut d = json!(v);
    for line in d["lines"].as_array_mut().unwrap() {
        let object = line.as_object_mut().unwrap();
        for key in ["origin_position", "refresh_item", "keep_archived"] {
            object.remove(key);
        }
    }
    d
}
fn deposit(r: &Route) -> bool {
    r.name.starts_with("prepayments:")
}
fn ns(r: &Route) -> &'static str {
    if deposit(r) { "prepayments" } else { "credits" }
}
fn action(r: &Route) -> &str {
    r.name.split(':').nth(1).unwrap_or("")
}
fn source_arg(r: &Route) -> &str {
    if deposit(r) {
        "customer_deposit_id"
    } else {
        "credit_memo_id"
    }
}
fn refund_arg(r: &Route) -> &str {
    if deposit(r) {
        "deposit_refund_id"
    } else {
        "refund_id"
    }
}
fn application_arg(r: &Route) -> &str {
    if deposit(r) {
        "deposit_application_id"
    } else {
        "application_id"
    }
}
pub(super) fn render(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    match action(r) {
        "list" => records::listing(app, r, p),
        "detail" => records::detail(app, r, p),
        "refund_detail" => records::refund_detail(app, r),
        "new" | "apply" | "invoice_apply" | "refund" => Ok((
            forms::page(app, r, path, p, None, &Errors::new(), &[], None)?,
            200,
        )),
        "void" | "unapply" | "refund_void" => Ok((
            forms::reverse_page(app, r, path, None, &Errors::new())?,
            200,
        )),
        _ => Err(AppError::NotFound(path.into())),
    }
}
pub(super) fn submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    forms::submit(app, r, path, p)
}

type SourceMap = HashMap<String, (Option<SourceVm>, String)>;
pub(super) fn register_sources(
    tx: &(impl fb_db::Query + ?Sized),
    c: CompanyId,
) -> Result<SourceMap, FbError> {
    let mut result = HashMap::new();
    for (table, kind, label, route, parent, field) in [
        (
            "credits_creditmemo",
            "CM",
            "Credit memo",
            "credits:detail",
            "",
            "",
        ),
        (
            "credits_creditrefund",
            "RFD",
            "Refund",
            "credits:refund_detail",
            "credits_creditmemo",
            "credit_memo_id",
        ),
        (
            "sales_receipt",
            "CDP",
            "Customer deposit",
            "prepayments:detail",
            "",
            "",
        ),
        (
            "prepayments_depositrefund",
            "CDR",
            "Deposit refund",
            "prepayments:refund_detail",
            "sales_receipt",
            "deposit_id",
        ),
    ] {
        let mut st=tx.prepare(&format!("SELECT j.id,d.id,d.data,i.number FROM ledger_journal j JOIN documents_documentidentity i ON i.id=j.source_id JOIN {table} d ON d.identity_id=i.id WHERE j.company_id=? AND i.kind=?"))?;
        for row in st.query_map([c.to_db(), kind.into()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })? {
            let (jid, id, raw, number) = row?;
            let d: Value = fb_db::parse_unique_json_as(&raw)
                .map_err(|_| FbError::validation("Invalid stored source."))?;
            let mut party = s(&d["customer_snapshot"], "display_name").to_owned();
            if !parent.is_empty() {
                let parent_number:String=tx.query_row(&format!("SELECT i.number FROM {parent} p JOIN documents_documentidentity i ON i.id=p.identity_id WHERE p.id=?"),[s(&d,field)],|r|r.get(0))?;
                party = format!(
                    "to {party} ({} {parent_number}{})",
                    if kind == "RFD" {
                        "credit memo"
                    } else {
                        "customer deposit"
                    },
                    if s(&d, "check_number").is_empty() {
                        String::new()
                    } else {
                        format!(", check {}", s(&d, "check_number"))
                    }
                );
            }
            result.insert(
                jid,
                (
                    Some(SourceVm {
                        label: label.into(),
                        number: number.clone(),
                        party,
                        url: url(route, &[&c.to_db(), &id]),
                    }),
                    number,
                ),
            );
        }
    }
    let mut st=tx.prepare("SELECT j.id,a.deposit_id,i.number,json_extract(r.data,'$.customer_snapshot.display_name'),di.number,ii.number FROM ledger_journal j JOIN documents_documentidentity i ON i.id=j.source_id JOIN prepayments_depositapplication a ON a.identity_id=i.id JOIN sales_receipt r ON r.id=a.deposit_id JOIN documents_documentidentity di ON di.id=r.identity_id JOIN sales_invoice inv ON inv.id=a.invoice_id JOIN documents_documentidentity ii ON ii.id=inv.identity_id WHERE j.company_id=?")?;
    for row in st.query_map([c.to_db()], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
        ))
    })? {
        let (jid, id, number, party, dep, inv) = row?;
        result.insert(
            jid,
            (
                Some(SourceVm {
                    label: "Deposit application".into(),
                    number: number.clone(),
                    party: format!("{party} ({dep} to {inv})"),
                    url: url("prepayments:detail", &[&c.to_db(), &id]),
                }),
                number,
            ),
        );
    }
    Ok(result)
}
