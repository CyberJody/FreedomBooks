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
pub(super) fn validated(w: &Writes<'_, '_>, co: CompanyId, transaction_id: &str) -> FbResult<(Item, Option<Feed>)> {
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
    if let Some(label) = retired_connection(q, &item)? {
        return Err(err(w.text(&format!("This transaction came from {label}, a bank connection that was disconnected, so it cannot be recorded from here. Ignore it, or enter it by hand if the books need it."))));
    }
    if !f.active || f.ledger_account_id.is_none() {
        return Err(err(w.text(&format!("Map {} to a ledger account before accepting its transactions.", f.label()))));
    }
    let company = store::get_company(q, co)?;
    if f.currency.to_uppercase() != company.currency {
        return Err(err(w.text(&format!("{} is in {}, not {}.", f.label(), text::sanitize(&f.currency, 40), company.currency))));
    }
    Ok((item, Some(f)))
}
/// Compare the caller's reviewed state under the same writer as the posting.
fn require_reviewed_state(w: &Writes<'_, '_>, co: CompanyId, transaction_id: &str, input: &Value) -> FbResult<()> {
    let Some(expected) = input.get("preconditions") else { return Ok(()) };
    let changed = || err("Bank review precondition failed. Nothing was recorded. Check bank review again.");
    let row = w.tx.query_row(
        &format!("SELECT {ITEM_COLS},f.ledger_account_id,f.currency,c.host,f.remote_id,f.provider_id_verified,t.provider_id_verified FROM bankfeeds_transaction t
                  JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id
                  WHERE t.id=?1 AND c.company_id=?2"),
        params![transaction_id, co.to_db()],
        |r| Ok((item_row(r)?, r.get::<_, Option<String>>(17)?, r.get::<_, String>(18)?, r.get::<_, String>(19)?,
            r.get::<_, String>(20)?, r.get::<_, bool>(21)?, r.get::<_, bool>(22)?)),
    ).optional()?.ok_or_else(changed)?;
    let (item, mapped, currency, host, account_remote, account_verified, transaction_verified) = row;
    let actual = json!({"id":item.id,"status":item.status,"return_count":item.return_count,
        "amount_cents":item.amount.to_string(),"date":item.date(),"feed_account_id":item.feed_account_id,"mapped_account_id":mapped});
    if actual.as_object().expect("review fields").iter().any(|(key, value)| expected.get(key) != Some(value))
    {
        return Err(changed());
    }
    let identity = json!({"currency":currency,"provider_host":host,"provider_account_id":account_remote,
        "provider_transaction_id":item.remote_id,"description":item.description});
    if identity.as_object().expect("identity fields").iter().any(|(key, value)| {
        expected.get(key).is_some_and(|seen| !seen.is_null() && seen != value)
    }) || (expected["require_exact_account_id"] == true && !account_verified)
        || (expected["require_exact_transaction_id"] == true && !transaction_verified)
    {
        return Err(changed());
    }
    for (field, account_id) in [
        ("mapped_account_detail_type", mapped.as_deref()),
        ("category_detail_type", input["account_id"].as_str()),
    ] {
        let Some(detail_type) = expected[field].as_str() else { continue };
        let account_id = account_id.and_then(AccountId::parse).ok_or_else(changed)?;
        let account = store::account(w.tx, account_id)?.filter(|a| a.company_id == co && a.active && a.postable).ok_or_else(changed)?;
        if account.detail_type.as_str() != detail_type {
            return Err(changed());
        }
    }
    if expected["require_no_candidates"] == true && review::review_candidates(w.tx, co, &item, 1)?.any() {
        return Err(changed());
    }
    Ok(())
}

fn field<'v>(input: &'v Value, key: &str) -> &'v Value {
    input.get(key).unwrap_or(&Value::Null)
}
/// Worker review 3b M3: an account id in its standard form (lower-case, hyphenated), so that another spelling
/// of an account (upper case, braces) is compared, and refused, exactly like the account itself. The book
/// keeps every id in this form.
fn account_id(v: &Value, optional: bool) -> FbResult<Option<String>> {
    Ok(id(v, optional)?.map(|raw| py::record_uuid(&raw).map_or(raw, |c| c.0)))
}

/// Worker item 46: one line of a split row. `amount` is in cents and in the row's own direction (an expense
/// on a withdrawal, income on a deposit); a negative line goes the other way.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SplitLine {
    pub account: String,
    pub amount: i64,
    pub memo: String,
}
/// Most lines one split may carry.
pub const MAX_SPLIT_LINES: usize = 20;
/// Most posted ledger entries one bank transaction may match at once.
pub const MAX_MATCH_LINES: usize = 20;
/// A signed money amount typed on a split or fee line: at most two decimals, never zero.
pub(super) fn signed_cents(v: &Value, what: &str) -> FbResult<i64> {
    let raw = v.as_str().map(py::py_strip).unwrap_or("");
    let (negative, body) = match raw.strip_prefix('-') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, raw),
    };
    let amount = crate::calculations::unit_rate(&json!(body))
        .ok()
        .and_then(|d| i64::try_from(d.mantissa()).ok())
        .ok_or_else(|| err(format!("Enter an amount such as 19.99 on each {what}.")))?;
    if amount == 0 {
        return Err(err(format!("A {what} cannot be 0.00.")));
    }
    Ok(if negative { -amount } else { amount })
}
/// Worker item 46: the split lines of `input["lines"]` (`[{account_id, amount, memo}]`), or None for a row
/// recorded with one category. The lines must add up exactly to the row's amount; a check's lines must each
/// be more than zero (a check line cannot be negative).
pub(super) fn split_lines(input: &Value, item: &Item, mapped: &str, check: bool) -> FbResult<Option<Vec<SplitLine>>> {
    let rows = match field(input, "lines") {
        Value::Null => return Ok(None),
        Value::Array(a) if a.is_empty() => return Ok(None),
        Value::Array(a) => a,
        _ => return Err(err("Enter the split lines again.")),
    };
    if rows.len() > MAX_SPLIT_LINES {
        return Err(err(format!("A split has at most {MAX_SPLIT_LINES} lines.")));
    }
    let mut lines = vec![];
    let mut total = 0i64;
    for row in rows {
        let account = account_id(field(row, "account_id"), true)?.ok_or_else(|| err("Choose an account on each split line."))?;
        if account == mapped {
            return Err(err("Choose an account other than the feed account itself on each split line."));
        }
        let amount = signed_cents(field(row, "amount"), "split line")?;
        if check && amount < 0 {
            return Err(err("A check's split lines must each be more than 0.00. Record it as a journal entry to put a line the other way."));
        }
        total = total.checked_add(amount).filter(|n| n.unsigned_abs() <= 9_999_999_999_999_999).ok_or_else(|| err("Document amount exceeds 99999999999999.99."))?;
        lines.push(SplitLine { account, amount, memo: opt_text(row, "memo", 160)? });
    }
    let want = magnitude(item)?;
    if total != want {
        let gap = if total < want {
            format!("{} is left to assign", money(want - total))
        } else {
            format!("they are {} over", money(total - want))
        };
        return Err(err(format!("The split lines add up to {}, not the bank's {}: {gap}. Nothing was recorded.", money(total), money(want))));
    }
    Ok(Some(lines))
}
/// A short stable text of a split for the request key: a retry with the same lines reuses the same draft.
fn split_intent(lines: &[SplitLine]) -> String {
    lines.iter().map(|l| format!("{}={}={}", l.account, l.amount, l.memo)).collect::<Vec<_>>().join(";")
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
    matched_lines: Vec<(String, String)>,
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
    let mut detail = json!({"kind":d.kind,"status":d.status,"journal_id":d.journal_id,"reference":reference,"amount":money(item.amount),"date":item.date(),"description":text::strip_url_userinfo(&item.description)});
    if d.matched_lines.len() > 1 {
        detail["matched_journal_ids"] = json!(d.matched_lines.iter().map(|(j, _)| j).collect::<Vec<_>>());
        detail["matched_lines"] = json!(d.matched_lines.iter().map(|(j, l)| json!({"journal_id": j, "journal_line_id": l})).collect::<Vec<_>>());
    }
    w.audit(
        &user,
        co,
        "bankfeed.accepted",
        format!("bankfeedtransaction:{}", item.id),
        detail,
    )?;
    super::item(tx, co, &item.id)
}
/// `accept`: turn one downloaded transaction into a posted document through the existing services, or
/// match it to an existing posted entry. Accepting an already-decided item returns it unchanged.
/// `input` keys follow Python's keyword arguments: kind, account_id, payee_name, vendor_id, check_number,
/// journal_line_id, receipt_ids, transfer_account_id. Worker item 46: `lines` (`[{account_id, amount, memo}]`)
/// records a CHECK or JOURNAL row split over several accounts instead of one `account_id`. Worker items 47 and
/// 48: a DEPOSIT takes several `receipt_ids` and `adjustments` (`[{account_id, amount}]`, a fee negative); the
/// deposit must equal the row.
///
/// Composition (Python's `accept` / `_finish`): a check draft commits first in its own transaction, as
/// Python's `_check_draft` does. A guarded check keeps its draft and posting in the same writer.
/// Then validation, the posting (through the modules' `_tx` forms), the
/// decision and its audit commit together in ONE writer, so a failure anywhere leaves no posted
/// document behind an item still to review. A retry reuses the draft through the item's request key.
///
/// A transaction that is very likely already in the books ([`already_recorded`]) is refused unless `input`
/// carries `"record_again": true`, the user's explicit confirmation; a confirmed second recording is audited
/// (`bankfeed.recorded_again`, naming the earlier entry) in the same writer. Matching an existing entry
/// (`MATCH`) posts nothing and is never refused here. Worker item 40: so is a new entry for a transaction
/// whose mapped account already has a posted entry it could match ([`match_candidates`]), the same way.
pub fn accept(book: &mut Book, actor: UserId, co: CompanyId, transaction_id: &str, input: &Value, secrets: &SecretSet) -> FbResult<Item> {
    let kind = field(input, "kind").as_str().unwrap_or("").to_owned();
    let mut draft = None;
    if kind == "CHECK" && input.get("preconditions").is_none() {
        match command(book, secrets, |w| check_draft(w, actor, co, transaction_id, input))? {
            Err(decided) => return Ok(decided),
            Ok(check) => draft = Some(check["id"].as_str().unwrap_or("").to_owned()),
        }
    }
    command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        require_reviewed_state(w, co, transaction_id, input)?;
        let (item, f) = validated(w, co, transaction_id)?;
        let Some(f) = f else { return Ok(item) };
        if !KINDS.contains(&kind.as_str()) {
            return Err(err("Choose what to record this transaction as."));
        }
        if kind == "CHECK" && draft.is_none() {
            match check_draft(w, actor, co, transaction_id, input)? {
                Err(decided) => return Ok(decided),
                Ok(check) => draft = Some(check["id"].as_str().unwrap_or("").to_owned()),
            }
        }
        let earlier = if kind == "MATCH" { None } else { already_recorded(tx, co, &item)? };
        let entry = if kind == "MATCH" || earlier.is_some() { None } else { entry_on_account(tx, co, &item)? };
        let done = finish(w, actor, co, &item, &f, &kind, input, draft.as_deref())?;
        // After the input's own validation (its messages come first); refusing rolls the posting back.
        if let Some(earlier) = recorded_unless_confirmed(earlier, input)? {
            w.audit(
                &user,
                co,
                "bankfeed.recorded_again",
                format!("bankfeedtransaction:{}", item.id),
                json!({"journal_id":done.journal_id,"reference":done.reference,"earlier_transaction":earlier.earlier_id,"earlier_journal_id":earlier.journal_id,"earlier_reference":earlier.reference}),
            )?;
        }
        if let Some(entry) = entry_unless_confirmed(entry, item.date(), input)? {
            w.audit(
                &user,
                co,
                "bankfeed.recorded_again",
                format!("bankfeedtransaction:{}", item.id),
                // Worker review M2: the earlier entry by its number, as the earlier-download audit above names it.
                json!({"journal_id":done.journal_id,"reference":done.reference,"earlier_journal_id":entry.journal_id,"earlier_line_id":entry.line_id,"earlier_reference":entry.number,"earlier_date":entry.date,"earlier_memo":entry.memo()}),
            )?;
        }
        Ok(done)
    })
}

/// Refuse to post a transaction that is very likely already recorded unless the user confirmed recording it
/// again; returns the earlier entry when the confirmation is what lets it through. A confirmation that names
/// the earlier transaction the user was shown (`"recorded_as"`) holds only for that one: if the books changed
/// since the page was drawn, nothing is recorded and the row is to be reviewed again.
fn recorded_unless_confirmed(earlier: Option<Recorded>, input: &Value) -> FbResult<Option<Recorded>> {
    let Some(earlier) = earlier else { return Ok(None) };
    if field(input, "record_again") == &Value::Bool(true) {
        if let Some(seen) = field(input, "recorded_as").as_str()
            && seen != earlier.earlier_id
        {
            return Err(err(format!(
                "The books changed since this page was shown: this transaction now looks already recorded as {} ({}). Nothing was recorded; review it again.",
                earlier.reference, earlier.date
            )));
        }
        return Ok(Some(earlier));
    }
    if earlier.same_account {
        return Err(err(format!(
            "This transaction looks already recorded as {} ({}): the same date, amount and description on this bank account. Ignore it if it is the same transaction, or confirm that you want to record it again.",
            earlier.reference, earlier.date
        )));
    }
    Err(err(format!(
        "This transaction looks already recorded as {} ({}) from an earlier bank download. Link it to that entry, or confirm that you want to record it again.",
        earlier.reference, earlier.date
    )))
}

/// Worker item 40: the entry Match would offer ([`match_candidates`]' first: posted, unreversed, on this
/// transaction's mapped account with the same amount and direction, within [`MATCH_WINDOW_DAYS`], not
/// matched to another downloaded row). Typically a card payment recorded from the other feed: the checking
/// side's check to the card account is the card side's payment. Recording a new entry would post it twice.
pub fn entry_on_account(q: &(impl Query + ?Sized), co: CompanyId, item: &Item) -> FbResult<Option<MatchCandidate>> {
    if item.amount == 0 {
        return Ok(None);
    }
    Ok(match_candidates(q, co, item, 1)?.into_iter().next())
}
/// Refuse to post a new entry while [`entry_on_account`] found one, unless the user confirmed recording it
/// again (`"record_again": true`; with `"recorded_as"`, only for the entry line the page showed). Returns the
/// entry when the confirmation is what lets it through.
/// Worker review M2: the entry is named by its number and date, its memo after them ([`MatchCandidate::named`]).
fn entry_unless_confirmed(entry: Option<MatchCandidate>, item_date: Option<&str>, input: &Value) -> FbResult<Option<MatchCandidate>> {
    let Some(entry) = entry else { return Ok(None) };
    let named = entry.named();
    if field(input, "record_again") == &Value::Bool(true) {
        if let Some(seen) = field(input, "recorded_as").as_str()
            && seen != entry.line_id
        {
            return Err(err(format!(
                "The books changed since this page was shown: this transaction now looks already recorded as {named}. Nothing was recorded; review it again."
            )));
        }
        return Ok(Some(entry));
    }
    let within_default_window = item_date
        .and_then(|d| parse_date(d).ok())
        .zip(parse_date(&entry.date).ok())
        .is_none_or(|(bank, entry_day)| (bank - entry_day).num_days().abs() <= MATCH_WINDOW_DAYS);
    if within_default_window {
        Err(err(format!(
            "This transaction looks already recorded as {named}: an entry on this account with the same amount, {MATCH_WINDOW_DAYS} days or less apart. Match it to that entry, or confirm that you want to record it again."
        )))
    } else {
        Err(err(format!(
            "This transaction looks already recorded as {named}: a check on this account with the same amount. Match it to that entry, or confirm that you want to record it again."
        )))
    }
}

/// "Link to existing": mark a re-downloaded transaction handled by the entry [`already_recorded`] found,
/// without posting anything. The row becomes IGNORED with a link to the earlier feed row
/// (`bankfeeds_link`), audited as `bankfeed.linked`; returning it to review removes the link.
/// `expected` is the earlier transaction the screen showed: if the books changed since, nothing is linked.
pub fn link_existing(book: &mut Book, actor: UserId, co: CompanyId, transaction_id: &str, expected: Option<&str>, secrets: &SecretSet) -> FbResult<Item> {
    command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        let (item, f) = validated(w, co, transaction_id)?;
        if f.is_none() {
            return Err(err("This transaction is already decided. Return it to review first."));
        }
        let earlier = already_recorded(tx, co, &item)?
            .filter(|e| expected.is_none_or(|x| x == e.earlier_id))
            .ok_or_else(|| err("The books changed: no earlier entry matches this transaction now. Review it again."))?;
        if earlier.same_account {
            return Err(err(format!(
                "This looks like a second copy, on the same bank account, of {} ({}). Ignore it if it is the same transaction, or record it again.",
                earlier.reference, earlier.date
            )));
        }
        let now = py::now_utc();
        w.execute("UPDATE bankfeeds_transaction SET status='IGNORED',decided_at=?2,decided_by_id=?3,updated_at=?2 WHERE id=?1", params![item.id, now, actor.to_db()])?;
        w.execute("INSERT INTO bankfeeds_link(item_ordinal,earlier_ordinal) VALUES(?1,?2)", params![item.ordinal, earlier.earlier_ordinal])?;
        w.audit(
            &user,
            co,
            "bankfeed.linked",
            format!("bankfeedtransaction:{}", item.id),
            json!({"earlier_transaction":earlier.earlier_id,"journal_id":earlier.journal_id,"reference":earlier.reference,"same_identity":earlier.same_identity,
                "amount":money(item.amount),"date":item.date(),"description":text::strip_url_userinfo(&item.description)}),
        )?;
        super::item(tx, co, transaction_id)
    })
}

/// `_finish` after validation: record through the owning module's `_tx` service, then decide.
#[allow(clippy::too_many_arguments)]
/// The payloads handed to the other modules' services are masked like written text ([`Writes::value`]):
/// those services write and audit them.
pub(super) fn finish(w: &Writes<'_, '_>, actor: UserId, co: CompanyId, item: &Item, f: &Feed, kind: &str, input: &Value, draft: Option<&str>) -> FbResult<Item> {
    let tx = w.tx;
    let mapped = f.ledger_account_id.clone().unwrap_or_default();
    let (debit, credit) = direction(item)?;
    let amount = magnitude(item)?;
    let date = item.date().unwrap_or("").to_owned();
    if matches!(kind, "CHECK" | "DEPOSIT" | "TRANSFER" | "JOURNAL") {
        crate::periods::require_open(tx, co, &date)?;
    }
    match kind {
        "MATCH" => {
            let multi_ids: Option<Vec<String>> = match field(input, "journal_line_ids") {
                Value::Null => None,
                Value::Array(a) => Some(
                    a.iter()
                        .map(|v| id(v, false).map(Option::unwrap_or_default))
                        .collect::<FbResult<Vec<_>>>()?,
                ),
                _ => return Err(err("Choose the posted entries to match.")),
            };
            if let Some(lines) = multi_ids {
                if lines.is_empty() {
                    return Err(err("Choose at least one posted entry to match."));
                }
                if lines.len() > MAX_MATCH_LINES {
                    return Err(err(format!("A bank transaction matches at most {MAX_MATCH_LINES} posted entries.")));
                }
                let mut seen = std::collections::HashSet::new();
                for l in &lines {
                    if !seen.insert(l.as_str()) {
                        return Err(err("Choose each posted entry only once."));
                    }
                }
                if lines.len() == 1 {
                    let line = &lines[0];
                    let found = match_candidates(tx, co, item, 50)?.into_iter().find(|c| c.line_id == *line).ok_or_else(|| {
                        err(format!("Choose an unmatched posted entry on the mapped account with the same amount and a date within {MATCH_WINDOW_DAYS} days (or a matching check within {CHECK_MATCH_WINDOW_DAYS} days)."))
                    })?;
                    let r = reference(tx, &found.journal_id)?;
                    let url = format!("journal:{}", found.journal_id);
                    return decide(w, actor, co, item, Decision { status: "MATCHED", kind: "MATCH", journal_id: &found.journal_id, line_id: line, reference: &r, url, matched_lines: vec![] });
                }
                let eligible = multi_match_candidates(tx, co, item, MULTI_MATCH_POOL_LIMIT)?;
                let mut chosen = Vec::with_capacity(lines.len());
                let mut total = 0i64;
                for line in &lines {
                    let cand = eligible.iter().find(|c| c.line_id == *line).ok_or_else(|| {
                        err(format!("Choose unmatched posted entries on the mapped account in the same direction, dated within {MATCH_WINDOW_DAYS} days (or matching checks within {CHECK_MATCH_WINDOW_DAYS} days)."))
                    })?;
                    total = total.checked_add(cand.amount).ok_or_else(|| err("Document amount exceeds 99999999999999.99."))?;
                    chosen.push(cand.clone());
                }
                if total != amount {
                    return Err(err(format!("The selected entries total {}, not the bank's {}.", money(total), money(amount))));
                }
                let mut refs = Vec::with_capacity(chosen.len());
                let mut pairs = Vec::with_capacity(chosen.len());
                for cand in &chosen {
                    w.execute(
                        "INSERT INTO bankfeeds_transaction_match(transaction_id,journal_id,journal_line_id) VALUES(?1,?2,?3)",
                        params![item.id, cand.journal_id, cand.line_id],
                    )?;
                    refs.push(reference(tx, &cand.journal_id)?);
                    pairs.push((cand.journal_id.clone(), cand.line_id.clone()));
                }
                let first = &chosen[0];
                let joined = refs.join(" + ");
                let r = if joined.chars().count() <= 60 {
                    joined
                } else {
                    text::sanitize(&format!("{} + {} more", refs[0], refs.len() - 1), 60)
                };
                let url = format!("journal:{}", first.journal_id);
                return decide(w, actor, co, item, Decision { status: "MATCHED", kind: "MATCH", journal_id: &first.journal_id, line_id: &first.line_id, reference: &r, url, matched_lines: pairs });
            }
            let line = id(field(input, "journal_line_id"), false)?.unwrap_or_default();
            let found = match_candidates(tx, co, item, 50)?.into_iter().find(|c| c.line_id == line).ok_or_else(|| {
                err(format!("Choose an unmatched posted entry on the mapped account with the same amount and a date within {MATCH_WINDOW_DAYS} days (or a matching check within {CHECK_MATCH_WINDOW_DAYS} days)."))
            })?;
            let r = reference(tx, &found.journal_id)?;
            let url = format!("journal:{}", found.journal_id);
            decide(w, actor, co, item, Decision { status: "MATCHED", kind: "MATCH", journal_id: &found.journal_id, line_id: &line, reference: &r, url, matched_lines: vec![] })
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
            decide(w, actor, co, item, Decision { status: "ACCEPTED", kind: "CHECK", journal_id: &journal, line_id: &line, reference: &number, url: format!("checks:detail:{cid}"), matched_lines: vec![] })
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
            // Worker items 47 and 48: several received payments, and fee or adjustment lines (a processor
            // payout net of its fee), in one deposit that equals the bank row.
            // A deposit without them is requested exactly as before (no `adjustments`, the same request key).
            let adjustments = match field(input, "adjustments") {
                Value::Null => Value::Null,
                Value::Array(a) if a.is_empty() => Value::Null,
                Value::Array(a) => json!(a),
                _ => return Err(err("Enter the fees and adjustments again.")),
            };
            let intent = if adjustments.is_null() { String::new() } else { adjustments.to_string() };
            let payload = w.value(&json!({"request_key":request_key("DEPOSIT", item, &intent),"bank_id":mapped,"date":date,"memo":memo(item),"receipt_ids":ids,"adjustments":adjustments}));
            let review = banking::review_deposit_tx(tx, actor, co, &payload)?;
            let total = review.display["amount"].as_str().unwrap_or("").to_owned();
            if total != money(amount) {
                // Without fees, the frozen message (pinned by the parity scenarios).
                let what = if adjustments.is_null() { "The selected receipts total" } else { "The selected receipts and fees total" };
                return Err(err(format!("{what} {total}, not the bank's {}.", money(amount))));
            }
            let dep = banking::record_deposit_tx(tx, actor, co, &payload, &review)?;
            let text_of = |k: &str| dep[k].as_str().unwrap_or("").to_owned();
            let (journal, dep_id, identity) = (text_of("journal_id"), text_of("id"), text_of("identity_id"));
            let line = bank_line(tx, &journal, &mapped, debit, credit)?;
            let number = identity_number(tx, &identity)?;
            decide(w, actor, co, item, Decision { status: "ACCEPTED", kind: "DEPOSIT", journal_id: &journal, line_id: &line, reference: &number, url: format!("deposits:detail:{dep_id}"), matched_lines: vec![] })
        }
        "TRANSFER" => {
            if !field(input, "lines").is_null() {
                return Err(err("A transfer must have exactly two accounts. Remove the split lines."));
            }
            let other = id(field(input, "transfer_account_id"), false)?.unwrap_or_default();
            let (from, to) = if debit > 0 { (other, mapped.clone()) } else { (mapped.clone(), other) };
            let payload = w.value(&json!({"request_key":request_key("TRANSFER", item, ""),"date":date,"amount":money(amount),"from_account_id":from,"to_account_id":to,"memo":memo(item)}));
            let review = transfers::review_transfer_tx(tx, actor, co, &payload)?;
            let trf = transfers::record_transfer_tx(tx, actor, co, &payload, &review)?;
            let journal = trf["journal_id"].as_str().unwrap_or("").to_owned();
            let line = bank_line(tx, &journal, &mapped, debit, credit)?;
            let number = identity_number(tx, trf["identity_id"].as_str().unwrap_or(""))?;
            let url = format!("transfers:detail:{}", trf["id"].as_str().unwrap_or(""));
            decide(w, actor, co, item, Decision { status: "ACCEPTED", kind: "TRANSFER", journal_id: &journal, line_id: &line, reference: &number, url, matched_lines: vec![] })
        }
        _ => {
            let name = opt_text(input, "payee_name", 120)?;
            let description = text::sanitize(&item.description, 160);
            let mut lines = vec![w.value(&json!({"account":mapped,"debit":money(debit),"credit":money(credit),"description":description,"name":name}))];
            // Worker item 46: a split posts one line per split line against the one bank line; a negative
            // split line goes the other way (a credit on a withdrawal, a debit on a deposit).
            let intent = match split_lines(input, item, &mapped, false)? {
                Some(split) => {
                    for l in &split {
                        let n = money(l.amount.abs());
                        let (dr, cr) = if (l.amount > 0) == item.money_in() { ("0.00".to_owned(), n) } else { (n, "0.00".to_owned()) };
                        let text = if l.memo.is_empty() { description.clone() } else { l.memo.clone() };
                        lines.push(w.value(&json!({"account":l.account,"debit":dr,"credit":cr,"description":text,"name":name})));
                    }
                    format!("split:{}:{name}", split_intent(&split))
                }
                None => {
                    let category = account_id(field(input, "account_id"), false)?.unwrap_or_default();
                    if category == mapped {
                        return Err(err("Choose a category other than the feed account itself."));
                    }
                    lines.push(w.value(&json!({"account":category,"debit":money(credit),"credit":money(debit),"description":description,"name":name})));
                    format!("{category}:{name}")
                }
            };
            let mut draft = DraftInput::new(&date, &w.text(&memo(item)), lines);
            draft.request_key = Some(request_key("JOURNAL", item, &intent));
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
            decide(w, actor, co, item, Decision { status: "ACCEPTED", kind: "JOURNAL", journal_id: &jid, line_id: &line, reference: &r, url: format!("journal:{jid}"), matched_lines: vec![] })
        }
    }
}

/// `_check_draft`: Err(item) when the item is already decided.
fn check_draft(w: &Writes<'_, '_>, actor: UserId, co: CompanyId, transaction_id: &str, input: &Value) -> FbResult<Result<Value, Item>> {
    let tx = w.tx;
    services::access(tx, actor, co)?;
    let (item, f) = validated(w, co, transaction_id)?;
    let Some(f) = f else { return Ok(Err(item)) };
    crate::periods::require_open(tx, co, item.date().unwrap_or_default())?;
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
    let description = text::sanitize(&item.description, 1000);
    let mapped = f.ledger_account_id.clone().unwrap_or_default();
    // Worker item 46: a split check carries one expense line per split line, with the row's EFT-n number.
    let (category, lines) = match split_lines(input, &item, &mapped, true)? {
        Some(split) => (
            format!("split:{}", split_intent(&split)),
            split.iter().map(|l| json!({"tab":"EXPENSE","account_id":l.account,"description":if l.memo.is_empty() { description.clone() } else { l.memo.clone() },"rate":money(l.amount)})).collect::<Vec<_>>(),
        ),
        None => {
            let category = account_id(field(input, "account_id"), false)?.unwrap_or_default();
            let line = json!({"tab":"EXPENSE","account_id":category,"description":description,"rate":money(magnitude(&item)?)});
            (category, vec![line])
        }
    };
    let intent = format!("{category}:{}:{payee}:{number}", vendor.clone().unwrap_or_else(|| "None".into()));
    let payload = w.value(&json!({"bank_id":f.ledger_account_id,"payee_kind":payee_kind,"vendor_id":vendor,"payee_name":payee,"date":item.date(),
        "check_number":number,"memo":memo(&item),"request_key":request_key("CHECK", &item, &intent),
        "lines":lines}));
    let saved = checks::save_check_tx(tx, actor, co, &payload)?;
    // After the input's validation, before the draft commits: a refused duplicate leaves no draft behind.
    let earlier = already_recorded(tx, co, &item)?;
    let entry = if earlier.is_none() { entry_on_account(tx, co, &item)? } else { None };
    recorded_unless_confirmed(earlier, input)?;
    entry_unless_confirmed(entry, item.date(), input)?;
    Ok(Ok(saved))
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
/// document was voided / reversed through the normal path. Refused while another downloaded transaction is
/// linked to this one, or its original ledger mapping is held by a replacement ([`return_blocker`]).
pub fn return_to_review(book: &mut Book, actor: UserId, co: CompanyId, transaction_id: &str, secrets: &SecretSet) -> FbResult<Item> {
    command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        let item = item(tx, co, transaction_id)?;
        if item.status == "NEW" {
            return Ok(item);
        }
        if let Some(reason) = return_blocker(tx, co, &item)? {
            return Err(err(format!("This transaction cannot return to review yet: {reason}")));
        }
        if item.status == "ACCEPTED" && journal_live(tx, item.journal_id.as_deref())? {
            return Err(err(format!("{} is still posted. Void or reverse it from its own page first; then return this transaction to review.", item.reference)));
        }
        back_to_review(w, &user, co, &item)
    })
}

/// The entry a decided row points at is still in the books: posted, not a reversal, not reversed.
pub(crate) fn journal_live(q: &(impl Query + ?Sized), journal: Option<&str>) -> FbResult<bool> {
    let Some(journal) = journal else { return Ok(false) };
    Ok(q.query_row(
        "SELECT EXISTS(SELECT 1 FROM ledger_journal j WHERE j.id=?1 AND j.status='POSTED' AND j.reversal_of_id IS NULL AND NOT EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=j.id))",
        [journal],
        |r| r.get(0),
    )?)
}
/// "<bank account> <date> <description>" of a downloaded transaction, for a reason shown to the user.
fn row_words(q: &(impl Query + ?Sized), ordinal: i64) -> FbResult<String> {
    let (name, date, description): (String, Option<String>, String) = q.query_row(
        "SELECT f.name,coalesce(t.posted_on,t.transacted_on),t.description FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id WHERE t.ordinal=?1",
        [ordinal],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok(py::py_strip(&format!("{name} {} {}", date.unwrap_or_default(), py::py_truncate(&description, 60))).to_owned())
}
/// A later download linked to this row ("Link to existing"): that row has no entry of its own, so this row
/// must keep its decision while the link stands.
fn linked_from(q: &(impl Query + ?Sized), item: &Item) -> FbResult<Option<String>> {
    let other: Option<i64> =
        q.query_row("SELECT item_ordinal FROM bankfeeds_link WHERE earlier_ordinal=?1 ORDER BY item_ordinal LIMIT 1", [item.ordinal], |r| r.get(0)).optional()?;
    other
        .map(|o| Ok(format!("the copy downloaded again ({}) is linked to it. Return that copy to review first.", row_words(q, o)?)))
        .transpose()
}
/// Why a decided row cannot go back to review now, apart from its own still-posted entry: links, matches
/// to that live entry, or a replacement holding its ledger mapping. Shared by Undo and plain Return.
pub fn return_blocker(q: &(impl Query + ?Sized), _co: CompanyId, item: &Item) -> FbResult<Option<String>> {
    if item.status == "NEW" {
        return Ok(None);
    }
    if let Some(reason) = linked_from(q, item)? {
        return Ok(Some(reason));
    }
    if item.status == "ACCEPTED" {
        if journal_live(q, item.journal_id.as_deref())? {
            let other: Option<i64> = q.query_row(
                "SELECT b.ordinal FROM bankfeeds_transaction b WHERE b.id<>?2 AND (b.journal_id=?1 OR b.journal_line_id IN(SELECT id FROM ledger_journalline WHERE journal_id=?1) OR EXISTS(SELECT 1 FROM bankfeeds_transaction_match m WHERE m.transaction_id=b.id AND m.journal_id=?1)) ORDER BY b.ordinal LIMIT 1",
                params![item.journal_id, item.id],
                |r| r.get(0),
            ).optional()?;
            if let Some(other) = other {
                return Ok(Some(format!("another downloaded transaction ({}) is matched to this entry. Return that one to review first.", row_words(q, other)?)));
            }
        }
        if replaced_account(q, item)? {
            return Ok(Some("this row came from a bank connection that has been replaced.".to_owned()));
        }
    }
    Ok(None)
}

/// The shared end of [`return_to_review`] and [`undo_accept`]: the row is NEW again (its link to an existing
/// entry and any recorded bank change go with it), audited as `bankfeed.returned`.
fn back_to_review(w: &Writes<'_, '_>, user: &crate::store::User, co: CompanyId, item: &Item) -> FbResult<Item> {
    let tx = w.tx;
    let linked: Option<String> = tx
        .query_row("SELECT e.id FROM bankfeeds_link k JOIN bankfeeds_transaction e ON e.ordinal=k.earlier_ordinal WHERE k.item_ordinal=?1", [item.ordinal], |r| r.get(0))
        .optional()?;
    let mut previous = json!({"status":item.status,"kind":item.accepted_as,"journal_id":item.journal_id,"reference":item.reference});
    if let Some(earlier) = &linked {
        previous["linked_to"] = json!(earlier);
    }
    let count = item.return_count.checked_add(1).ok_or_else(|| err("Too many returns."))?;
    if linked.is_some() {
        w.execute("DELETE FROM bankfeeds_link WHERE item_ordinal=?1", params![item.ordinal])?;
    }
    // Back in review, the next sync brings the bank's current values into the row itself.
    w.execute("DELETE FROM bankfeeds_bank_change WHERE item_ordinal=?1", params![item.ordinal])?;
    w.execute(
        "UPDATE bankfeeds_transaction SET status='NEW',accepted_as='',journal_id=NULL,journal_line_id=NULL,reference='',reference_url='',decided_at=NULL,decided_by_id=NULL,return_count=?2,updated_at=?3 WHERE id=?1",
        params![item.id, count, py::now_utc()],
    )?;
    w.execute("DELETE FROM bankfeeds_transaction_match WHERE transaction_id=?1", params![item.id])?;
    w.audit(user, co, "bankfeed.returned", format!("bankfeedtransaction:{}", item.id), json!({"previous":previous,"amount":money(item.amount),"date":item.date().unwrap_or("None")}))?;
    super::item(tx, co, &item.id)
}

/// What Undo would void or reverse for an accepted transaction, when it can: the document the acceptance
/// created is still current and posted, and nothing else depends on it ([`undo_status`]). That status also
/// explains blockers that must be resolved before either Undo or a manual void can return the row to review.
#[derive(Clone, Debug, PartialEq)]
pub struct UndoPlan {
    pub kind: String,
    pub document_id: String,
    pub reference: String,
    pub date: String,
}
pub fn undo_plan(q: &(impl Query + ?Sized), actor: UserId, co: CompanyId, item: &Item) -> FbResult<Option<UndoPlan>> {
    Ok(undo_status(q, actor, co, item)?.and_then(Result::ok))
}
/// Undo of an accepted transaction: None when Undo does not apply (the row is not accepted, or its entry is
/// already voided or reversed, where Return to review is the way back); `Some(Err(reason))` when it applies
/// but undoing would leave the books or the review queue inconsistent, with the reason in a few plain words;
/// `Some(Ok(plan))` when it is offered.
///
/// Undo voids the document and returns the row to review, so it is refused while anything else depends on
/// that document: a later download linked to this row ("Link to existing"); another downloaded transaction
/// accepted onto or matched to any line of the entry (a transfer's other side, a card payment's card side);
/// a cleared or reconciled line of the entry; the document changed since (no longer the current version the
/// acceptance made). Checks, transfers and bank-feed journals avoid control accounts; a feed deposit only
/// clears Undeposited Funds. None can have later payments or applications depending on them. A row whose
/// ledger mapping is now held by a replacement, or whose disconnected account is no longer mapped, is
/// refused too: back in review it could not be recorded again.
pub fn undo_status(q: &(impl Query + ?Sized), actor: UserId, co: CompanyId, item: &Item) -> FbResult<Option<Result<UndoPlan, String>>> {
    if item.status != "ACCEPTED" {
        return Ok(None);
    }
    let (Some(journal), Some(_)) = (item.journal_id.as_deref(), item.journal_line_id.as_deref()) else { return Ok(None) };
    let date: Option<String> = q.query_row("SELECT date FROM ledger_journal WHERE id=?1 AND company_id=?2", params![journal, co.to_db()], |r| r.get(0)).optional()?;
    let Some(date) = date else { return Ok(None) };
    if !journal_live(q, Some(journal))? {
        return Ok(None);
    }
    let refuse = |reason: String| Ok(Some(Err(reason)));
    if let Some(reason) = crate::periods::closed_message(q, co, &date)? {
        return refuse(reason);
    }
    if let Some(reason) = return_blocker(q, co, item)? {
        return refuse(reason);
    }
    // Any cleared or reconciled line of the entry (a transfer has two bank lines) blocks its reversal.
    let cleared: bool = q.query_row(
        "SELECT EXISTS(SELECT 1 FROM banking_banklinestatus s JOIN ledger_journalline l ON l.id=s.journal_line_id WHERE l.journal_id=?1 AND s.state IN('C','R'))",
        [journal],
        |r| r.get(0),
    )?;
    if cleared {
        return refuse(format!("{} is cleared or reconciled in the bank register.", item.reference));
    }
    let Some((route, id)) = item.reference_url.rsplit_once(':') else { return refuse(format!("{} cannot be found from here.", item.reference)) };
    let current = match (item.accepted_as.as_str(), route) {
        ("CHECK", "checks:detail") => {
            let check = checks::get_check_tx(q, actor, co, id)?;
            check["current"]["state"] == "RECORDED" && check["current"]["journal_id"].as_str() == Some(journal)
        }
        ("DEPOSIT", "deposits:detail") => q.query_row(
            "SELECT EXISTS(SELECT 1 FROM banking_deposit WHERE id=?1 AND company_id=?2 AND journal_id=?3) AND NOT EXISTS(SELECT 1 FROM banking_depositreversal WHERE deposit_id=?1)",
            params![id, co.to_db(), journal],
            |r| r.get(0),
        )?,
        ("TRANSFER", "transfers:detail") => q.query_row(
            "SELECT EXISTS(SELECT 1 FROM transfers_transfer WHERE id=?1 AND company_id=?2 AND journal_id=?3) AND NOT EXISTS(SELECT 1 FROM transfers_transfervoid WHERE transfer_id=?1)",
            params![id, co.to_db(), journal],
            |r| r.get(0),
        )?,
        ("JOURNAL", "journal") => id == journal,
        _ => false,
    };
    if !current {
        return refuse(format!("{} was changed after it was recorded from this transaction.", item.reference));
    }
    if let Some(label) = retired_connection(q, item)? {
        return refuse(format!("it came from {label}, a bank connection that was disconnected, so it could not be recorded again from here."));
    }
    Ok(Some(Ok(UndoPlan { kind: item.accepted_as.clone(), document_id: id.to_owned(), reference: item.reference.clone(), date })))
}

/// Undo an acceptance in one writer: void or reverse the document the acceptance created through its own
/// service (check void, deposit reversal, transfer void, journal reversal), dated on the document's own
/// date so the original period nets to zero, then return the transaction to review. Every step keeps its
/// own audit; `bankfeed.undone` names what was voided. Refused when [`undo_plan`] offers no Undo.
pub fn undo_accept(book: &mut Book, actor: UserId, co: CompanyId, transaction_id: &str, secrets: &SecretSet) -> FbResult<Item> {
    command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        let item = item(tx, co, transaction_id)?;
        let plan = match undo_status(tx, actor, co, &item)? {
            Some(Ok(plan)) => plan,
            Some(Err(reason)) => return Err(err(format!("{} cannot be undone here: {reason}", item.reference))),
            None => return Err(err(format!("{} cannot be undone here. Void or reverse it from its own page, then return this transaction to review.", item.reference))),
        };
        let reason = json!(UNDO_REASON);
        let date = json!(plan.date);
        match plan.kind.as_str() {
            "CHECK" => {
                let revision = checks::get_check_tx(tx, actor, co, &plan.document_id)?["revision"].as_i64();
                checks::void_check_tx(tx, actor, co, &plan.document_id, revision, &date, &reason)?;
            }
            "DEPOSIT" => {
                banking::reverse_deposit_tx(tx, actor, co, &plan.document_id, Some(1), &date, &reason)?;
            }
            "TRANSFER" => {
                transfers::void_transfer_tx(tx, actor, co, &plan.document_id, Some(1), &date, &reason)?;
            }
            _ => {
                let journal = crate::JournalId::parse(&plan.document_id).ok_or_else(|| err("This transaction's entry reference is damaged."))?;
                services::reverse_journal_tx(tx, actor, co, journal, &plan.date, UNDO_REASON)?;
            }
        }
        let reversal: Option<String> =
            tx.query_row("SELECT id FROM ledger_journal WHERE reversal_of_id=?1", [item.journal_id.as_deref().unwrap_or("")], |r| r.get(0)).optional()?;
        w.audit(
            &user,
            co,
            "bankfeed.undone",
            format!("bankfeedtransaction:{}", item.id),
            json!({"kind":plan.kind,"reference":plan.reference,"journal_id":item.journal_id,"reversal_journal_id":reversal,"date":plan.date}),
        )?;
        back_to_review(w, &user, co, &item)
    })
}
/// The void / reversal reason Undo records on the document.
pub const UNDO_REASON: &str = "Undo of a bank-feed acceptance";
