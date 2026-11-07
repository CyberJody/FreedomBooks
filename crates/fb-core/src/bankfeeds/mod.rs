//! Bank feeds, the core half (frozen `bankfeeds/models.py` + `services.py`): connections, downloaded
//! accounts mapped to ledger accounts, staged transactions with dedupe and withdrawn-pending handling,
//! matching rules, suggestions, and acceptance into checks / deposits / transfers / journal entries
//! through those modules' public services. Nothing here talks to the network: the `simplefin` plugin
//! claims, fetches and parses, then hands its staged data to [`store_pull`] or
//! [`stage_bank_transactions`] (the `CoreBridge::stage_bank_transactions` entry point).
//!
//! Platform changes (see README.md): the access URL is a plugin credential, so a connection row
//! holds no cipher; Python's integer transaction pk is the book-wide `ordinal`; `reference_url`
//! holds an app route name instead of a Django URL.
mod accept;
mod mapping;
pub use mapping::{MappingTarget, NewAccountKind, save_mappings};
mod rules;
mod secrets;
mod staging;
pub mod text;
mod write;
mod credentials;
pub use credentials::{flag_unreadable_credential, restore_connection_credentials};
#[cfg(test)]
mod tests;

pub use accept::{accept, default_check_number, ignore, return_to_review};
pub use secrets::{MASK, MIN_CREDENTIAL_CHARS, SecretSet, TOO_SHORT, CREDENTIAL_ALPHABET, UNSAFE_CREDENTIAL};
pub use rules::{Rule, active_rules, all_rules, move_rule, rule_accounts, rule_for, save_rule, set_rule_active};
pub use staging::{
    ACCOUNT_DOMAIN, CONN_KEY_DOMAIN, StagedAccount, StagedPull, StagedTransaction, TRANSACTION_DOMAIN, begin_sync, identity_of,
    record_sync_failure, stage_bank_transactions, store_pull,
};

use crate::{AccountId, Book, CompanyId, FbError, FbResult, Money, UserId, documents, py, services, store};
use chrono::{Duration, NaiveDate};
use fb_db::rusqlite::{OptionalExtension, Row, params};
use fb_db::{Query, Writer};
use serde_json::{Value, json};
use write::Writes;

pub const FIRST_SYNC_DAYS: i64 = 44;
pub const OVERLAP_DAYS: i64 = 7;
pub const MAX_WINDOW_DAYS: i64 = 90;
pub const SCHEDULED_MIN_INTERVAL_SECS: i64 = 6 * 3600;
pub const MANUAL_MIN_INTERVAL_SECS: i64 = 10 * 60;
pub const MATCH_WINDOW_DAYS: i64 = 5;
pub const MAX_RULES: i64 = 200;
pub const DEFAULT_LABEL: &str = "SimpleFIN";
pub const MAX_DROPPED_IDS_AUDITED: usize = 50;
const KINDS: [&str; 5] = ["CHECK", "DEPOSIT", "TRANSFER", "JOURNAL", "MATCH"];

/// One downloaded transaction as the review screen and the services see it.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub ordinal: i64,
    pub feed_account_id: String,
    pub remote_id: String,
    pub posted_on: Option<String>,
    pub transacted_on: Option<String>,
    pub amount: i64,
    pub description: String,
    pub pending: bool,
    pub status: String,
    pub accepted_as: String,
    pub journal_id: Option<String>,
    pub journal_line_id: Option<String>,
    pub reference: String,
    pub reference_url: String,
    pub decided_by_id: Option<String>,
    pub return_count: i64,
}
impl Item {
    /// `BankFeedTransaction.date`: posted date, else transaction date.
    pub fn date(&self) -> Option<&str> {
        self.posted_on.as_deref().or(self.transacted_on.as_deref())
    }
    pub fn money_in(&self) -> bool {
        self.amount > 0
    }
}
pub(crate) const ITEM_COLS: &str = "t.id,t.ordinal,t.feed_account_id,t.remote_id,t.posted_on,t.transacted_on,t.amount,t.description,t.pending,t.status,t.accepted_as,t.journal_id,t.journal_line_id,t.reference,t.reference_url,t.decided_by_id,t.return_count";
pub(crate) fn item_row(r: &Row<'_>) -> fb_db::rusqlite::Result<Item> {
    Ok(Item {
        id: r.get(0)?,
        ordinal: r.get(1)?,
        feed_account_id: r.get(2)?,
        remote_id: r.get(3)?,
        posted_on: r.get(4)?,
        transacted_on: r.get(5)?,
        amount: r.get(6)?,
        description: r.get(7)?,
        pending: r.get(8)?,
        status: r.get(9)?,
        accepted_as: r.get(10)?,
        journal_id: r.get(11)?,
        journal_line_id: r.get(12)?,
        reference: r.get(13)?,
        reference_url: r.get(14)?,
        decided_by_id: r.get(15)?,
        return_count: r.get(16)?,
    })
}

/// A downloaded account (`BankFeedAccount`) with its connection's company.
#[derive(Clone, Debug, PartialEq)]
pub struct Feed {
    pub id: String,
    pub connection_id: String,
    pub company_id: String,
    pub conn_key: String,
    pub remote_id: String,
    pub name: String,
    pub org_name: String,
    pub currency: String,
    pub ledger_account_id: Option<String>,
    pub active: bool,
}
impl Feed {
    /// `str(feed)`.
    pub fn label(&self) -> String {
        feed_label(&self.org_name, &self.name)
    }
}
/// `str(feed)` of an organization and account name.
pub(crate) fn feed_label(org_name: &str, name: &str) -> String {
    if org_name.is_empty() { name.to_owned() } else { format!("{org_name} · {name}") }
}
const FEED_COLS: &str = "f.id,f.connection_id,c.company_id,f.conn_key,f.remote_id,f.name,f.org_name,f.currency,f.ledger_account_id,f.active";
fn feed_row(r: &Row<'_>) -> fb_db::rusqlite::Result<Feed> {
    Ok(Feed {
        id: r.get(0)?,
        connection_id: r.get(1)?,
        company_id: r.get(2)?,
        conn_key: r.get(3)?,
        remote_id: r.get(4)?,
        name: r.get(5)?,
        org_name: r.get(6)?,
        currency: r.get(7)?,
        ledger_account_id: r.get(8)?,
        active: r.get(9)?,
    })
}
pub fn feed(q: &(impl Query + ?Sized), id: &str) -> FbResult<Option<Feed>> {
    Ok(q.query_row(
        &format!("SELECT {FEED_COLS} FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE f.id=?1"),
        [id],
        feed_row,
    )
    .optional()?)
}

pub(crate) fn err(message: impl Into<String>) -> FbError {
    FbError::validation(message)
}
/// Every bank-feeds write path retains the final serialized TEXT policy here (FIX10). This writer
/// and every future writer inherit it, including downstream and later lifecycle serializers.
pub(crate) fn command<T>(book: &mut Book, secrets: &SecretSet, f: impl FnOnce(&Writes<'_, '_>) -> FbResult<T>) -> FbResult<T> {
    write::retain_secrets(book, secrets)?;
    book.write(|tx| tx.atomic(|| f(&Writes::new(tx, secrets))))
}
/// `payables.services._text`.
pub(crate) fn text(v: &Value, max: usize, required: bool) -> FbResult<String> {
    if let Some(raw) = v.as_str() { py::clean_free_text(raw)?; }
    v.as_str()
        .map(py::py_strip)
        .filter(|s| py::py_len(s) <= max && (!required || !s.is_empty()))
        .map(str::to_owned)
        .ok_or_else(|| err(format!("Enter {}text up to {max} characters.", if required { "nonblank " } else { "" })))
}
/// `payables.services._id`: a record key (UUID text in v2, where Python takes a positive int).
pub(crate) fn id(v: &Value, optional: bool) -> FbResult<Option<String>> {
    if v.is_null() && optional {
        return Ok(None);
    }
    v.as_str()
        .filter(|s| s.len() == 36 && py::record_uuid(s).is_some())
        .map(|s| Some(s.to_owned()))
        .ok_or_else(|| err("Choose a record from this company."))
}
/// Python truthiness for `bool(value)`.
pub(crate) fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}
pub(crate) fn money(n: i64) -> String {
    Money::from_db(n).to_string()
}
pub(crate) fn parse_date(d: &str) -> FbResult<NaiveDate> {
    NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| err("Invalid stored bank-feed date."))
}
pub(crate) fn add_days(d: NaiveDate, days: i64) -> FbResult<NaiveDate> {
    d.checked_add_signed(Duration::days(days)).ok_or_else(|| err("Date out of range."))
}

/// `_item`: one downloaded transaction of this company.
pub fn item(q: &(impl Query + ?Sized), co: CompanyId, transaction_id: &str) -> FbResult<Item> {
    q.query_row(
        &format!("SELECT {ITEM_COLS} FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE t.id=?1 AND c.company_id=?2"),
        params![transaction_id, co.to_db()],
        item_row,
    )
    .optional()?
    .ok_or_else(|| err("Choose a downloaded transaction of this company."))
}
/// The review list of a company, Python's model order (`-posted_on`, `-pk`): PostgreSQL sorts NULL as the
/// largest value, so undated rows come first in descending order.
pub fn items(book: &mut Book, actor: UserId, co: CompanyId) -> FbResult<Vec<Item>> {
    book.read(|q| {
        services::access(q, actor, co)?;
        let mut st = q.prepare(&format!("SELECT {ITEM_COLS} FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE c.company_id=?1 ORDER BY t.posted_on IS NOT NULL,t.posted_on DESC,t.ordinal DESC"))?;
        Ok(st.query_map([co.to_db()], item_row)?.collect::<Result<Vec<_>, _>>()?)
    })
}

fn connection_row(q: &(impl Query + ?Sized), co: CompanyId, connection_id: &str) -> FbResult<Value> {
    q.query_row(
        "SELECT id,label,host,active,last_sync_at,last_sync_ok,sync_count,forgotten_at,needs_reconnect FROM bankfeeds_connection WHERE company_id=?1 AND id=?2",
        params![co.to_db(), connection_id],
        |r| {
            let mut value = json!({"id":r.get::<_,String>(0)?,"label":r.get::<_,String>(1)?,"host":r.get::<_,String>(2)?,"active":r.get::<_,bool>(3)?,
                "last_sync_at":r.get::<_,Option<String>>(4)?,"last_sync_ok":r.get::<_,Option<bool>>(5)?,"sync_count":r.get::<_,i64>(6)?,"forgotten_at":r.get::<_,Option<String>>(7)?});
            // Preserve the existing successful-command shape; expose an explicit flag when needed.
            if r.get::<_, bool>(8)? { value["needs_reconnect"] = json!(true); }
            Ok(value)
        },
    )
    .optional()?
    .ok_or_else(|| err("Choose a bank feed connection of this company."))
}

// --- connect / forget ----------------------------------------------------------------------------

/// `connect`, after the plugin claimed the one-time token: register the connection with the host its
/// access is pinned to. The secret itself is stored by the plugin credential store, never here; the plugin
/// passes its values as `secrets`, and the host is masked of them (FIX6) before it is stored or audited.
/// A credential too short to protect never gets here: the set can't be built from one (FIX7,
/// [`TOO_SHORT`]).
pub fn connect(book: &mut Book, actor: UserId, co: CompanyId, label: &Value, host: &str, secrets: &SecretSet) -> FbResult<Value> {
    command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        let label = text(label, 120, false)?;
        let label = if label.is_empty() { DEFAULT_LABEL.to_owned() } else { label };
        let host = secrets.mask(&staging::provider_text(secrets, host).to_lowercase());
        if host.is_empty() || py::py_len(&host) > 253 {
            return Err(err("The SimpleFIN access address has no usable host."));
        }
        let key = fb_ids::new_v7().to_string();
        w.execute(
            "INSERT INTO bankfeeds_connection(id,company_id,label,host,active,created_by_id,created_at,forgotten_at,last_sync_at,last_sync_ok,last_sync_summary,sync_count) VALUES(?1,?2,?3,?4,1,?5,?6,NULL,NULL,NULL,'',0)",
            params![key, co.to_db(), label, host, actor.to_db(), py::now_utc()],
        )?;
        w.audit(&user, co, "bankfeed.connected", format!("bankfeedconnection:{key}"), json!({"label":label,"host":host}))?;
        connection_row(tx, co, &key)
    })
}

/// `forget`: deactivate; downloaded history and decisions are kept. The host wipes the plugin credential.
pub fn forget(book: &mut Book, actor: UserId, co: CompanyId, connection_id: &str, secrets: &SecretSet) -> FbResult<Value> {
    command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        let c = connection_row(tx, co, connection_id)?;
        if c["active"] == false {
            return Ok(c);
        }
        w.execute("UPDATE bankfeeds_connection SET active=0,forgotten_at=?2 WHERE id=?1", params![connection_id, py::now_utc()])?;
        w.audit(&user, co, "bankfeed.forgotten", format!("bankfeedconnection:{connection_id}"), json!({"label":c["label"],"host":c["host"]}))?;
        connection_row(tx, co, connection_id)
    })
}

// --- mapping -------------------------------------------------------------------------------------

fn control_accounts(q: &(impl Query + ?Sized), co: CompanyId) -> FbResult<Vec<String>> {
    let mut st = q.prepare("SELECT DISTINCT account_id FROM fb_control_accounts WHERE company_id=?1")?;
    Ok(st.query_map([co.to_db()], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?)
}
/// `mappable_accounts`: active postable ASSET / LIABILITY accounts that are not control accounts.
pub fn mappable_accounts(book: &mut Book, actor: UserId, co: CompanyId) -> FbResult<Vec<crate::model::Account>> {
    book.read(|q| {
        services::access(q, actor, co)?;
        let controls = control_accounts(q, co)?;
        Ok(store::accounts(q, co)?
            .into_iter()
            .filter(|a| a.active && a.postable && matches!(a.kind.as_str(), "ASSET" | "LIABILITY") && !controls.contains(&a.id.to_db()))
            .collect())
    })
}

/// `map_account`: map a downloaded account to a ledger bank or credit-card account, or clear it with null.
pub fn map_account(book: &mut Book, actor: UserId, co: CompanyId, feed_account_id: &str, ledger_account_id: &Value, secrets: &SecretSet) -> FbResult<Value> {
    command(book, secrets, |w| map_account_tx(w, actor, co, feed_account_id, ledger_account_id))
}

fn map_account_tx(w: &Writes<'_, '_>, actor: UserId, co: CompanyId, feed_account_id: &str, ledger_account_id: &Value) -> FbResult<Value> {
    let tx = w.tx;
    let user = services::access(tx, actor, co)?;
    let f = feed(tx, feed_account_id)?
        .filter(|f| f.company_id == co.to_db())
        .ok_or_else(|| err("Choose a downloaded account of this company."))?;
    let wanted = id(ledger_account_id, true)?;
    let before = f.ledger_account_id.clone();
    match &wanted {
        None => {
            w.execute("UPDATE bankfeeds_account SET ledger_account_id=NULL,mapped_at=NULL WHERE id=?1", params![f.id])?;
        }
        Some(account_id) => {
            let company = store::get_company(tx, co)?;
            if f.currency.to_uppercase() != company.currency {
                return Err(err(w.text(&format!(
                    "{} is in {}; this company keeps its books in {}. Multi-currency feeds are not supported.",
                    f.label(),
                    text::sanitize(&f.currency, 40),
                    company.currency
                ))));
            }
            let account = AccountId::parse(account_id)
                .map(|a| store::account(tx, a))
                .transpose()?
                .flatten()
                .filter(|a| a.company_id == co)
                .ok_or_else(|| err("Choose an account from this company."))?;
            if let Some(role) = documents::control_role(tx, co, account.id)? {
                return Err(err(documents::control_message(&account, &role) + " A bank feed cannot map to it."));
            }
            if !matches!(account.kind.as_str(), "ASSET" | "LIABILITY") || !account.active || !account.postable {
                return Err(err("Map a feed to an active postable bank (asset) or credit card (liability) account."));
            }
            let taken: Option<String> = tx
                .query_row(
                    &format!("SELECT {FEED_COLS} FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE c.company_id=?1 AND f.ledger_account_id=?2 AND f.active=1 AND f.id<>?3 ORDER BY c.id,f.org_name,f.name,f.id LIMIT 1"),
                    params![co.to_db(), account_id, f.id],
                    feed_row,
                )
                .optional()?
                .map(|t| t.label());
            if let Some(taken) = taken {
                return Err(err(w.text(&format!("{} · {} is already mapped to {taken}. Clear that mapping first.", account.code, account.name))));
            }
            w.execute("UPDATE bankfeeds_account SET ledger_account_id=?2,mapped_at=?3 WHERE id=?1", params![f.id, account_id, py::now_utc()])?;
        }
    }
    w.audit(
        &user,
        co,
        "bankfeed.mapped",
        format!("bankfeedaccount:{}", f.id),
        json!({"feed_account":f.label(),"remote_id":f.remote_id,"before":before,"after":wanted}),
    )?;
    Ok(json!({"id":f.id,"ledger_account_id":wanted}))
}

// --- suggestions (read only) ---------------------------------------------------------------------

/// `_direction`: (debit, credit) on the mapped account.
pub(crate) fn direction(item: &Item) -> FbResult<(i64, i64)> {
    let amount = magnitude(item)?;
    Ok(if item.money_in() { (amount, 0) } else { (0, amount) })
}
/// `abs(item.amount)` in cents, checked: an `Item` built by a caller may carry `i64::MIN`.
pub(crate) fn magnitude(item: &Item) -> FbResult<i64> {
    i64::try_from(item.amount.unsigned_abs()).map_err(|_| err("Document amount exceeds 99999999999999.99."))
}

/// One match candidate: a posted line on the mapped account.
#[derive(Clone, Debug, PartialEq)]
pub struct MatchCandidate {
    pub line_id: String,
    pub journal_id: String,
    pub label: String,
    pub date: String,
}
/// `match_candidates`: posted unreversed lines on the mapped account with this exact amount and direction,
/// dated within MATCH_WINDOW_DAYS, not already matched. Closest date first.
pub fn match_candidates(q: &(impl Query + ?Sized), co: CompanyId, item: &Item, limit: i64) -> FbResult<Vec<MatchCandidate>> {
    let Some(f) = feed(q, &item.feed_account_id)? else { return Ok(vec![]) };
    let (Some(account), Some(when)) = (f.ledger_account_id.clone(), item.date()) else { return Ok(vec![]) };
    let (debit, credit) = direction(item)?;
    let limit = limit.max(0);
    if limit == 0 {
        return Ok(vec![]);
    }
    let when = parse_date(when)?;
    let base = "SELECT l.id,j.id,j.memo,j.date FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id WHERE j.company_id=?1 AND j.status='POSTED' AND j.reversal_of_id IS NULL AND NOT EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=j.id) AND l.account_id=?2 AND l.debit=?3 AND l.credit=?4 AND NOT EXISTS(SELECT 1 FROM bankfeeds_transaction b WHERE b.journal_line_id=l.id)";
    let fetch = |sql: String, lo: String, hi: String| -> FbResult<Vec<MatchCandidate>> {
        let mut st = q.prepare(&sql)?;
        Ok(st
            .query_map(params![co.to_db(), account, debit, credit, lo, hi, limit], |r| {
                Ok(MatchCandidate { line_id: r.get(0)?, journal_id: r.get(1)?, label: r.get(2)?, date: r.get(3)? })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    };
    let fmt = |d: NaiveDate| d.format("%Y-%m-%d").to_string();
    let mut found = fetch(
        format!("{base} AND j.date>=?5 AND j.date<=?6 ORDER BY j.date,l.id LIMIT ?7"),
        fmt(when),
        fmt(add_days(when, MATCH_WINDOW_DAYS)?),
    )?;
    found.extend(fetch(
        format!("{base} AND j.date>=?5 AND j.date<?6 ORDER BY j.date DESC,l.id LIMIT ?7"),
        fmt(add_days(when, -MATCH_WINDOW_DAYS)?),
        fmt(when),
    )?);
    let mut keyed = found
        .into_iter()
        .map(|c| Ok(((parse_date(&c.date)? - when).num_days().abs(), c.line_id.clone(), c)))
        .collect::<FbResult<Vec<_>>>()?;
    keyed.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    Ok(keyed.into_iter().take(limit as usize).map(|(_, _, c)| c).collect())
}

pub(crate) fn is_designated_bank(q: &(impl Query + ?Sized), co: CompanyId, account_id: Option<&str>) -> FbResult<bool> {
    let Some(account_id) = account_id else { return Ok(false) };
    let setup = documents::setup_snapshot(q, co)?;
    Ok(setup["banks"].as_array().is_some_and(|b| b.contains(&json!(account_id))))
}

/// An undeposited receipt a bank-feed deposit could take.
#[derive(Clone, Debug, PartialEq)]
pub struct ReceiptCandidate {
    pub receipt_id: String,
    pub label: String,
    pub date: String,
}
/// `receipt_candidates`: undeposited customer / sales receipts of this exact amount.
pub fn receipt_candidates(q: &(impl Query + ?Sized), co: CompanyId, item: &Item, limit: i64) -> FbResult<Vec<ReceiptCandidate>> {
    let Some(f) = feed(q, &item.feed_account_id)? else { return Ok(vec![]) };
    let Some(when) = item.date() else { return Ok(vec![]) };
    if !item.money_in() || !is_designated_bank(q, co, f.ledger_account_id.as_deref())? {
        return Ok(vec![]);
    }
    let when_d = parse_date(when)?;
    let lo = add_days(when_d, -(MATCH_WINDOW_DAYS + 25))?.format("%Y-%m-%d").to_string();
    let mut st = q.prepare("SELECT r.id,i.number,r.date FROM sales_receipt r JOIN documents_documentidentity i ON i.id=r.identity_id WHERE r.company_id=?1 AND r.destination='UNDEPOSITED' AND r.amount=?2 AND NOT EXISTS(SELECT 1 FROM sales_receiptreversal z WHERE z.receipt_id=r.id) AND r.date>=?3 AND r.date<=?4 AND NOT EXISTS(SELECT 1 FROM banking_depositline l WHERE l.receipt_id=r.id AND NOT EXISTS(SELECT 1 FROM banking_depositreversal z WHERE z.deposit_id=l.deposit_id)) ORDER BY r.date DESC,r.id LIMIT ?5")?;
    let rows = st
        .query_map(params![co.to_db(), item.amount, lo, when, limit.max(0)], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(key, number, date)| {
            let receipt = crate::sales::receipt(q, co, &key)?;
            let name = receipt["customer_snapshot"]["display_name"].as_str().unwrap_or("");
            Ok(ReceiptCandidate { receipt_id: key, label: py::py_strip(&format!("{number} {name}")).to_owned(), date })
        })
        .collect()
}

/// `suggest`'s result.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub kind: String,
    pub matches: Vec<MatchCandidate>,
    pub receipts: Vec<ReceiptCandidate>,
    pub rule: Option<rules::Rule>,
    pub account_id: Option<String>,
    pub payee_name: String,
}
/// `_default_kind`: a check for money out of a designated bank, else a journal entry.
pub(crate) fn default_kind(q: &(impl Query + ?Sized), co: CompanyId, item: &Item, f: &Feed) -> FbResult<&'static str> {
    Ok(if !item.money_in() && is_designated_bank(q, co, f.ledger_account_id.as_deref())? { "CHECK" } else { "JOURNAL" })
}
/// `suggest`: an existing entry first, a deposit of an undeposited receipt second, a rule third. Never posts.
pub fn suggest(book: &mut Book, actor: UserId, co: CompanyId, transaction_id: &str) -> FbResult<Suggestion> {
    book.read(|q| {
        services::access(q, actor, co)?;
        let item = item(q, co, transaction_id)?;
        suggest_for(q, co, &item, None)
    })
}
/// `suggest(company, item)` for an item of any company: Python's isolation test asks another company and
/// gets no matches because candidates are scoped to the asking company.
pub fn suggest_for(q: &(impl Query + ?Sized), co: CompanyId, item: &Item, rules: Option<&[rules::Rule]>) -> FbResult<Suggestion> {
    let owned;
    let rules = match rules {
        Some(r) => r,
        None => {
            owned = rules::active_rules_in(q, co)?;
            &owned
        }
    };
    let matches = match_candidates(q, co, item, 3)?;
    let receipts = receipt_candidates(q, co, item, 3)?;
    let rule = rule_for(rules, item).cloned();
    let f = feed(q, &item.feed_account_id)?.ok_or_else(|| err("Choose a downloaded transaction of this company."))?;
    let kind = if !matches.is_empty() {
        "MATCH"
    } else if !receipts.is_empty() {
        "DEPOSIT"
    } else if rule.is_some() {
        default_kind(q, co, item, &f)?
    } else {
        ""
    };
    Ok(Suggestion {
        kind: kind.into(),
        account_id: rule.as_ref().map(|r| r.account_id.clone()),
        payee_name: rule.as_ref().map(|r| r.payee_name.clone()).unwrap_or_default(),
        matches,
        receipts,
        rule,
    })
}
