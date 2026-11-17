//! Sales tax payments: money sent to a tax agency, which is the only entry that may reduce the collected
//! sales tax control account. A payment debits collected sales tax (and, optionally, a penalty/interest
//! expense) and credits one designated bank. It is numbered (`STP-…`), reviewed before it posts, recorded
//! once per request key, audited, and corrected only by a void that posts the exact reversal.
//!
//! Account access is narrow on purpose, and the database repeats every rule (migration 0200):
//! - the bank must be a designated bank account;
//! - the liability line must be the company's configured collected sales tax account;
//! - the optional penalty/interest line must be an expense account that is not a control account.
//!
//! The control-account guard for journals, checks, bills and transfers is unchanged.
//!
//! [`owed`] reports collected tax, payments and the balance per agency for a period, on the owed basis in
//! the sales tax settings. Totals come from the same posted-line conversion as the financial reports, so
//! they always agree with the balance sheet on that basis. Per-agency figures are attributed from each
//! sales document's saved tax snapshot (rate/group members and their agencies); tax that no snapshot names
//! (a manual combined rate, an imported invoice, or an older document) is shown as not linked to an agency.
use crate::checks::support::*;
use crate::reports::financial::{Basis, Books, share};
use crate::{AccountId, Book, CompanyId, FbError, FbResult, Money, UserId, directory, documents, py, services};
use chrono::NaiveDate;
use fb_db::rusqlite::params;
use fb_db::{Query, Writer};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Document kind of a sales tax payment identity.
pub const KIND: &str = "STP";
const MAX_CENTS: i64 = 9_999_999_999_999_999;
/// Label shown for tax that no saved snapshot attributes to an agency.
pub const UNLINKED: &str = "Not linked to an agency";

/// A reviewed payment plan. Cannot be built outside this module, so recording always follows a review.
#[derive(Clone, Debug)]
pub struct Review {
    digest: String,
    pub display: Value,
}
struct Plan {
    mapping: Value,
    entries: Vec<Value>,
    agency: Value,
    review: Review,
}

fn money(cents: i128) -> String {
    Money::from_cents(cents).to_string()
}
fn optional_date(v: &Value, label: &str) -> FbResult<Option<String>> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) if s.is_empty() => Ok(None),
        _ => date(v).map(Some).map_err(|_| err(&format!("Enter a valid {label} date."))),
    }
}
fn amount(v: &Value, label: &str) -> FbResult<i64> {
    let raw = match v {
        Value::Null => json!("0"),
        Value::String(s) if s.trim().is_empty() => json!("0"),
        other => other.clone(),
    };
    let n = cents(&raw).map_err(|_| err(&format!("Enter the {label} as an amount of zero or more, with at most two decimal places.")))?;
    if !(0..=MAX_CENTS).contains(&n) {
        return Err(err(&format!("Enter the {label} as an amount of zero or more, with at most two decimal places.")));
    }
    Ok(n)
}

/// The request as it is bound by retries and reviews.
fn normalize(p: &Value) -> FbResult<Value> {
    let key = py::py_uuid(s(p, "request_key")).ok_or_else(|| err("Supply a stable UUID request key."))?.0;
    let dt = date(&p["date"]).map_err(|_| err("Enter the payment date."))?;
    let agency = id(&p["agency_id"], false).map_err(|_| err("Choose the tax agency you paid."))?.unwrap();
    let bank = id(&p["bank_id"], false).map_err(|_| err("Choose the bank account the payment came from."))?.unwrap();
    let tax = amount(&p["tax_amount"], "sales tax paid")?;
    let penalty = amount(p.get("penalty_amount").unwrap_or(&Value::Null), "penalty and interest")?;
    if tax.checked_add(penalty).is_none_or(|t| t <= 0 || t > MAX_CENTS) {
        return Err(err("Enter the amount paid. The payment total must be more than zero."));
    }
    let expense = if penalty > 0 {
        Some(id(p.get("expense_account_id").unwrap_or(&Value::Null), false).map_err(|_| err("Choose the expense account for the penalty and interest."))?.unwrap())
    } else {
        None
    };
    let start = optional_date(p.get("period_start").unwrap_or(&Value::Null), "period start")?;
    let end = optional_date(p.get("period_end").unwrap_or(&Value::Null), "period end")?;
    if start.is_some() != end.is_some() {
        return Err(err("Enter both dates of the period this payment covers, or leave both blank."));
    }
    if start.as_ref().zip(end.as_ref()).is_some_and(|(a, b)| a > b) {
        return Err(err("The period start must not be after the period end."));
    }
    let reference = crate::card_numbers::card_free(field_text(p, "reference", 40)?, "Reference")?;
    let memo = crate::card_numbers::card_free(field_text(p, "memo", 2000)?, "Memo")?;
    Ok(json!({"request_key":key,"agency_id":agency,"date":dt,"bank_id":bank,"tax_amount":tax,"penalty_amount":penalty,
        "expense_account_id":expense,"period_start_date":start,"period_end_date":end,"reference":reference,"memo":memo}))
}

/// Vendors that are the agency of at least one tax rate in this company (active or not).
pub fn agencies(q: &(impl Query + ?Sized), co: CompanyId) -> FbResult<Vec<Value>> {
    let mut st = q.prepare("SELECT DISTINCT v.id FROM directory_vendor v JOIN salestax_taxitem t ON t.agency_id=v.id AND t.company_id=v.company_id WHERE v.company_id=?1")?;
    let ids = st.query_map([co.to_db()], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    let mut out = ids.iter().map(|id| directory::get(q, co, directory::Model::Vendor, id)).collect::<FbResult<Vec<_>>>()?;
    out.sort_by(|a, b| fb_db::collation::upper(s(a, "display_name")).cmp(&fb_db::collation::upper(s(b, "display_name"))).then_with(|| s(a, "id").cmp(s(b, "id"))));
    Ok(out)
}
fn agency(q: &(impl Query + ?Sized), co: CompanyId, aid: &str) -> FbResult<Value> {
    let v = directory::get(q, co, directory::Model::Vendor, aid).map_err(|_| err("Choose the tax agency you paid."))?;
    let linked: bool = q.query_row("SELECT EXISTS(SELECT 1 FROM salestax_taxitem WHERE company_id=?1 AND agency_id=?2)", params![co.to_db(), aid], |r| r.get(0))?;
    if !linked {
        return Err(err("Choose a tax agency: the authority named on one of your sales tax rates."));
    }
    if v["active"] != true {
        return Err(err("This tax agency is inactive. Reactivate it in Vendors before recording a payment to it."));
    }
    Ok(v)
}

fn prepare(tx: &Writer<'_>, actor: UserId, co: CompanyId, d: &Value) -> FbResult<Plan> {
    let agency = agency(tx, co, s(d, "agency_id"))?;
    let bank = account(tx, co, s(d, "bank_id")).map_err(|_| err("Choose a bank account from this company."))?;
    let setup = documents::setup_snapshot(tx, co)?;
    if bank.kind.as_str() != "ASSET" || !bank.active || !bank.postable || !setup["banks"].as_array().is_some_and(|b| b.contains(&json!(bank.id.to_db()))) {
        return Err(err(&format!("{} · {} is not an active bank account in Accounting setup. Choose the bank account the payment came from.", bank.code, bank.name)));
    }
    let tax = d["tax_amount"].as_i64().unwrap();
    let penalty = d["penalty_amount"].as_i64().unwrap();
    let mut extra = vec![];
    let expense = match d["expense_account_id"].as_str() {
        Some(eid) => {
            let a = account(tx, co, eid).map_err(|_| err("Choose an expense account from this company for the penalty and interest."))?;
            if documents::control_role(tx, co, a.id)?.is_some() || a.kind.as_str() != "EXPENSE" || !a.active || !a.postable || a.id == bank.id {
                return Err(err(&format!("{} · {} cannot take the penalty and interest. Choose an active expense account.", a.code, a.name)));
            }
            extra.push(a.id);
            Some(a)
        }
        None => None,
    };
    let roles: &[&str] = if tax > 0 { &["collected_tax"] } else { &[] };
    let locked = documents::lock_configuration(tx, actor, co, roles, Some(bank.id), &extra)
        .map_err(|e| if e.message().contains("collected tax") { err("Choose the account for sales tax you collect in Accounting setup before recording a sales tax payment.") } else { e })?;
    let mut mapping = json!({"bank":bank.id.to_db()});
    let mut entries = vec![];
    if tax > 0 {
        mapping["collected_tax"] = locked["collected_tax"].clone();
        entries.push(json!({"account":locked["collected_tax"],"debit":money(tax.into()),"credit":"0.00"}));
    }
    if let Some(a) = &expense {
        mapping["expense"] = json!(a.id.to_db());
        entries.push(json!({"account":a.id.to_db(),"debit":money(penalty.into()),"credit":"0.00"}));
    }
    entries.push(json!({"account":bank.id.to_db(),"debit":"0.00","credit":money((tax + penalty).into())}));
    let display = display_mapping(tx, co, &mapping)?;
    let company = company_snap(tx, co)?;
    let party = json!({"id":agency["id"],"display_name":agency["display_name"]});
    let digest = digest(&json!({"company":co.to_db(),"intent":d,"mapping":display,"entries":entries,"agency":party,"company_snapshot":company}));
    // What is owed on the payment date, on the owed basis in the settings: shown, never bound by the review.
    let report = owed(tx, co, None, s(d, "date"))?;
    let owed_total = report_cents(&report["total"]["balance_cents"]).unwrap_or(0);
    let owed_agency = report["agencies"].as_array().unwrap().iter().find(|a| a["agency_id"] == agency["id"]).map(|a| report_cents(&a["balance_cents"]).unwrap_or(0)).unwrap_or(0);
    let mut warnings = vec![];
    if i128::from(tax) > owed_total {
        warnings.push(format!(
            "This is {} more than all the sales tax owed on {} ({}). The extra stays in the books as tax paid in advance. If part of the payment is a penalty or interest, enter that part separately.",
            money(i128::from(tax) - owed_total), s(d, "date"), money(owed_total)
        ));
    } else if i128::from(tax) > owed_agency {
        warnings.push(format!(
            "This is more than the {} shown as owed to {} on {}. Check the agency, or the tax on sales that are not linked to an agency.",
            money(owed_agency), s(&agency, "display_name"), s(d, "date")
        ));
    }
    let shown: Vec<Value> = entries.iter().map(|e| {
        let a = account(tx, co, s(e, "account")).ok();
        json!({"account":a.map(|a|format!("{} · {}",a.code,a.name)).unwrap_or_default(),"debit":if e["debit"]=="0.00"{Value::Null}else{e["debit"].clone()},"credit":if e["credit"]=="0.00"{Value::Null}else{e["credit"].clone()}})
    }).collect();
    Ok(Plan {
        mapping,
        entries,
        agency: party.clone(),
        review: Review {
            digest,
            display: json!({"agency":party,"mapping":display,"entries":shown,"total":money((tax+penalty).into()),"tax_amount":money(tax.into()),"penalty_amount":money(penalty.into()),
                "owed_on_date":money(owed_total),"owed_to_agency":money(owed_agency),"basis":report["basis"],"basis_label":report["basis_label"],"warnings":warnings}),
        },
    })
}

pub fn review_payment(book: &mut Book, actor: UserId, co: CompanyId, p: &Value) -> FbResult<Review> {
    write(book, |tx| review_payment_tx(tx, actor, co, p))
}
/// Review inside a caller's writer. Nothing is kept: the savepoint is rolled back by the caller's error
/// path only; a successful review writes nothing.
pub fn review_payment_tx(tx: &Writer<'_>, actor: UserId, co: CompanyId, p: &Value) -> FbResult<Review> {
    tx.atomic(|| {
        services::access(tx, actor, co)?;
        Ok(prepare(tx, actor, co, &normalize(p)?)?.review)
    })
}
pub fn record_payment(book: &mut Book, actor: UserId, co: CompanyId, p: &Value, review: &Review) -> FbResult<Value> {
    write(book, |tx| record_payment_tx(tx, actor, co, p, review))
}
/// Atomic record primitive; only an active writer can be supplied.
/// ```compile_fail
/// use fb_core::{salestax_payments, Book, CompanyId, UserId};
/// fn cannot_write(book: &mut Book, actor: UserId, company: CompanyId, review: &salestax_payments::Review) {
///     book.read(|r| salestax_payments::record_payment_tx(r, actor, company, &serde_json::json!({}), review));
/// }
/// ```
pub fn record_payment_tx(tx: &Writer<'_>, actor: UserId, co: CompanyId, p: &Value, review: &Review) -> FbResult<Value> {
    tx.atomic(|| {
        let u = services::access(tx, actor, co)?;
        let d = normalize(p)?;
        let creation = digest(&d);
        if let Some(v) = rows(tx, "SELECT t.id,t.creation_digest,t.review_digest FROM salestax_payment t JOIN documents_documentidentity i ON i.id=t.identity_id WHERE t.company_id=? AND i.request_key=?", &[&co.to_db(), &s(&d, "request_key")])?.pop() {
            if v["creation_digest"] != creation || v["review_digest"] != review.digest {
                return Err(err("This request was already recorded as a different sales tax payment. Start a new payment instead."));
            }
            return payment(tx, co, s(&v, "id"));
        }
        let identity = documents::create_identity(tx, actor, co, KIND, s(&d, "request_key"))?;
        let plan = prepare(tx, actor, co, &d)?;
        if plan.review.digest != review.digest {
            return Err(err("The payment or its accounts changed since the review. Review it again before recording."));
        }
        documents::snapshot_source_accounts(tx, actor, co, s(&identity, "id"), &plan.mapping)?;
        let number = s(&identity, "number").to_owned();
        let name = s(&plan.agency, "display_name").to_owned();
        let memo = py::py_truncate(&format!("{number}: Sales tax payment to {name}{}", if s(&d, "reference").is_empty() { String::new() } else { format!(" ({})", s(&d, "reference")) }), 240);
        let j = documents::post_source(tx, actor, co, s(&identity, "id"), s(&d, "date"), &memo, plan.entries)?;
        let id = fb_ids::new_v7().to_string();
        let row = json!({"id":id,"company_id":co.to_db(),"identity_id":identity["id"],"creation_digest":creation,"review_digest":review.digest,
            "agency_id":d["agency_id"],"agency_name":name,"bank_id":d["bank_id"],"tax_account_id":plan.mapping.get("collected_tax").cloned().unwrap_or(Value::Null),
            "expense_account_id":d["expense_account_id"],"date":d["date"],"tax_amount":d["tax_amount"],"penalty_amount":d["penalty_amount"],
            "period_start_date":d["period_start_date"],"period_end_date":d["period_end_date"],"reference":d["reference"],"memo":d["memo"],"journal_id":j.id.to_db(),
            "actor_id":actor.to_db(),"actor_name":u.username,"created_at":py::now_utc()});
        insert(tx, "salestax_payment", &row)?;
        seal(tx, &j.id.to_db())?;
        let tax = d["tax_amount"].as_i64().unwrap();
        let penalty = d["penalty_amount"].as_i64().unwrap();
        services::audit(tx, &u, co, "salestax.payment.recorded", format!("salestaxpayment:{id}"), json!({"number":number,"agency_id":d["agency_id"],"agency":name,"date":d["date"],
            "bank_id":d["bank_id"],"tax_amount":money(tax.into()),"penalty_amount":money(penalty.into()),"expense_account_id":d["expense_account_id"],"period_start_date":d["period_start_date"],
            "period_end_date":d["period_end_date"],"reference":d["reference"],"memo":d["memo"],"journal_id":j.id.to_db()}))?;
        payment(tx, co, &id)
    })
}

/// Void by posting the exact reversal on `dt` (on or after the payment date). Retries with the same
/// date and reason return the recorded void.
pub fn void_payment(book: &mut Book, actor: UserId, co: CompanyId, pid: &str, expected: Option<i64>, dt: &Value, reason: &Value) -> FbResult<Value> {
    write(book, |tx| {
        let u = services::access(tx, actor, co)?;
        let t = rows(tx, "SELECT * FROM salestax_payment WHERE company_id=? AND id=?", &[&co.to_db(), &pid])?.pop().ok_or_else(|| FbError::missing("Sales tax payment"))?;
        let dt = date(dt).map_err(|_| err("Enter the void date."))?;
        let reason = text(reason, 500, true).map_err(|_| err("Enter a reason for the void (up to 500 characters)."))?;
        if let Some(v) = rows(tx, "SELECT * FROM salestax_paymentvoid WHERE payment_id=?", &[&pid])?.pop() {
            if expected == Some(1) && v["date"] == dt && v["reason"] == reason {
                return payment(tx, co, pid);
            }
            return Err(err("This sales tax payment is already voided (with a different date or reason, or from a page that was out of date)."));
        }
        documents::check_revision(1, expected)?;
        if dt.as_str() < s(&t, "date") {
            return Err(err("The void date cannot be before the payment date."));
        }
        let j = documents::reverse_source(tx, actor, co, s(&t, "identity_id"), &dt, &reason)?;
        let vid = fb_ids::new_v7().to_string();
        insert(tx, "salestax_paymentvoid", &json!({"id":vid,"payment_id":pid,"journal_id":j.id.to_db(),"date":dt,"reason":reason,"actor_id":actor.to_db(),"actor_name":u.username,"created_at":py::now_utc()}))?;
        seal(tx, &j.id.to_db())?;
        let identity = documents::identity(tx, co, s(&t, "identity_id"))?;
        services::audit(tx, &u, co, "salestax.payment.voided", format!("salestaxpayment:{pid}"), json!({"number":identity["number"],"reason":reason,"date":dt,"journal_id":j.id.to_db()}))?;
        payment(tx, co, pid)
    })
}
fn seal(tx: &Writer<'_>, jid: &str) -> FbResult<()> {
    tx.execute("INSERT INTO fb_seal VALUES('salestax_journal',?)", [jid])?;
    Ok(())
}

/// One payment with its number, void and posted lines (debit/credit as display strings or null).
pub fn payment(q: &(impl Query + ?Sized), co: CompanyId, pid: &str) -> FbResult<Value> {
    let mut v = rows(q, "SELECT t.*,i.number,i.request_key FROM salestax_payment t JOIN documents_documentidentity i ON i.id=t.identity_id WHERE t.company_id=? AND t.id=?", &[&co.to_db(), &pid])?
        .pop().ok_or_else(|| FbError::missing("Sales tax payment"))?;
    let tax = v["tax_amount"].as_i64().unwrap_or(0);
    let penalty = v["penalty_amount"].as_i64().unwrap_or(0);
    v["tax_amount_cents"] = json!(tax);
    v["penalty_amount_cents"] = json!(penalty);
    v["total_cents"] = json!(tax + penalty);
    v["tax_amount"] = json!(money(tax.into()));
    v["penalty_amount"] = json!(money(penalty.into()));
    v["total"] = json!(money((tax + penalty).into()));
    let bank = account(q, co, s(&v, "bank_id"))?;
    v["bank_label"] = json!(format!("{} · {}", bank.code, bank.name));
    let void = rows(q, "SELECT * FROM salestax_paymentvoid WHERE payment_id=?", &[&pid])?.pop();
    v["voided"] = json!(void.is_some());
    v["state_label"] = json!(if void.is_some() { "Voided" } else { "Recorded" });
    v["revision"] = json!(1);
    let lines = |jid: &str| -> FbResult<Vec<Value>> {
        let mut st = q.prepare("SELECT a.code,a.name,l.debit,l.credit FROM ledger_journalline l JOIN ledger_account a ON a.id=l.account_id WHERE l.journal_id=?1 ORDER BY l.position")?;
        Ok(st.query_map([jid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?)))?
            .map(|row| row.map(|(code, name, debit, credit)| json!({"account":format!("{code} · {name}"),"debit":(debit>0).then(||money(debit.into())),"credit":(credit>0).then(||money(credit.into()))})))
            .collect::<Result<Vec<_>, _>>()?)
    };
    v["lines"] = json!(lines(s(&v, "journal_id"))?);
    if let Some(mut void) = void {
        void["lines"] = json!(lines(s(&void, "journal_id"))?);
        v["void"] = void;
    } else {
        v["void"] = Value::Null;
    }
    Ok(v)
}
/// Every payment of the company, newest first.
pub fn payments(q: &(impl Query + ?Sized), co: CompanyId) -> FbResult<Vec<Value>> {
    let mut st = q.prepare("SELECT id FROM salestax_payment WHERE company_id=?1 ORDER BY date DESC,id DESC")?;
    let ids = st.query_map([co.to_db()], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    ids.iter().map(|id| payment(q, co, id)).collect()
}

// What is owed ---------------------------------------------------------------------------------------------

/// Accounts that hold collected sales tax: the current setup account and any a document froze.
fn tax_accounts(q: &(impl Query + ?Sized), co: CompanyId) -> FbResult<BTreeSet<AccountId>> {
    let mut st = q.prepare("SELECT DISTINCT account_id FROM fb_control_accounts WHERE company_id=?1 AND role='collected_tax'")?;
    let ids = st.query_map([co.to_db()], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(ids.iter().filter_map(|i| AccountId::parse(i)).collect())
}
/// Per sales document identity: positive tax cents per agency from its saved tax snapshot.
fn weights(q: &(impl Query + ?Sized), co: CompanyId) -> FbResult<HashMap<String, Vec<(String, Money)>>> {
    let mut out: HashMap<String, Vec<(String, Money)>> = HashMap::new();
    for sql in [
        "SELECT i.identity_id,r.data FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id WHERE i.company_id=?1 AND r.journal_id IS NOT NULL ORDER BY r.number",
        "SELECT r.identity_id,x.data FROM sales_receipt r JOIN sales_salesreceipt x ON x.receipt_id=r.id WHERE r.company_id=?1",
        "SELECT identity_id,data FROM credits_creditmemo WHERE company_id=?1",
    ] {
        let mut st = q.prepare(sql)?;
        for row in st.query_map([co.to_db()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (identity, raw) = row?;
            if out.contains_key(&identity) {
                continue;
            }
            let data: Value = fb_db::parse_unique_json_as(&raw).map_err(|_| err("Invalid stored sales document."))?;
            let mut by: BTreeMap<String, i128> = BTreeMap::new();
            for m in data["tax_snapshot"]["members"].as_array().into_iter().flatten() {
                let (Some(agency), Some(tax)) = (m["agency_id"].as_str(), m["tax"].as_str()) else { continue };
                let Ok(n) = crate::calculations::unit_rate(&json!(tax)) else { continue };
                *by.entry(agency.to_owned()).or_default() += n.mantissa();
            }
            let list: Vec<(String, Money)> = by.into_iter().filter(|(_, n)| *n > 0).map(|(a, n)| (a, Money::from_cents(n))).collect();
            out.insert(identity, list);
        }
    }
    Ok(out)
}
/// Split signed `amount` across agencies in proportion to `w`; magnitude first, so a reversal mirrors
/// its original exactly. Returns None when nothing attributes it.
fn attribute(w: Option<&Vec<(String, Money)>>, amount: Money) -> FbResult<Option<BTreeMap<String, Money>>> {
    let Some(w) = w.filter(|w| !w.is_empty()) else { return Ok(None) };
    let total: Money = w.iter().map(|(_, n)| *n).sum();
    let magnitude = Money::from_cents(amount.cents().abs());
    let parts = share(w, total, magnitude)?;
    Ok(Some(parts.into_iter().map(|(k, v)| (k, if amount.cents() < 0 { -v } else { v })).collect()))
}

// Keep ordinary JSON numbers compatible; aggregates wider than i64 use decimal
// strings, because JSON consumers cannot universally preserve a wider integer.
fn cents_json(n: i128) -> Value {
    i64::try_from(n).map_or_else(|_| json!(n.to_string()), |n| json!(n))
}
/// Exact cents from an owed-report aggregate (a number or a wide decimal string).
pub fn report_cents(value: &Value) -> Option<i128> {
    value.as_i64().map(i128::from).or_else(|| value.as_str()?.parse().ok())
}

#[derive(Default, Clone, Copy)]
struct Figures {
    opening: i128,
    collected: i128,
    paid: i128,
}
impl Figures {
    fn add(&mut self, before: bool, collected: i128, paid: i128) {
        if before {
            self.opening += collected - paid;
        } else {
            self.collected += collected;
            self.paid += paid;
        }
    }
    fn json(self) -> Value {
        let balance = self.opening + self.collected - self.paid;
        json!({"opening":money(self.opening),"collected":money(self.collected),"paid":money(self.paid),"balance":money(balance),
            "opening_cents":cents_json(self.opening),"collected_cents":cents_json(self.collected),"paid_cents":cents_json(self.paid),"balance_cents":cents_json(balance)})
    }
}
fn parse(d: &str) -> FbResult<NaiveDate> {
    NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| err("Enter a valid date."))
}

/// Report with access check.
pub fn owed_report(book: &mut Book, actor: UserId, co: CompanyId, start: Option<&str>, end: &str) -> FbResult<Value> {
    book.read(|q| {
        services::access(q, actor, co)?;
        owed(q, co, start, end)
    })
}

/// Collected, paid and balance per agency for `start..=end` (start None = from the beginning), on the
/// owed basis in the sales tax settings. `total` always equals the collected sales tax balance on that
/// basis (the balance sheet figure).
pub fn owed(q: &(impl Query + ?Sized), co: CompanyId, start: Option<&str>, end: &str) -> FbResult<Value> {
    let settings = crate::salestax::settings_for(q, co)?;
    let basis = Basis::parse(s(&settings, "owed_basis")).unwrap_or(Basis::Accrual);
    let end_d = parse(end)?;
    let start_d = start.map(parse).transpose()?;
    if start_d.is_some_and(|s| s > end_d) {
        return Err(err("The start date must not be after the end date."));
    }
    let accounts = tax_accounts(q, co)?;
    let w = weights(q, co)?;
    let mut paid_by: HashMap<String, String> = HashMap::new();
    {
        let mut st = q.prepare("SELECT identity_id,agency_id FROM salestax_payment WHERE company_id=?1")?;
        for row in st.query_map([co.to_db()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (i, a) = row?;
            paid_by.insert(i, a);
        }
    }
    let mut agencies: BTreeMap<String, Figures> = BTreeMap::new();
    let mut unlinked = Figures::default();
    let mut book_total = Figures::default();
    let before = |d: NaiveDate| start_d.is_some_and(|s| d < s);
    let credit = |identity: Option<&str>, d: NaiveDate, net_credit: Money, agencies: &mut BTreeMap<String, Figures>, unlinked: &mut Figures| -> FbResult<()> {
        let b = before(d);
        if let Some(agency) = identity.and_then(|i| paid_by.get(i)) {
            agencies.entry(agency.clone()).or_default().add(b, 0, -net_credit.cents());
            return Ok(());
        }
        match attribute(identity.and_then(|i| w.get(i)), net_credit)? {
            Some(parts) => {
                for (agency, n) in parts {
                    agencies.entry(agency).or_default().add(b, n.cents(), 0);
                }
            }
            None => unlinked.add(b, net_credit.cents(), 0),
        }
        Ok(())
    };
    if !accounts.is_empty() {
        let mut st = q.prepare("SELECT l.account_id,l.debit,l.credit,j.date,j.source_id,coalesce(s.kind,'') FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id JOIN ledger_account a ON a.id=l.account_id LEFT JOIN documents_documentidentity s ON s.id=j.source_id WHERE j.company_id=?1 AND a.company_id=?1 AND j.status='POSTED' AND j.date<=?2 AND l.account_id IN(SELECT account_id FROM fb_control_accounts WHERE company_id=?1 AND role='collected_tax') ORDER BY j.date,j.number,l.position")?;
        let rows = st.query_map(params![co.to_db(), end], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, String>(5)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        for (account, debit, credit_cents, d, source, kind) in rows {
            if !AccountId::parse(&account).is_some_and(|a| accounts.contains(&a)) {
                continue;
            }
            // The financial reports leave deferred documents out on the cash basis and recognise them as paid.
            if basis == Basis::Cash && ["INV", "BILL", "CM"].contains(&kind.as_str()) {
                continue;
            }
            credit(source.as_deref(), parse(&d)?, Money::from_db(credit_cents) - Money::from_db(debit), &mut agencies, &mut unlinked)?;
        }
        let books = Books::load(q, co, basis, end_d)?;
        if basis == Basis::Cash {
            for row in books.recognition_rows(None, Some(end_d), Some(&accounts))? {
                credit(Some(row.document.identity.as_str()), row.date, row.credit - row.debit, &mut agencies, &mut unlinked)?;
            }
        }
        // The balance-sheet figures for the same basis; any difference is shown as not linked to an agency.
        let net = |nets: HashMap<AccountId, (Money, i64)>| -> i128 { accounts.iter().map(|a| -nets.get(a).map(|n| n.0.cents()).unwrap_or(0)).sum() };
        let through = net(books.nets(None, Some(end_d))?);
        let opening = match start_d { Some(s) => net(books.nets(None, s.pred_opt())?), None => 0 };
        let period_paid: i128 = agencies.values().map(|f| f.paid).sum();
        book_total = Figures { opening, collected: through - opening + period_paid, paid: period_paid };
    }
    let mut sum = unlinked;
    for f in agencies.values() {
        sum.opening += f.opening;
        sum.collected += f.collected;
        sum.paid += f.paid;
    }
    unlinked.opening += book_total.opening - sum.opening;
    unlinked.collected += book_total.collected - sum.collected;
    let names: HashMap<String, String> = {
        let mut st = q.prepare("SELECT id,display_name FROM directory_vendor WHERE company_id=?1")?;
        st.query_map([co.to_db()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<_, _>>()?
    };
    // Every agency on a tax rate gets a row, even with nothing collected yet.
    for a in agencies_of(q, co)? {
        agencies.entry(s(&a, "id").to_owned()).or_default();
    }
    let mut rows: Vec<Value> = agencies.iter().map(|(id, f)| {
        let mut v = f.json();
        v["agency_id"] = json!(id);
        v["name"] = json!(names.get(id).cloned().unwrap_or_else(|| id.clone()));
        v
    }).collect();
    rows.sort_by(|a, b| fb_db::collation::upper(s(a, "name")).cmp(&fb_db::collation::upper(s(b, "name"))).then_with(|| s(a, "agency_id").cmp(s(b, "agency_id"))));
    let show_unlinked = unlinked.opening != 0 || unlinked.collected != 0 || unlinked.paid != 0;
    let mut u = unlinked.json();
    u["name"] = json!(UNLINKED);
    Ok(json!({"basis":basis.as_str(),"basis_label":if basis==Basis::Cash{"Cash basis: tax is owed when the customer pays"}else{"Accrual basis: tax is owed as of the invoice date"},
        "start":start,"end":end,"agencies":rows,"unlinked":if show_unlinked{u}else{Value::Null},"total":book_total.json(),
        "accounts":accounts.iter().map(|a|a.to_db()).collect::<Vec<_>>()}))
}
fn agencies_of(q: &(impl Query + ?Sized), co: CompanyId) -> FbResult<Vec<Value>> {
    agencies(q, co)
}

#[cfg(test)]
#[path = "salestax_payments_tests.rs"]
mod tests;
