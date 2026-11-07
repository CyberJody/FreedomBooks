//! Customer deposits held as liabilities, invoice applications and record-only refunds.
//! All writes, including nested document primitives and audits, share one atomic Writer.
use crate::{
    AccountId, Book, CompanyId, FbError, FbResult, Money, UserId, calculations,
    directory::{self, Model},
    documents, py, sales, services, store,
};
use fb_db::{
    Query, Writer,
    rusqlite::{OptionalExtension, params, types::Value as SqlValue},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

mod applications;
mod refunds;
pub use applications::{apply_deposit, invoice_open_balance, review_application, unapply_deposit};
pub use refunds::{record_refund, review_refund, void_refund};

#[cfg(test)]
mod integration_tests;
#[cfg(test)]
mod guard_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod secfix1_tests;

const SETUP_MESSAGE: &str = "Choose a Customer deposits account (type Other current liability) in Accounting setup before receiving a customer deposit.";
#[derive(Clone, Debug)]
pub struct Review {
    domain: &'static str,
    digest: String,
    pub display: Value,
}
fn err(s: impl Into<String>) -> FbError {
    FbError::validation(s)
}
fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
fn new_id() -> String {
    fb_ids::new_v7().to_string()
}
fn digest(v: &Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(py::py_json_dumps_sorted(v).as_bytes())
    )
}
fn text(v: &Value, max: usize, required: bool) -> FbResult<String> {
    if let Some(raw) = v.as_str() { py::clean_free_text(raw)?; }
    v.as_str()
        .map(py::py_strip)
        .filter(|s| py::py_len(s) <= max && (!required || !s.is_empty()))
        .map(str::to_owned)
        .ok_or_else(|| {
            err(format!(
                "Enter {}text up to {max} characters.",
                if required { "nonblank " } else { "" }
            ))
        })
}
fn field(v: &Value, k: &str, max: usize) -> FbResult<String> {
    text(v.get(k).unwrap_or(&json!("")), max, false)
}
fn id(v: &Value, optional: bool) -> FbResult<Option<String>> {
    if optional && v.is_null() {
        return Ok(None);
    }
    v.as_str()
        .filter(|s| fb_ids::CompanyId::parse(s).is_some())
        .map(|s| Some(s.to_owned()))
        .ok_or_else(|| err("Choose a record from this company."))
}
fn key(v: &Value) -> FbResult<String> {
    v.as_str()
        .and_then(py::py_uuid)
        .map(|v| v.0)
        .ok_or_else(|| err("Supply a stable UUID request key."))
}
fn date(v: &Value) -> FbResult<String> {
    v.as_str().and_then(fb_db::dates::parse_iso_date).ok_or_else(|| err("Enter a valid calendar date."))
}

fn cents(v: &Value) -> FbResult<i64> {
    Money::from_decimal_exact(calculations::unit_rate(v)?)
        .and_then(|m| m.to_db())
        .ok_or_else(|| err("Document amount exceeds 99999999999999.99."))
}
fn money(v: &Value, what: &str) -> FbResult<Value> {
    let n = cents(v)?;
    if n <= 0 {
        return Err(err(format!("{what} must be positive.")));
    }
    Ok(cash(n))
}
fn cash(n: i64) -> Value {
    json!(Money::from_db(n).to_string())
}
fn command<T>(book: &mut Book, f: impl FnOnce(&Writer<'_>) -> FbResult<T>) -> FbResult<T> {
    book.write(|tx| tx.atomic(|| f(tx)))
}
fn token<'a>(r: Option<&'a Review>, domain: &str, what: &str) -> FbResult<&'a str> {
    r.filter(|r| r.domain == domain)
        .map(|r| r.digest.as_str())
        .ok_or_else(|| err(format!("Invalid review. Open a fresh {what} review.")))
}
fn account(q: &(impl Query + ?Sized), c: CompanyId, key: &str) -> FbResult<crate::model::Account> {
    AccountId::parse(key)
        .map(|a| store::account(q, a))
        .transpose()?
        .flatten()
        .filter(|a| a.company_id == c)
        .ok_or_else(|| err("Posting accounts must belong to this company."))
}
fn party(v: &Value) -> Value {
    let mut out = json!({});
    for k in [
        "id",
        "display_name",
        "business_name",
        "contact_name",
        "billing_address",
        "email",
        "phone",
    ] {
        out[k] = v[k].clone();
    }
    out
}
fn company_snapshot(q: &(impl Query + ?Sized), c: CompanyId) -> FbResult<Value> {
    let co = store::get_company(q, c)?;
    Ok(json!({"id":c.to_db(),"name":co.name,"currency":co.currency}))
}
fn display(q: &(impl Query + ?Sized), c: CompanyId, mapping: &Value) -> FbResult<Vec<Value>> {
    let m = mapping
        .as_object()
        .ok_or_else(|| err("Invalid account mapping."))?;
    let mut pairs = m.iter().collect::<Vec<_>>();
    pairs.sort_by_key(|(role, _)| *role);
    pairs.into_iter().map(|(role,k)| { let a=account(q,c,k.as_str().ok_or_else(||err("Invalid account mapping."))?)?; Ok(json!({"role":role,"id":a.id.to_db(),"code":a.code,"name":a.name,"kind":a.kind.as_str()})) }).collect()
}
fn entries(debit: &str, credit: &str, amount: &Value) -> Vec<Value> {
    vec![
        json!({"account":debit,"debit":amount,"credit":"0.00"}),
        json!({"account":credit,"debit":"0.00","credit":amount}),
    ]
}
fn summary(number: &str, text: &str) -> String {
    py::py_truncate(&format!("{number}: {text}"), 240)
}
fn load(
    q: &(impl Query + ?Sized),
    table: &str,
    column: &str,
    key: &str,
) -> FbResult<Option<Value>> {
    let raw: Option<String> = q
        .query_row(
            &format!("SELECT data FROM {table} WHERE {column}=?"),
            [key],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|s| {
        let value: Value =
            fb_db::parse_unique_json_as(&s).map_err(|_| err("Invalid stored prepayment."))?;
        let kind = match table {
            "prepayments_depositapplication" => "application",
            "prepayments_depositrefund" => "refund",
            "prepayments_depositunapplication" => "unapplication",
            "prepayments_depositrefundvoid" => "refundvoid",
            "sales_receiptreversal" => "depositvoid",
            _ => return Err(err("Unknown stored prepayment table.")),
        };
        if !fb_db::prepayments::prepayment_snapshot_ok(kind, &value) {
            return Err(err("Invalid stored prepayment evidence."));
        }
        Ok(value)
    })
    .transpose()
}
fn insert(tx: &Writer<'_>, table: &str, row: &Value, columns: &[&str]) -> FbResult<()> {
    let vals = columns
        .iter()
        .map(|k| match &row[*k] {
            Value::String(s) => Ok(SqlValue::Text(s.clone())),
            Value::Null => Ok(SqlValue::Null),
            Value::Number(n) => n
                .as_i64()
                .map(SqlValue::Integer)
                .ok_or_else(|| err("Invalid stored integer.")),
            _ => Err(err("Invalid stored field.")),
        })
        .collect::<FbResult<Vec<_>>>()?;
    tx.execute(
        &format!(
            "INSERT INTO {table}({}) VALUES({})",
            columns.join(","),
            vec!["?"; columns.len()].join(",")
        ),
        fb_db::rusqlite::params_from_iter(vals),
    )?;
    Ok(())
}
fn persist(tx: &Writer<'_>, table: &str, row: &Value, columns: &[&str]) -> FbResult<()> {
    let mut stored = row.clone();
    let mut data = row.clone();
    if table == "sales_receiptreversal" {
        data.as_object_mut()
            .ok_or_else(|| err("Invalid reversal."))?
            .remove("actor_id");
    }
    stored["data"] = json!(data.to_string());
    if row.get("amount").is_some() {
        stored["amount"] = json!(cents(&row["amount"])?);
    }
    insert(tx, table, &stored, columns)
}
fn seal(tx: &Writer<'_>, journal: &str) -> FbResult<()> {
    tx.execute(
        "INSERT INTO prepayments_seal VALUES('journal',?)",
        [journal],
    )?;
    Ok(())
}
fn sales_seal(tx: &Writer<'_>, kind: &str, id: &str) -> FbResult<()> {
    tx.execute("INSERT INTO sales_seal VALUES(?,?)", params![kind, id])?;
    Ok(())
}
fn sum(q: &(impl Query + ?Sized), sql: &str, key: &str) -> FbResult<i64> {
    let mut st = q.prepare(sql)?;
    st.query_map([key], |r| r.get::<_, i64>(0))?
        .try_fold(0i64, |a, b| {
            a.checked_add(b?)
                .ok_or_else(|| err("Prepayment balance exceeds supported limits."))
        })
}
fn deposit(q: &(impl Query + ?Sized), c: CompanyId, id: &str) -> FbResult<Value> {
    let mut v = sales::receipt(q, c, id).map_err(|e| match e {
        FbError::DoesNotExist(_) => FbError::DoesNotExist(String::new()),
        other => other,
    })?;
    if v["kind"] != "CDP" {
        return Err(FbError::DoesNotExist(String::new()));
    }
    let detail=q.query_row("SELECT id,liability_id,estimate_id,order_reference FROM prepayments_customerdeposit WHERE receipt_id=?",[id],|r|Ok(json!({"id":r.get::<_,String>(0)?,"liability_id":r.get::<_,String>(1)?,"estimate_id":r.get::<_,Option<String>>(2)?,"order_reference":r.get::<_,String>(3)?}))).optional()?.ok_or_else(||err("Missing customer deposit detail."))?;
    v["prepayment"] = detail;
    Ok(v)
}
pub fn live_applied(q: &(impl Query + ?Sized), deposit_id: &str) -> FbResult<i64> {
    sum(
        q,
        "SELECT amount FROM prepayments_depositapplication a WHERE deposit_id=? AND NOT EXISTS(SELECT 1 FROM prepayments_depositunapplication u WHERE u.application_id=a.id)",
        deposit_id,
    )
}
pub fn live_refunded(q: &(impl Query + ?Sized), deposit_id: &str) -> FbResult<i64> {
    sum(
        q,
        "SELECT amount FROM prepayments_depositrefund r WHERE deposit_id=? AND NOT EXISTS(SELECT 1 FROM prepayments_depositrefundvoid v WHERE v.refund_id=r.id)",
        deposit_id,
    )
}
pub fn remaining(q: &(impl Query + ?Sized), receipt: &Value) -> FbResult<i64> {
    let id = s(receipt, "id");
    if load(q, "sales_receiptreversal", "receipt_id", id)?.is_some() {
        return Ok(0);
    }
    cents(&receipt["amount"])?
        .checked_sub(live_applied(q, id)?)
        .and_then(|v| v.checked_sub(live_refunded(q, id).ok()?))
        .filter(|v| *v >= 0)
        .ok_or_else(|| err("Invalid customer deposit balance."))
}
pub fn deposit_balances(book: &mut Book, actor: UserId, c: CompanyId) -> FbResult<Vec<Value>> {
    book.read(|q| {
        services::access(q, actor, c)?;
        let mut st = q.prepare(
            "SELECT id FROM sales_receipt WHERE company_id=? AND kind='CDP' ORDER BY date,id",
        )?;
        let ids = st
            .query_map([c.to_db()], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids.iter()
            .map(|id| {
                let mut r = deposit(q, c, id)?;
                let left = remaining(q, &r)?;
                let void = load(q, "sales_receiptreversal", "receipt_id", id)?.is_some();
                r["applied"] = cash(live_applied(q, id)?);
                r["refunded"] = cash(live_refunded(q, id)?);
                r["remaining"] = cash(left);
                r["deposit_state"] = json!(if void {
                    "VOID"
                } else if left == 0 {
                    "USED"
                } else if left < cents(&r["amount"])? {
                    "PARTIAL"
                } else {
                    "AVAILABLE"
                });
                Ok(r)
            })
            .collect()
    })
}
pub fn unapplied_total(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    customer: Option<&str>,
) -> FbResult<Money> {
    deposit_balances(book, actor, c)?
        .iter()
        .filter(|r| customer.is_none_or(|id| r["customer_id"] == id))
        .try_fold(0i64, |a, r| {
            a.checked_add(cents(&r["remaining"])?)
                .ok_or_else(|| err("Prepayment total exceeds supported limits."))
        })
        .map(Money::from_db)
}
fn normalize(p: &Value) -> FbResult<Value> {
    let key = key(&p["request_key"])?;
    if !["CHECK", "CASH", "CARD", "TRANSFER", "OTHER"].contains(&s(p, "method")) {
        return Err(err("Choose a payment method (record only)."));
    }
    if !["BANK", "UNDEPOSITED"].contains(&s(p, "destination")) {
        return Err(err("Choose where the deposit goes."));
    }
    let bank = id(&p["bank_id"], p["destination"] == "UNDEPOSITED")?;
    if p["destination"] == "UNDEPOSITED" && bank.is_some() {
        return Err(err("Do not select a bank for undeposited funds."));
    }
    Ok(
        json!({"request_key":key,"customer_id":id(&p["customer_id"],false)?,"date":date(&p["date"])? ,"amount":money(&p["amount"],"Deposit amount")?,"method":p["method"],"reference":crate::card_numbers::card_free(field(p,"reference",120)?,"Reference")?,"memo":crate::card_numbers::card_free(field(p,"memo",2000)?,"Memo")?,"destination":p["destination"],"bank_id":bank,"estimate_id":id(&p["estimate_id"],true)?,"order_reference":crate::card_numbers::card_free(field(p,"order_reference",120)?,"Order reference")?}),
    )
}
fn prepare(tx: &Writer<'_>, actor: UserId, c: CompanyId, d: &Value) -> FbResult<Value> {
    let estimate_number = if let Some(id) = d["estimate_id"].as_str() {
        let estimate = sales::estimate(tx, c, id).map_err(|e| match e {
            FbError::DoesNotExist(_) => err("Choose an estimate from this company."),
            other => other,
        })?;
        let identity = documents::identity(tx, c, s(&estimate, "identity_id"))?;
        let number = s(&identity, "number");
        if estimate["current"]["customer_id"] != d["customer_id"] {
            return Err(err(format!("Estimate {number} belongs to another customer. Link an estimate of this customer, or none.")));
        }
        let state = s(&estimate["current"], "state");
        if ["DECLINED", "CANCELLED"].contains(&state) {
            return Err(err(format!("Estimate {number} is {}. Link an open estimate, or none.", state.to_ascii_lowercase())));
        }
        number.to_owned()
    } else { String::new() };
    let customer = directory::get(tx, c, Model::Customer, s(d, "customer_id"))
        .map_err(|_| err("Choose an active customer in this company."))?;
    if customer["active"] != true {
        return Err(err("Choose an active customer in this company."));
    }
    let setup = documents::setup_snapshot(tx, c)?;
    let liability = setup["customer_deposits"]
        .as_str()
        .ok_or_else(|| err(SETUP_MESSAGE))?;
    let dest = if d["destination"] == "BANK" {
        d["bank_id"].as_str()
    } else {
        setup["undeposited"].as_str()
    }
    .ok_or_else(|| err("Configure an explicit undeposited funds destination first."))?;
    let a = account(tx, c, dest)?;
    let l = account(tx, c, liability)?;
    if a.kind.as_str() != "ASSET" || !a.active || !a.postable {
        return Err(err(
            "The deposit destination must be an active postable ASSET account (undeposited funds or a designated bank).",
        ));
    }
    if l.kind.as_str() != "LIABILITY"
        || l.detail_type.as_str() != "OTHER_CURRENT_LIABILITY"
        || !l.active
        || !l.postable
    {
        return Err(err(SETUP_MESSAGE));
    }
    documents::lock_configuration(
        tx,
        actor,
        c,
        if d["destination"] == "BANK" {
            &["customer_deposits"]
        } else {
            &["customer_deposits", "undeposited"]
        },
        d["bank_id"].as_str().and_then(AccountId::parse),
        &[],
    )?;
    let mapping = json!({"destination":dest,"customer_deposits":liability});
    let display = display(tx, c, &mapping)?;
    let party = party(&customer);
    let company = company_snapshot(tx, c)?;
    let digest = digest(
        &json!({"company":c.to_db(),"intent":d,"mapping":display,"party":party,"company_snapshot":company,"estimate":estimate_number}),
    );
    Ok(
        json!({"mapping":mapping,"display":display,"entries":entries(dest,liability,&d["amount"]),"customer_snapshot":party,"company_snapshot":company,"digest":digest,"estimate_number":estimate_number}),
    )
}
pub fn review_deposit(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    payload: &Value,
) -> FbResult<Review> {
    command(book, |tx| {
        services::access(tx, actor, c)?;
        let d = normalize(payload)?;
        let plan = prepare(tx, actor, c, &d)?;
        Ok(Review {
            domain: "deposit",
            digest: s(&plan, "digest").into(),
            display: json!({"method_label":match s(&d,"method"){"CHECK"=>"Check","CASH"=>"Cash","CARD"=>"Card (record only)","TRANSFER"=>"Transfer (record only)",_=>"Other"},"data":d,"mapping":plan["display"],"entries":plan["entries"],"customer_snapshot":plan["customer_snapshot"],"company_snapshot":plan["company_snapshot"],"estimate_number":plan["estimate_number"]}),
        })
    })
}
pub fn record_deposit(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    review: Option<&Review>,
    payload: &Value,
) -> FbResult<Value> {
    command(book, |tx| {
        let u = services::access(tx, actor, c)?;
        let d = normalize(payload)?;
        let creation = digest(&d);
        let reviewed = token(review, "deposit", "customer deposit")?;
        if let Some(raw)=tx.query_row("SELECT r.data FROM sales_receipt r JOIN documents_documentidentity i ON i.id=r.identity_id WHERE r.company_id=? AND i.request_key=?",params![c.to_db(),s(&d,"request_key")],|r|r.get::<_,String>(0)).optional()? {
            let found:Value=fb_db::parse_unique_json_as(&raw).map_err(|_|err("Invalid stored receipt."))?;
            if found["kind"]!="CDP" || found["creation_digest"]!=creation || found["review_digest"]!=reviewed {return Err(err("This request key was recorded with a different payload or review."));}
            return Ok(found);
        }
        let identity = documents::create_identity(tx, actor, c, "CDP", s(&d, "request_key"))?;
        let plan = prepare(tx, actor, c, &d)?;
        if reviewed != s(&plan, "digest") {
            return Err(err(
                "Customer deposit or posting mapping changed. Open a fresh review before recording.",
            ));
        }
        documents::snapshot_source_accounts(tx, actor, c, s(&identity, "id"), &plan["mapping"])?;
        let mut text = format!(
            "customer deposit from {}",
            s(&plan["customer_snapshot"], "display_name")
        );
        if !s(&d, "memo").is_empty() {
            text += &format!(" · {}", s(&d, "memo"));
        }
        let lines = plan["entries"]
            .as_array()
            .cloned()
            .ok_or_else(|| err("Invalid posting plan."))?;
        let j = documents::post_source(
            tx,
            actor,
            c,
            s(&identity, "id"),
            s(&d, "date"),
            &summary(s(&identity, "number"), &text),
            lines,
        )?;
        let r = json!({"id":new_id(),"company_id":c.to_db(),"identity_id":identity["id"],"kind":"CDP","creation_digest":creation,"review_digest":reviewed,"customer_id":d["customer_id"],"date":d["date"],"amount":d["amount"],"method":d["method"],"reference":d["reference"],"memo":d["memo"],"destination":d["destination"],"customer_snapshot":plan["customer_snapshot"],"company_snapshot":plan["company_snapshot"],"destination_account_id":plan["mapping"]["destination"],"journal_id":j.id.to_db(),"actor_id":actor.to_db(),"actor_name":u.username});
        persist(
            tx,
            "sales_receipt",
            &r,
            &[
                "id",
                "company_id",
                "identity_id",
                "kind",
                "customer_id",
                "date",
                "amount",
                "destination",
                "destination_account_id",
                "journal_id",
                "data",
                "actor_id",
            ],
        )?;
        tx.execute(
            "INSERT INTO prepayments_customerdeposit VALUES(?,?,?,?,?,?)",
            params![
                new_id(),
                s(&r, "id"),
                c.to_db(),
                s(&plan["mapping"], "customer_deposits"),
                d["estimate_id"].as_str(),
                s(&d, "order_reference")
            ],
        )?;
        sales_seal(tx, "sales_receipt", s(&r, "id"))?;
        seal(tx, &j.id.to_db())?;
        services::audit(
            tx,
            &u,
            c,
            "customer_deposit.recorded",
            format!("receipt:{}", s(&r, "id")),
            json!({"number":identity["number"],"amount":r["amount"],"memo":r["memo"],"destination":r["destination"],"destination_account_id":r["destination_account_id"],"liability_id":plan["mapping"]["customer_deposits"],"estimate":plan["estimate_number"],"order_reference":d["order_reference"]}),
        )?;
        Ok(r)
    })
}
pub fn void_deposit(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    receipt_id: &str,
    expected: Option<i64>,
    dt: &Value,
    reason: &Value,
) -> FbResult<Value> {
    command(book, |tx| {
        let u = services::access(tx, actor, c)?;
        let r = deposit(tx, c, receipt_id)?;
        let dt = date(dt)?;
        let reason = text(reason, 500, true)?;
        if let Some(v) = load(tx, "sales_receiptreversal", "receipt_id", receipt_id)? {
            if expected == Some(1) && v["date"] == dt && v["reason"] == reason {
                return Ok(v);
            }
            return Err(err(
                "Customer deposit already voided with a different or stale command.",
            ));
        }
        documents::check_revision(1, expected)?;
        if dt.as_str() < s(&r, "date") {
            return Err(err("Void date cannot precede the deposit date."));
        }
        let app:Option<String>=tx.query_row("SELECT i.number FROM prepayments_depositapplication a JOIN sales_invoice v ON v.id=a.invoice_id JOIN documents_documentidentity i ON i.id=v.identity_id WHERE a.deposit_id=? AND NOT EXISTS(SELECT 1 FROM prepayments_depositunapplication u WHERE u.application_id=a.id) ORDER BY a.id LIMIT 1",[receipt_id],|r|r.get(0)).optional()?;
        if let Some(n) = app {
            return Err(err(format!(
                "Unapply this deposit from invoice {n} before voiding it."
            )));
        }
        let refund:Option<String>=tx.query_row("SELECT i.number FROM prepayments_depositrefund r JOIN documents_documentidentity i ON i.id=r.identity_id WHERE r.deposit_id=? AND NOT EXISTS(SELECT 1 FROM prepayments_depositrefundvoid v WHERE v.refund_id=r.id) ORDER BY r.id LIMIT 1",[receipt_id],|r|r.get(0)).optional()?;
        if let Some(n) = refund {
            return Err(err(format!(
                "Void refund {n} before voiding this customer deposit."
            )));
        }
        let release: Option<String> = tx.query_row(
            "SELECT max(date) FROM prepayments_releases WHERE deposit_id=?",
            [receipt_id],
            |r| r.get(0),
        )?;
        if let Some(d) = release.filter(|d| d > &dt) {
            return Err(err(format!(
                "Void date cannot precede {d}, when this deposit was last released by an unapply or refund void."
            )));
        }
        refuse_banked(tx, receipt_id, &dt)?;
        let j = documents::reverse_source(tx, actor, c, s(&r, "identity_id"), &dt, &reason)?;
        let mut v = json!({"id":new_id(),"receipt_id":receipt_id,"journal_id":j.id.to_db(),"date":dt,"reason":reason,"actor_id":actor.to_db(),"actor_name":u.username});
        persist(
            tx,
            "sales_receiptreversal",
            &v,
            &["id", "receipt_id", "journal_id", "date", "data", "actor_id"],
        )?;
        v.as_object_mut()
            .ok_or_else(|| err("Invalid reversal."))?
            .remove("actor_id");
        sales_seal(tx, "sales_receipt_reversal", s(&v, "id"))?;
        seal(tx, &j.id.to_db())?;
        services::audit(
            tx,
            &u,
            c,
            "customer_deposit.voided",
            format!("receipt:{receipt_id}"),
            json!({"reason":reason,"date":dt,"journal_id":j.id.to_db()}),
        )?;
        Ok(v)
    })
}
fn refuse_banked(q: &(impl Query + ?Sized), receipt: &str, date: &str) -> FbResult<()> {
    let company: String = q.query_row("SELECT company_id FROM sales_receipt WHERE id=?", [receipt], |r| r.get(0))?;
    let company = CompanyId::parse(&company).ok_or_else(|| err("Invalid stored company."))?;
    crate::banking::refuse_live_deposit(q, company, receipt, date)
}
