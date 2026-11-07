//! `accept` / `ignore` / `return_to_review`. Acceptance goes through the public check, deposit,
//! transfer and ledger services, which keep their own validation, numbering and audit.
use super::*;
use crate::services::DraftInput;
use crate::{banking, checks, transfers};

/// `REQUEST_NAMESPACE = uuid5(NAMESPACE_URL, 'https://freedombooks.invalid/bankfeeds')`.
pub(crate) fn request_key(kind: &str, item: &Item, intent: &str) -> String {
    let ns = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, b"https://freedombooks.invalid/bankfeeds");
    uuid::Uuid::new_v5(&ns, format!("{kind}:{}:{}:{intent}", item.ordinal, item.return_count).as_bytes()).to_string()
}
/// `_memo`.
fn memo(item: &Item) -> String {
    memo_text(&item.description)
}
/// `_memo` of a description.
pub(crate) fn memo_text(description: &str) -> String {
    if description.is_empty() { "Bank feed".into() } else { text::sanitize(&format!("Bank feed: {description}"), 240) }
}
/// `default_check_number`: `EFT-<item number>`, or the number in a 'CHECK 1234' description.
pub fn default_check_number(item: &Item) -> String {
    text::check_number_in(&item.description).unwrap_or_else(|| format!("EFT-{}", item.ordinal))
}

/// `_validated_item`: feed None for an already-decided item, which the caller hands back unchanged.
/// Messages quoting provider text are masked like written text ([`Writes::text`]).
fn validated(w: &Writes<'_, '_>, co: CompanyId, transaction_id: &str) -> FbResult<(Item, Option<Feed>)> {
    let q = w.tx;
    let item = item(q, co, transaction_id)?;
    if item.status == "ACCEPTED" || item.status == "MATCHED" {
        return Ok((item, None));
    }
    if item.status == "IGNORED" {
        return Err(err("This transaction is ignored. Return it to review first."));
    }
    if item.pending {
        return Err(err("This transaction is still pending at the bank. Sync again once it has posted."));
    }
    if item.date().is_none() {
        return Err(err("This transaction carries no date."));
    }
    let f = feed(q, &item.feed_account_id)?.ok_or_else(|| err("Choose a downloaded transaction of this company."))?;
    if !f.active || f.ledger_account_id.is_none() {
        return Err(err(w.text(&format!("Map {} to a ledger account before accepting its transactions.", f.label()))));
    }
    let company = store::get_company(q, co)?;
    if f.currency.to_uppercase() != company.currency {
        return Err(err(w.text(&format!("{} is in {}, not {}.", f.label(), text::sanitize(&f.currency, 40), company.currency))));
    }
    Ok((item, Some(f)))
}
fn field<'v>(input: &'v Value, key: &str) -> &'v Value {
    input.get(key).unwrap_or(&Value::Null)
}
fn opt_text(input: &Value, key: &str, max: usize) -> FbResult<String> {
    text(input.get(key).unwrap_or(&json!("")), max, false)
}

/// `_bank_line`: the mapped account's line in the posted entry.
fn bank_line(q: &(impl Query + ?Sized), journal_id: &str, account_id: &str, debit: i64, credit: i64) -> FbResult<String> {
    q.query_row(
        "SELECT id FROM ledger_journalline WHERE journal_id=?1 AND account_id=?2 AND debit=?3 AND credit=?4 ORDER BY id LIMIT 1",
        params![journal_id, account_id, debit, credit],
        |r| r.get(0),
    )
    .optional()?
    .ok_or_else(|| err("The posted entry does not carry the expected bank line."))
}
/// `_reference`: the source document number, else the entry number.
fn reference(q: &(impl Query + ?Sized), journal_id: &str) -> FbResult<String> {
    let (source, entry): (Option<String>, String) =
        q.query_row("SELECT source_id,entry_number FROM ledger_journal WHERE id=?1", [journal_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if let Some(source) = source {
        return Ok(q.query_row("SELECT number FROM documents_documentidentity WHERE id=?1", [source], |r| r.get(0))?);
    }
    Ok(py::py_strip(&format!("JE {entry}")).to_owned())
}
fn identity_number(q: &(impl Query + ?Sized), identity: &str) -> FbResult<String> {
    Ok(q.query_row("SELECT number FROM documents_documentidentity WHERE id=?1", [identity], |r| r.get(0))?)
}

struct Decision<'a> {
    status: &'a str,
    kind: &'a str,
    journal_id: &'a str,
    line_id: &'a str,
    reference: &'a str,
    url: String,
}
/// `_decide`.
fn decide(w: &Writes<'_, '_>, actor: UserId, co: CompanyId, item: &Item, d: Decision<'_>) -> FbResult<Item> {
    let tx = w.tx;
    let user = services::access(tx, actor, co)?;
    let reference = py::py_truncate(d.reference, 60);
    w.execute(
        "UPDATE bankfeeds_transaction SET status=?2,accepted_as=?3,journal_id=?4,journal_line_id=?5,reference=?6,reference_url=?7,decided_at=?8,decided_by_id=?9,updated_at=?8 WHERE id=?1",
        params![item.id, d.status, d.kind, d.journal_id, d.line_id, reference, py::py_truncate(&d.url, 200), py::now_utc(), actor.to_db()],
    )?;
    w.audit(
        &user,
        co,
        "bankfeed.accepted",
        format!("bankfeedtransaction:{}", item.id),
        json!({"kind":d.kind,"status":d.status,"journal_id":d.journal_id,"reference":reference,"amount":money(item.amount),"date":item.date(),"description":text::strip_url_userinfo(&item.description)}),
    )?;
    super::item(tx, co, &item.id)
}
/// `accept`: turn one downloaded transaction into a posted document through the existing services, or
/// match it to an existing posted entry. Accepting an already-decided item returns it unchanged.
/// `input` keys follow Python's keyword arguments: kind, account_id, payee_name, vendor_id, check_number,
/// journal_line_id, receipt_ids, transfer_account_id.
///
/// Composition (Python's `accept` / `_finish`): a check draft commits first in its own transaction, as
/// Python's `_check_draft` does; then validation, the posting (through the modules' `_tx` forms), the
/// decision and its audit commit together in ONE writer, so a failure anywhere leaves no posted
/// document behind an item still to review. A retry reuses the draft through the item's request key.
pub fn accept(book: &mut Book, actor: UserId, co: CompanyId, transaction_id: &str, input: &Value, secrets: &SecretSet) -> FbResult<Item> {
    let kind = field(input, "kind").as_str().unwrap_or("").to_owned();
    let mut draft = None;
    if kind == "CHECK" {
        match check_draft(book, actor, co, transaction_id, input, secrets)? {
            Err(decided) => return Ok(decided),
            Ok(check) => draft = Some(check["id"].as_str().unwrap_or("").to_owned()),
        }
    }
    command(book, secrets, |w| {
        let tx = w.tx;
        services::access(tx, actor, co)?;
        let (item, f) = validated(w, co, transaction_id)?;
        let Some(f) = f else { return Ok(item) };
        if !KINDS.contains(&kind.as_str()) {
            return Err(err("Choose what to record this transaction as."));
        }
        finish(w, actor, co, &item, &f, &kind, input, draft.as_deref())
    })
}

/// `_finish` after validation: record through the owning module's `_tx` service, then decide.
#[allow(clippy::too_many_arguments)]
/// The payloads handed to the other modules' services are masked like written text ([`Writes::value`]):
/// those services write and audit them.
fn finish(w: &Writes<'_, '_>, actor: UserId, co: CompanyId, item: &Item, f: &Feed, kind: &str, input: &Value, draft: Option<&str>) -> FbResult<Item> {
    let tx = w.tx;
    let mapped = f.ledger_account_id.clone().unwrap_or_default();
    let (debit, credit) = direction(item)?;
    let amount = magnitude(item)?;
    let date = item.date().unwrap_or("").to_owned();
    match kind {
        "MATCH" => {
            let line = id(field(input, "journal_line_id"), false)?.unwrap_or_default();
            let found = match_candidates(tx, co, item, 50)?.into_iter().find(|c| c.line_id == line).ok_or_else(|| {
                err(format!("Choose an unmatched posted entry on the mapped account with the same amount and a date within {MATCH_WINDOW_DAYS} days."))
            })?;
            let r = reference(tx, &found.journal_id)?;
            let url = format!("journal:{}", found.journal_id);
            decide(w, actor, co, item, Decision { status: "MATCHED", kind: "MATCH", journal_id: &found.journal_id, line_id: &line, reference: &r, url })
        }
        "CHECK" => {
            let cid = draft.ok_or_else(|| err("Choose what to record this transaction as."))?;
            let mut b = checks::get_check_tx(tx, actor, co, cid)?;
            if b["current"]["state"] == "VOID" {
                return Err(err(format!(
                    "{} recorded from this transaction was voided. Return the transaction to review before accepting it again.",
                    b["number"].as_str().unwrap_or("")
                )));
            }
            if b["current"]["state"] == "DRAFT" {
                let revision = b["revision"].as_i64();
                let review = checks::review_check_tx(tx, actor, co, cid, revision)?;
                b = checks::record_check_tx(tx, actor, co, cid, revision, &review)?;
            }
            let journal = b["current"]["journal_id"].as_str().unwrap_or("").to_owned();
            let number = b["number"].as_str().unwrap_or("").to_owned();
            let line = bank_line(tx, &journal, &mapped, debit, credit)?;
            decide(w, actor, co, item, Decision { status: "ACCEPTED", kind: "CHECK", journal_id: &journal, line_id: &line, reference: &number, url: format!("checks:detail:{cid}") })
        }
        "DEPOSIT" => {
            if !item.money_in() {
                return Err(err("Money going out cannot be recorded as a deposit."));
            }
            let ids = match field(input, "receipt_ids") {
                Value::Null => vec![],
                Value::Array(a) => a.iter().map(|v| id(v, false).map(Option::unwrap_or_default)).collect::<FbResult<Vec<_>>>()?,
                Value::String(s) if s.is_empty() => vec![],
                _ => return Err(err("Choose a record from this company.")),
            };
            if ids.is_empty() {
                return Err(err("Choose the undeposited receipt(s) this deposit covers."));
            }
            let payload = w.value(&json!({"request_key":request_key("DEPOSIT", item, ""),"bank_id":mapped,"date":date,"memo":memo(item),"receipt_ids":ids}));
            let review = banking::review_deposit_tx(tx, actor, co, &payload)?;
            let total = review.display["amount"].as_str().unwrap_or("").to_owned();
            if total != money(amount) {
                return Err(err(format!("The selected receipts total {total}, not the bank's {}.", money(amount))));
            }
            let dep = banking::record_deposit_tx(tx, actor, co, &payload, &review)?;
            let text_of = |k: &str| dep[k].as_str().unwrap_or("").to_owned();
            let (journal, dep_id, identity) = (text_of("journal_id"), text_of("id"), text_of("identity_id"));
            let line = bank_line(tx, &journal, &mapped, debit, credit)?;
            let number = identity_number(tx, &identity)?;
            decide(w, actor, co, item, Decision { status: "ACCEPTED", kind: "DEPOSIT", journal_id: &journal, line_id: &line, reference: &number, url: format!("deposits:detail:{dep_id}") })
        }
        "TRANSFER" => {
            let other = id(field(input, "transfer_account_id"), false)?.unwrap_or_default();
            let (from, to) = if item.money_in() { (other, mapped.clone()) } else { (mapped.clone(), other) };
            let payload = w.value(&json!({"request_key":request_key("TRANSFER", item, ""),"date":date,"amount":money(amount),"from_account_id":from,"to_account_id":to,"memo":memo(item)}));
            let review = transfers::review_transfer_tx(tx, actor, co, &payload)?;
            let trf = transfers::record_transfer_tx(tx, actor, co, &payload, &review)?;
            let journal = trf["journal_id"].as_str().unwrap_or("").to_owned();
            let line = bank_line(tx, &journal, &mapped, debit, credit)?;
            let number = identity_number(tx, trf["identity_id"].as_str().unwrap_or(""))?;
            let url = format!("transfers:detail:{}", trf["id"].as_str().unwrap_or(""));
            decide(w, actor, co, item, Decision { status: "ACCEPTED", kind: "TRANSFER", journal_id: &journal, line_id: &line, reference: &number, url })
        }
        _ => {
            let category = id(field(input, "account_id"), false)?.unwrap_or_default();
            if category == mapped {
                return Err(err("Choose a category other than the feed account itself."));
            }
            let name = opt_text(input, "payee_name", 120)?;
            let description = text::sanitize(&item.description, 160);
            let lines = vec![
                w.value(&json!({"account":mapped,"debit":money(debit),"credit":money(credit),"description":description,"name":name})),
                w.value(&json!({"account":category,"debit":money(credit),"credit":money(debit),"description":description,"name":name})),
            ];
            let mut draft = DraftInput::new(&date, &w.text(&memo(item)), lines);
            draft.request_key = Some(request_key("JOURNAL", item, &format!("{category}:{name}")));
            let mut journal = services::save_draft_tx(tx, actor, co, &draft, None)?;
            if journal.status.as_str() == "DRAFT" {
                journal = services::post_journal_tx(tx, actor, co, journal.id, Some(journal.revision))?;
            }
            let jid = journal.id.to_db();
            let reversed: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM ledger_journal WHERE reversal_of_id=?1)", [&jid], |r| r.get(0))?;
            if reversed {
                return Err(err(format!(
                    "Journal entry {} recorded from this transaction was reversed. Return the transaction to review before accepting it again.",
                    journal.entry_number
                )));
            }
            let line = bank_line(tx, &jid, &mapped, debit, credit)?;
            let r = py::py_strip(&format!("JE {}", journal.entry_number)).to_owned();
            decide(w, actor, co, item, Decision { status: "ACCEPTED", kind: "JOURNAL", journal_id: &jid, line_id: &line, reference: &r, url: format!("journal:{jid}") })
        }
    }
}

/// `_check_draft`: Err(item) when the item is already decided.
fn check_draft(book: &mut Book, actor: UserId, co: CompanyId, transaction_id: &str, input: &Value, secrets: &SecretSet) -> FbResult<Result<Value, Item>> {
    command(book, secrets, |w| {
        let tx = w.tx;
        services::access(tx, actor, co)?;
        let (item, f) = validated(w, co, transaction_id)?;
        let Some(f) = f else { return Ok(Err(item)) };
        if item.money_in() {
            return Err(err("Money coming in cannot be recorded as a check."));
        }
        if !is_designated_bank(tx, co, f.ledger_account_id.as_deref())? {
            return Err(err("A check needs the feed mapped to a designated bank account (Accounting setup)."));
        }
        let vendor = id(field(input, "vendor_id"), true)?;
        let payee_kind = if vendor.is_some() { "VENDOR" } else { "OTHER" };
        let mut payee = opt_text(input, "payee_name", 120)?;
        if payee_kind == "OTHER" && payee.is_empty() {
            payee = text::sanitize(&item.description, 120);
            if payee.is_empty() {
                payee = "Bank feed payee".into();
            }
        }
        let number = opt_text(input, "check_number", 20)?;
        let number = if number.is_empty() { default_check_number(&item) } else { number };
        let category = id(field(input, "account_id"), false)?.unwrap_or_default();
        let intent = format!("{category}:{}:{payee}:{number}", vendor.clone().unwrap_or_else(|| "None".into()));
        let payload = w.value(&json!({"bank_id":f.ledger_account_id,"payee_kind":payee_kind,"vendor_id":vendor,"payee_name":payee,"date":item.date(),
            "check_number":number,"memo":memo(&item),"request_key":request_key("CHECK", &item, &intent),
            "lines":[{"tab":"EXPENSE","account_id":category,"description":text::sanitize(&item.description, 1000),"rate":money(magnitude(&item)?)}]}));
        Ok(Ok(checks::save_check_tx(tx, actor, co, &payload)?))
    })
}

/// `ignore`.
pub fn ignore(book: &mut Book, actor: UserId, co: CompanyId, transaction_id: &str, secrets: &SecretSet) -> FbResult<Item> {
    command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        let item = item(tx, co, transaction_id)?;
        if item.status == "IGNORED" {
            return Ok(item);
        }
        if item.status != "NEW" {
            return Err(err("Only a transaction still to review can be ignored."));
        }
        w.execute("UPDATE bankfeeds_transaction SET status='IGNORED',decided_at=?2,decided_by_id=?3,updated_at=?2 WHERE id=?1", params![item.id, py::now_utc(), actor.to_db()])?;
        w.audit(&user, co, "bankfeed.ignored", format!("bankfeedtransaction:{}", item.id), json!({"amount":money(item.amount),"date":item.date().unwrap_or("None"),"description":text::strip_url_userinfo(&item.description)}))?;
        super::item(tx, co, transaction_id)
    })
}

/// `return_to_review`: ignored or matched items come back at any time; an accepted item only once its
/// document was voided / reversed through the normal path.
pub fn return_to_review(book: &mut Book, actor: UserId, co: CompanyId, transaction_id: &str, secrets: &SecretSet) -> FbResult<Item> {
    command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        let item = item(tx, co, transaction_id)?;
        if item.status == "NEW" {
            return Ok(item);
        }
        if item.status == "ACCEPTED" {
            let reversed: bool =
                tx.query_row("SELECT EXISTS(SELECT 1 FROM ledger_journal WHERE reversal_of_id=?1)", [item.journal_id.as_deref().unwrap_or("")], |r| r.get(0))?;
            if !reversed {
                return Err(err(format!("{} is still posted. Void or reverse it from its own page first; then return this transaction to review.", item.reference)));
            }
        }
        let previous = json!({"status":item.status,"kind":item.accepted_as,"journal_id":item.journal_id,"reference":item.reference});
        let count = item.return_count.checked_add(1).ok_or_else(|| err("Too many returns."))?;
        w.execute(
            "UPDATE bankfeeds_transaction SET status='NEW',accepted_as='',journal_id=NULL,journal_line_id=NULL,reference='',reference_url='',decided_at=NULL,decided_by_id=NULL,return_count=?2,updated_at=?3 WHERE id=?1",
            params![item.id, count, py::now_utc()],
        )?;
        w.audit(&user, co, "bankfeed.returned", format!("bankfeedtransaction:{}", item.id), json!({"previous":previous,"amount":money(item.amount),"date":item.date().unwrap_or("None")}))?;
        super::item(tx, co, transaction_id)
    })
}
