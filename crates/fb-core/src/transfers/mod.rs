//! Record-only transfers between two designated bank accounts.
use crate::checks::support::*;
use crate::{AccountId, Book, CompanyId, FbResult, Money, UserId, documents, py, services};
use fb_db::{Query, Writer};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub struct Review {
    digest: String,
    pub display: Value,
}
struct Plan {
    mapping: Value,
    entries: Vec<Value>,
    company: Value,
    review: Review,
}
fn normalize(p: &Value) -> FbResult<Value> {
    let key = py::py_uuid(s(p, "request_key"))
        .ok_or_else(|| err("Supply a stable UUID request key."))?
        .0;
    let dt = date(&p["date"])?;
    let amount = cents(&p["amount"])?;
    if amount <= 0 {
        return Err(err("Enter a transfer amount greater than zero."));
    }
    let from = id(&p["from_account_id"], false)?.unwrap();
    let to = id(&p["to_account_id"], false)?.unwrap();
    let memo = crate::card_numbers::card_free(field_text(p, "memo", 2000)?, "Memo")?;
    if from == to {
        return Err(err("Transfer from and to two different bank accounts."));
    }
    Ok(
        json!({"request_key":key,"date":dt,"amount":amount,"from_account_id":from,"to_account_id":to,"memo":memo}),
    )
}
fn prepare(tx: &Writer<'_>, actor: UserId, co: CompanyId, d: &Value) -> FbResult<Plan> {
    let from = account(tx, co, s(d, "from_account_id"))
        .map_err(|_| err("Choose two bank accounts from this company."))?;
    let to = account(tx, co, s(d, "to_account_id"))
        .map_err(|_| err("Choose two bank accounts from this company."))?;
    for a in [&from, &to] {
        if let Some(role) = documents::control_role(tx, co, a.id)? {
            return Err(err(&(documents::control_message(a, &role)
                + " Transfers move money between designated banks only.")));
        }
    }
    let setup = documents::setup_snapshot(tx, co)?;
    for a in [&from, &to] {
        if a.kind.as_str() != "ASSET"
            || !a.active
            || !a.postable
            || !setup["banks"]
                .as_array()
                .is_some_and(|v| v.contains(&json!(a.id.to_db())))
        {
            return Err(err(&format!(
                "{} · {} is not an active designated bank account (Accounting setup). Transfers move money between two designated banks.",
                a.code, a.name
            )));
        }
    }
    documents::lock_configuration(tx, actor, co, &[], Some(from.id), &[to.id])?;
    let mapping = json!({"from":from.id.to_db(),"to":to.id.to_db()});
    let display = display_mapping(tx, co, &mapping)?;
    let amount = Money::from_db(d["amount"].as_i64().unwrap()).to_string();
    let entries = vec![
        json!({"account":from.id.to_db(),"debit":"0.00","credit":amount}),
        json!({"account":to.id.to_db(),"debit":amount,"credit":"0.00"}),
    ];
    let company = company_snap(tx, co)?;
    let digest = digest(
        &json!({"company":co.to_db(),"intent":d,"mapping":display,"entries":entries,"company_snapshot":company}),
    );
    let balances = json!({"from":balance(tx,from.id)?,"to":balance(tx,to.id)?});
    Ok(Plan {
        mapping,
        entries: entries.clone(),
        company,
        review: Review {
            digest,
            display: json!({"mapping":display,"entries":entries,"balances":balances}),
        },
    })
}
fn balance(c: &(impl Query + ?Sized), aid: AccountId) -> FbResult<String> {
    let mut n = 0i128;
    let mut st=c.prepare("SELECT l.debit,l.credit FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id WHERE l.account_id=? AND j.status='POSTED'")?;
    for row in st.query_map([aid.to_db()], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
    })? {
        let (d, c) = row?;
        n = n
            .checked_add(d as i128 - c as i128)
            .ok_or_else(|| err("Balance exceeds supported bounds."))?;
    }
    Ok(format!(
        "{}{}.{:02}",
        if n < 0 { "-" } else { "" },
        n.unsigned_abs() / 100,
        n.unsigned_abs() % 100
    ))
}
pub fn posted_balance(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    aid: AccountId,
) -> FbResult<String> {
    book.read(|c| {
        services::access(c, actor, co)?;
        account(c, co, &aid.to_db())?;
        balance(c, aid)
    })
}
pub fn review_transfer(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    p: &Value,
) -> FbResult<Review> {
    write(book, |tx| review_transfer_tx(tx, actor, co, p))
}
/// [`review_transfer`] inside a caller's writer (bank-feed acceptance composes review, record and decision).
pub fn review_transfer_tx(tx: &Writer<'_>, actor: UserId, co: CompanyId, p: &Value) -> FbResult<Review> {
    tx.atomic(|| {
        services::access(tx, actor, co)?;
        Ok(prepare(tx, actor, co, &normalize(p)?)?.review)
    })
}
pub fn record_transfer(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    p: &Value,
    review: &Review,
) -> FbResult<Value> {
    write(book, |tx| record_transfer_tx(tx, actor, co, p, review))
}
/// Atomic record primitive; only an active writer can be supplied.
/// ```compile_fail
/// use fb_core::{transfers, Book, CompanyId, UserId};
/// fn cannot_write(book: &mut Book, actor: UserId, company: CompanyId, review: &transfers::Review) {
///     book.read(|r| transfers::record_transfer_tx(r, actor, company, &serde_json::json!({}), review));
/// }
/// ```
pub fn record_transfer_tx(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    p: &Value,
    review: &Review,
) -> FbResult<Value> {
    tx.atomic(||{
        let u=services::access(tx,actor,co)?;let d=normalize(p)?;let creation=digest(&d);
        if let Some(v)=rows(tx,"SELECT t.* FROM transfers_transfer t JOIN documents_documentidentity i ON i.id=t.identity_id WHERE t.company_id=? AND i.request_key=?",&[&co.to_db(),&s(&d,"request_key")])?.pop(){
            if v["creation_digest"]!=creation||v["review_digest"]!=review.digest{return Err(err("This request key was recorded with a different transfer or review. Start a new transfer instead."));}
            return Ok(v);
        }
        let identity=documents::create_identity(tx,actor,co,"TRF",s(&d,"request_key"))?;let plan=prepare(tx,actor,co,&d)?;
        if plan.review.digest!=review.digest{return Err(err("The transfer or its accounts changed. Open a fresh review before recording."));}
        documents::snapshot_source_accounts(tx,actor,co,s(&identity,"id"),&plan.mapping)?;
        let memo=if s(&d,"memo").is_empty(){"Funds transfer"}else{s(&d,"memo")};
        let j=documents::post_source(tx,actor,co,s(&identity,"id"),s(&d,"date"),&py::py_truncate(&format!("{}: {memo}",s(&identity,"number")),240),plan.entries)?;
        let r=json!({"id":fb_ids::new_v7().to_string(),"company_id":co.to_db(),"identity_id":identity["id"],"creation_digest":creation,"review_digest":review.digest,"from_account_id":d["from_account_id"],"to_account_id":d["to_account_id"],"from_snapshot":asnap(&account(tx,co,s(&d,"from_account_id"))?),"to_snapshot":asnap(&account(tx,co,s(&d,"to_account_id"))?),"company_snapshot":plan.company,"date":d["date"],"amount":d["amount"],"memo":d["memo"],"journal_id":j.id.to_db(),"actor_id":actor.to_db(),"actor_name":u.username,"created_at":py::now_utc()});
        insert(tx,"transfers_transfer",&r)?;seal_journal(tx,&j.id.to_db())?;
        services::audit(tx,&u,co,"transfer.recorded",format!("transfer:{}",s(&r,"id")),json!({"number":identity["number"],"date":d["date"],"amount":Money::from_db(d["amount"].as_i64().unwrap()).to_string(),"memo":d["memo"],"from_account_id":d["from_account_id"],"to_account_id":d["to_account_id"],"journal_id":j.id.to_db()}))?;Ok(r)
    })
}
pub fn void_transfer(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    tid: &str,
    expected: Option<i64>,
    dt: &Value,
    reason: &Value,
) -> FbResult<Value> {
    write(book, |tx| {
        let u = services::access(tx, actor, co)?;
        let t = rows(
            tx,
            "SELECT * FROM transfers_transfer WHERE company_id=? AND id=?",
            &[&co.to_db(), &tid],
        )?
        .pop()
        .ok_or_else(|| crate::FbError::missing("Transfer"))?;
        let dt = date(dt)?;
        let reason = text(reason, 500, true)?;
        if let Some(v) = rows(
            tx,
            "SELECT * FROM transfers_transfervoid WHERE transfer_id=?",
            &[&tid],
        )?
        .pop()
        {
            if expected == Some(1) && v["date"] == dt && v["reason"] == reason {
                return Ok(v);
            }
            return Err(err(
                "This transfer is already voided (with a different date or reason, or from a stale page).",
            ));
        }
        documents::check_revision(1, expected)?;
        if dt.as_str() < s(&t, "date") {
            return Err(err("The void date cannot precede the transfer date."));
        }
        let j = documents::reverse_source(tx, actor, co, s(&t, "identity_id"), &dt, &reason)?;
        let v = json!({"id":fb_ids::new_v7().to_string(),"transfer_id":tid,"journal_id":j.id.to_db(),"date":dt,"reason":reason,"actor_id":actor.to_db(),"actor_name":u.username,"created_at":py::now_utc()});
        insert(tx, "transfers_transfervoid", &v)?;
        seal_journal(tx, &j.id.to_db())?;
        let identity = documents::identity(tx, co, s(&t, "identity_id"))?;
        services::audit(
            tx,
            &u,
            co,
            "transfer.voided",
            format!("transfer:{tid}"),
            json!({"number":identity["number"],"reason":reason,"date":dt,"journal_id":j.id.to_db()}),
        )?;
        Ok(v)
    })
}
fn seal_journal(tx: &Writer<'_>, jid: &str) -> FbResult<()> {
    tx.execute("INSERT INTO fb_seal VALUES('transfer_journal',?)", [jid])?;
    Ok(())
}
