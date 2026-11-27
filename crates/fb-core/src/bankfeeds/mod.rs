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
mod review;
pub use review::{ReviewAccount, ReviewCandidates, ReviewDecision, ReviewRow, journal_receipt_ids, review_accounts, review_decision, review_page, review_snapshot};
pub use mapping::{ImportSettings, MappingTarget, NewAccountKind, save_mappings, save_mappings_and_import_settings};
mod rules;
pub mod qbo;
mod automatic;
pub use automatic::{auto_post,pending_automatic};
mod secrets;
mod staging;
pub use staging::{ProviderIdEvidence, begin_discovery, record_discovery_failure, store_discovery, store_pull_for_desktop};
pub mod text;
mod write;
mod credentials;
pub use credentials::{flag_unreadable_credential, restore_connection_credentials};
#[cfg(test)]
mod tests;

pub use accept::{MAX_MATCH_LINES, MAX_SPLIT_LINES, UNDO_REASON, UndoPlan, accept, default_check_number, entry_on_account, ignore, link_existing, return_blocker, return_to_review, undo_accept, undo_plan, undo_status};
pub use secrets::{MASK, MIN_CREDENTIAL_CHARS, SecretSet, TOO_SHORT, CREDENTIAL_ALPHABET, UNSAFE_CREDENTIAL};
pub use rules::{Rule, active_rules, all_rules, move_rule, rule_accounts, rule_for, save_rule, set_rule_active, turn_off_imported_auto_post};
pub use staging::{
    ACCOUNT_DOMAIN, BankChange, CONN_KEY_DOMAIN, ClockWarning, StagedAccount, StagedPull, StagedTransaction, SyncGap, TRANSACTION_DOMAIN, bank_changes, begin_sync,
    clock_warning, identity_of, keep_recorded, record_sync_failure, review_sync_gap, stage_bank_transactions, store_pull, sync_gaps,
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
/// Written checks (Checks, bill-payment checks, refund checks) may clear up to 180 days after issue.
pub const CHECK_MATCH_WINDOW_DAYS: i64 = 180;
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
    pub import_state: String,
    pub import_from_date: Option<String>,
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
const FEED_COLS: &str = "f.id,f.connection_id,c.company_id,f.conn_key,f.remote_id,f.name,f.org_name,f.currency,f.ledger_account_id,f.active,f.import_state,f.import_from_date";
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
        import_state: r.get(10)?,
        import_from_date: r.get(11)?,
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

/// Downloaded transactions waiting in Review transactions: the sidebar badge and the count on Home.
/// The caller checks company access.
pub fn review_count(q: &(impl Query + ?Sized), co: CompanyId) -> FbResult<i64> {
    Ok(q.query_row(
        "SELECT count(*) FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE c.company_id=?1 AND t.status='NEW'",
        [co.to_db()],
        |r| r.get(0),
    )?)
}

/// Labels of the company's live connections whose saved sign-in must be entered again before they can
/// download (the durable reconnect flag). The caller checks company access.
pub fn reconnect_labels(q: &(impl Query + ?Sized), co: CompanyId) -> FbResult<Vec<String>> {
    let mut st = q.prepare(
        "SELECT label FROM bankfeeds_connection WHERE company_id=?1 AND active=1 AND forgotten_at IS NULL AND needs_reconnect=1 ORDER BY label,id",
    )?;
    Ok(st.query_map([co.to_db()], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?)
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
    /// The entry's memo.
    pub label: String,
    pub date: String,
    /// Worker review M2: the number the entry is known by, its source document's (CHK-000027, DEP-000001,
    /// TRF-000001), else the journal's own (J000012), as Find and the journal page show it.
    pub number: String,
    /// The chosen entry number of a manual journal. Source documents use their document number.
    entry_number: String,
    /// Positive magnitude of this posted line, in cents.
    pub amount: i64,
    /// Live written check number when this entry is a check, bill-payment check, or refund check.
    pub check_number: Option<String>,
}

/// One matched ledger entry of a decided `MATCHED` bank transaction (single or multi-entry match).
#[derive(Clone, Debug, PartialEq)]
pub struct MatchedEntry {
    pub journal_id: String,
    pub line_id: String,
    pub reference: String,
    pub reference_url: String,
}

pub fn review_journal_number(entry: &str, ledger: &str) -> String {
    if entry.is_empty() || entry == ledger { ledger.to_owned() } else { format!("{entry} · {ledger}") }
}

impl MatchCandidate {
    pub fn display_number(&self) -> String {
        review_journal_number(&self.entry_number, &self.number)
    }

    /// The memo as extra words after [`number`](Self::number): without the "CHK-000027: " a document's
    /// memo starts with, at most 60 characters; empty when nothing is left.
    pub fn memo(&self) -> String {
        let memo = self.label.strip_prefix(self.number.as_str()).map(|rest| rest.strip_prefix(':').unwrap_or(rest)).unwrap_or(&self.label);
        py::py_truncate(py::py_strip(memo), 60)
    }
    /// "CHK-000027 (2026-07-06, “Bank feed: ONLINE PAYMENT”)": the entry by number and date, the memo after.
    pub fn named(&self) -> String {
        let memo = self.memo();
        let number = self.display_number();
        if memo.is_empty() { format!("{number} ({})", self.date) } else { format!("{number} ({}, “{memo}”)", self.date) }
    }
}
/// `match_candidates`: posted unreversed lines on the mapped account with this exact amount and direction,
/// dated within [`MATCH_WINDOW_DAYS`] (or up to [`CHECK_MATCH_WINDOW_DAYS`] for a live written check on a
/// withdrawal, enforcing check-number agreement when the bank description names a check number), not already
/// matched. Exact check-number match first, then closest date.
pub fn match_candidates(q: &(impl Query + ?Sized), co: CompanyId, item: &Item, limit: i64) -> FbResult<Vec<MatchCandidate>> {
    eligible_match_lines(q, co, item, true, limit)
}
/// Maximum number of partial candidates scanned by multi-entry Review UI, validation, and auto-post duplicate detection.
pub const MULTI_MATCH_POOL_LIMIT: i64 = 200;
/// Unmatched posted lines on the mapped account in the bank row's direction whose amount is strictly less
/// than the bank row's amount (for matching one bank transaction to 2..=20 posted entries totaling its exact amount).
pub fn multi_match_candidates(q: &(impl Query + ?Sized), co: CompanyId, item: &Item, limit: i64) -> FbResult<Vec<MatchCandidate>> {
    eligible_match_lines(q, co, item, false, limit)
}
/// True when 2..=[`MAX_MATCH_LINES`] of `candidates` sum exactly to `target` (conservatively true if the
/// bounded search exhausts its step budget, so auto-post holds ambiguous groups for manual review).
pub(crate) fn has_multi_match_group(candidates: &[MatchCandidate], target: i64) -> bool {
    if target <= 0 || candidates.len() < 2 {
        return false;
    }
    let mut amounts: Vec<i64> = candidates.iter().map(|c| c.amount).filter(|&a| a > 0 && a < target).collect();
    if amounts.len() < 2 {
        return false;
    }
    amounts.sort_unstable_by(|a, b| b.cmp(a));
    let mut suffix = vec![0i128; amounts.len() + 1];
    for i in (0..amounts.len()).rev() {
        suffix[i] = suffix[i + 1] + i128::from(amounts[i]);
    }
    if suffix[0] < i128::from(target) {
        return false;
    }
    fn search(amounts: &[i64], suffix: &[i128], idx: usize, count: usize, rem: i128, steps: &mut usize) -> bool {
        if rem == 0 {
            return count >= 2;
        }
        if count >= MAX_MATCH_LINES || idx >= amounts.len() || rem > suffix[idx] {
            return false;
        }
        if *steps >= 50_000 {
            return true;
        }
        *steps += 1;
        let a = i128::from(amounts[idx]);
        if a <= rem && search(amounts, suffix, idx + 1, count + 1, rem - a, steps) {
            return true;
        }
        search(amounts, suffix, idx + 1, count, rem, steps)
    }
    let mut steps = 0usize;
    search(&amounts, &suffix, 0, 0, i128::from(target), &mut steps)
}
fn eligible_match_lines(q: &(impl Query + ?Sized), co: CompanyId, item: &Item, exact_amount: bool, limit: i64) -> FbResult<Vec<MatchCandidate>> {
    let Some(f) = feed(q, &item.feed_account_id)? else { return Ok(vec![]) };
    let (Some(account), Some(when_str)) = (f.ledger_account_id.clone(), item.date()) else { return Ok(vec![]) };
    if item.amount == 0 {
        return Ok(vec![]);
    }
    let (debit, credit) = direction(item)?;
    let limit = limit.max(0);
    if limit == 0 {
        return Ok(vec![]);
    }
    let when = parse_date(when_str)?;
    let fmt = |d: NaiveDate| d.format("%Y-%m-%d").to_string();
    let lo_5 = fmt(add_days(when, -MATCH_WINDOW_DAYS)?);
    let hi_5 = fmt(add_days(when, MATCH_WINDOW_DAYS)?);
    let lo_chk = fmt(add_days(when, -CHECK_MATCH_WINDOW_DAYS)?);
    let money_out = !item.money_in();
    let bank_chk = money_out.then(|| text::match_check_number_in(&item.description)).flatten();
    let amount_clause = if exact_amount {
        "l.debit=?3 AND l.credit=?4"
    } else {
        "((?3>0 AND l.debit>0 AND l.debit<?3 AND l.credit=0) OR (?4>0 AND l.credit>0 AND l.credit<?4 AND l.debit=0))"
    };
    let sql = format!(
        "SELECT l.id,j.id,j.memo,j.date,
            COALESCE((SELECT d.number FROM documents_documentidentity d WHERE d.id=j.source_id),printf('J%06d',j.number)),
            CASE WHEN j.source_id IS NULL THEN j.entry_number ELSE '' END,
            CASE WHEN l.debit>0 THEN l.debit ELSE l.credit END,
            (SELECT h.number FROM fb_check_number_live h WHERE h.company_id=j.company_id AND h.bank_id=l.account_id AND h.identity_id=j.source_id AND h.number<>'' AND h.number NOT GLOB '*[^0-9]*' ORDER BY h.priority LIMIT 1) AS chk_no
         FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id
         WHERE j.company_id=?1 AND j.status='POSTED' AND j.reversal_of_id IS NULL
           AND NOT EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=j.id)
           AND l.account_id=?2 AND {amount_clause}
           AND NOT EXISTS(SELECT 1 FROM bankfeeds_transaction b WHERE b.journal_line_id=l.id)
           AND NOT EXISTS(SELECT 1 FROM bankfeeds_transaction_match m WHERE m.journal_line_id=l.id)
           AND NOT (?7=1 AND ?10 IS NOT NULL
             AND EXISTS(SELECT 1 FROM fb_check_number_live h WHERE h.company_id=j.company_id AND h.bank_id=l.account_id AND h.identity_id=j.source_id AND h.number<>'' AND h.number NOT GLOB '*[^0-9]*')
             AND NOT EXISTS(SELECT 1 FROM fb_check_number_live h WHERE h.company_id=j.company_id AND h.bank_id=l.account_id AND h.identity_id=j.source_id AND h.number<>'' AND h.number NOT GLOB '*[^0-9]*' AND lower(h.number)=lower(?10)))
           AND ((j.date>=?5 AND j.date<=?6)
             OR (?7=1 AND j.date>=?8 AND j.date<=?6
               AND EXISTS(SELECT 1 FROM fb_check_number_live h WHERE h.company_id=j.company_id AND h.bank_id=l.account_id AND h.identity_id=j.source_id AND h.number<>'' AND h.number NOT GLOB '*[^0-9]*'
                 AND (?10 IS NULL OR lower(h.number)=lower(?10)))))
         ORDER BY
           CASE WHEN ?7=1 AND ?10 IS NOT NULL AND EXISTS(SELECT 1 FROM fb_check_number_live h WHERE h.company_id=j.company_id AND h.bank_id=l.account_id AND h.identity_id=j.source_id AND h.number<>'' AND h.number NOT GLOB '*[^0-9]*' AND lower(h.number)=lower(?10)) THEN 0 ELSE 1 END,
           abs(julianday(j.date)-julianday(?9)),
           l.id
         LIMIT ?11"
    );
    let mut st = q.prepare(&sql)?;
    let found = st
        .query_map(
            params![co.to_db(), account, debit, credit, lo_5, hi_5, money_out, lo_chk, when_str, bank_chk, limit],
            |r| {
                Ok(MatchCandidate {
                    line_id: r.get(0)?,
                    journal_id: r.get(1)?,
                    label: r.get(2)?,
                    date: r.get(3)?,
                    number: r.get(4)?,
                    entry_number: r.get(5)?,
                    amount: r.get(6)?,
                    check_number: r.get(7)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(found)
}
/// All ledger entries matched to a `MATCHED` bank transaction (1 for a single match, 2..=20 for a multi-entry match).
pub fn matched_entries(q: &(impl Query + ?Sized), co: CompanyId, item: &Item) -> FbResult<Vec<MatchedEntry>> {
    if item.status != "MATCHED" {
        return Ok(vec![]);
    }
    let mut st = q.prepare(
        "SELECT m.journal_id,m.journal_line_id,
            COALESCE((SELECT d.number FROM documents_documentidentity d WHERE d.id=j.source_id),CASE WHEN j.entry_number<>'' THEN 'JE '||j.entry_number ELSE printf('J%06d',j.number) END)
         FROM bankfeeds_transaction_match m JOIN ledger_journal j ON j.id=m.journal_id
         WHERE m.transaction_id=?1 AND j.company_id=?2 ORDER BY j.date,m.journal_line_id",
    )?;
    let rows = st
        .query_map(params![item.id, co.to_db()], |r| {
            let jid: String = r.get(0)?;
            Ok(MatchedEntry {
                journal_id: jid.clone(),
                line_id: r.get(1)?,
                reference: r.get(2)?,
                reference_url: format!("journal:{jid}"),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if !rows.is_empty() {
        return Ok(rows);
    }
    match (&item.journal_id, &item.journal_line_id) {
        (Some(jid), Some(lid)) => Ok(vec![MatchedEntry {
            journal_id: jid.clone(),
            line_id: lid.clone(),
            reference: item.reference.clone(),
            reference_url: item.reference_url.clone(),
        }]),
        _ => Ok(vec![]),
    }
}

/// A staged transaction that is very likely already in the books: an earlier feed row of ANOTHER downloaded
/// account (an earlier connection, typically one replaced after a reinstall) produced or matched a posted,
/// unreversed line on this item's mapped ledger account, with the same amount and direction, dated within
/// [`MATCH_WINDOW_DAYS`] of this item. The earlier row carrying the same bank transaction identity ranks
/// first. [`match_candidates`] cannot offer such a line (it is already linked to that earlier row), so
/// without this check accepting would post the same money twice (review 2026-11-10, code-1 finding 2).
///
/// Rows of the item's own downloaded account need the same date, amount and description AND evidence that
/// the earlier row arrived before a completed, numbered sync, and this item afterwards. Identical charges
/// delivered together are not flagged. Such a row stays recordable with explicit confirmation; it cannot be
/// linked (a link joins two downloaded accounts), so the screen offers Record again and Ignore (`same_account`). An earlier row already
/// linked by another row of this item's account ([`link_existing`]) is not offered again, so two re-downloads
/// pair with two entries.
#[derive(Clone, Debug, PartialEq)]
pub struct Recorded {
    /// The earlier feed row whose entry this is.
    pub earlier_id: String,
    pub earlier_ordinal: i64,
    pub journal_id: String,
    pub line_id: String,
    /// The earlier row's document number (`CHK-000004`, `JE 12`) and app route (`checks:detail:<id>`).
    pub reference: String,
    pub reference_url: String,
    /// The earlier row's bank date.
    pub date: String,
    /// The earlier row carries this item's bank transaction identity (same provider id).
    pub same_identity: bool,
    /// The earlier row is on this item's own downloaded account (same date, amount and description).
    pub same_account: bool,
}
pub fn already_recorded(q: &(impl Query + ?Sized), co: CompanyId, item: &Item) -> FbResult<Option<Recorded>> {
    let Some(f) = feed(q, &item.feed_account_id)? else { return Ok(None) };
    let (Some(account), Some(when)) = (f.ledger_account_id.clone(), item.date()) else { return Ok(None) };
    if item.amount == 0 {
        return Ok(None);
    }
    let (debit, credit) = direction(item)?;
    let day = parse_date(when)?;
    let fmt = |d: NaiveDate| d.format("%Y-%m-%d").to_string();
    let (lo, hi) = (fmt(add_days(day, -MATCH_WINDOW_DAYS)?), fmt(add_days(day, MATCH_WINDOW_DAYS)?));
    let sql = "SELECT b.id,b.ordinal,b.journal_id,b.journal_line_id,b.reference,b.reference_url,coalesce(b.posted_on,b.transacted_on),
            b.remote_identity=(SELECT remote_identity FROM bankfeeds_transaction WHERE id=?2) AS same
        FROM bankfeeds_transaction b
        JOIN bankfeeds_account bf ON bf.id=b.feed_account_id JOIN bankfeeds_connection bc ON bc.id=bf.connection_id
        JOIN ledger_journalline l ON l.id=b.journal_line_id JOIN ledger_journal j ON j.id=l.journal_id
        WHERE bc.company_id=?1 AND b.id<>?2 AND b.feed_account_id<>?3 AND b.status IN('ACCEPTED','MATCHED')
          AND l.account_id=?4
          AND j.company_id=?1 AND j.status='POSTED' AND j.reversal_of_id IS NULL AND NOT EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=j.id)
          AND (
            (NOT EXISTS(SELECT 1 FROM bankfeeds_transaction_match m WHERE m.transaction_id=b.id) AND l.debit=?5 AND l.credit=?6)
            OR (b.status='MATCHED' AND b.amount=?10 AND EXISTS(SELECT 1 FROM bankfeeds_transaction_match m WHERE m.transaction_id=b.id)
              AND NOT EXISTS(SELECT 1 FROM bankfeeds_transaction_match m LEFT JOIN ledger_journalline ml ON ml.id=m.journal_line_id LEFT JOIN ledger_journal mj ON mj.id=m.journal_id
                WHERE m.transaction_id=b.id AND NOT (ml.id IS NOT NULL AND mj.id IS NOT NULL AND ml.journal_id=mj.id AND ml.account_id=?4
                  AND mj.company_id=?1 AND mj.status='POSTED' AND mj.reversal_of_id IS NULL AND NOT EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=mj.id)))
              AND (SELECT coalesce(sum(ml.debit),0) FROM bankfeeds_transaction_match m JOIN ledger_journalline ml ON ml.id=m.journal_line_id WHERE m.transaction_id=b.id)=?5
              AND (SELECT coalesce(sum(ml.credit),0) FROM bankfeeds_transaction_match m JOIN ledger_journalline ml ON ml.id=m.journal_line_id WHERE m.transaction_id=b.id)=?6)
          )
          AND coalesce(b.posted_on,b.transacted_on) BETWEEN ?7 AND ?8
          AND NOT EXISTS(SELECT 1 FROM bankfeeds_link k JOIN bankfeeds_transaction o ON o.ordinal=k.item_ordinal WHERE k.earlier_ordinal=b.ordinal AND o.feed_account_id=?3 AND o.id<>?2)
        ORDER BY same DESC,abs(julianday(coalesce(b.posted_on,b.transacted_on))-julianday(?9)),b.ordinal LIMIT 1";
    let row = |same_account: bool| {
        move |r: &Row<'_>| {
            Ok(Recorded {
                earlier_id: r.get(0)?,
                earlier_ordinal: r.get(1)?,
                journal_id: r.get(2)?,
                line_id: r.get(3)?,
                reference: r.get(4)?,
                reference_url: r.get(5)?,
                date: r.get(6)?,
                same_identity: r.get(7)?,
                same_account,
            })
        }
    };
    if let Some(found) = q.query_row(sql, params![co.to_db(), item.id, item.feed_account_id, account, debit, credit, lo, hi, when, item.amount], row(false)).optional()? {
        return Ok(Some(found));
    }
    // Rows do not store their arrival sync number. first_seen_at and the sync audit's occurred_at both use
    // actual write time (the pull's supplied `at` may differ). The last numbered sync completed before this
    // row was first seen supplies a boundary: only a candidate strictly before it arrived in an earlier
    // pull. Number order, not clock order, chooses the boundary. Ties or no numbered boundary conservatively
    // leave the row unflagged; first_seen_at is never changed by a later pull. This also works while the
    // current pull is still being stored, before its own sync event has been written.
    let boundary: Option<String> = q.query_row(
        "SELECT s.occurred_at FROM ledger_auditevent s WHERE s.company_id=?1 AND s.action='bankfeed.synced' AND s.target=?2
          AND json_type(s.detail,'$.sync')='integer' AND s.occurred_at<(SELECT first_seen_at FROM bankfeeds_transaction WHERE id=?3)
          ORDER BY json_extract(s.detail,'$.sync') DESC LIMIT 1",
        params![co.to_db(), format!("bankfeedconnection:{}", f.connection_id), item.id],
        |r| r.get(0),
    ).optional()?;
    let Some(boundary) = boundary else { return Ok(None) };
    let same = "SELECT b.id,b.ordinal,b.journal_id,b.journal_line_id,b.reference,b.reference_url,coalesce(b.posted_on,b.transacted_on),0
        FROM bankfeeds_transaction b JOIN ledger_journalline l ON l.id=b.journal_line_id JOIN ledger_journal j ON j.id=l.journal_id
        WHERE b.feed_account_id=?3 AND b.id<>?2 AND b.status IN('ACCEPTED','MATCHED') AND b.amount=?7 AND coalesce(b.posted_on,b.transacted_on)=?8 AND b.description=?9
          AND b.first_seen_at<?10 AND b.ordinal<?11
          AND l.account_id=?4
          AND j.company_id=?1 AND j.status='POSTED' AND j.reversal_of_id IS NULL AND NOT EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=j.id)
          AND (
            (NOT EXISTS(SELECT 1 FROM bankfeeds_transaction_match m WHERE m.transaction_id=b.id) AND l.debit=?5 AND l.credit=?6)
            OR (b.status='MATCHED' AND EXISTS(SELECT 1 FROM bankfeeds_transaction_match m WHERE m.transaction_id=b.id)
              AND NOT EXISTS(SELECT 1 FROM bankfeeds_transaction_match m LEFT JOIN ledger_journalline ml ON ml.id=m.journal_line_id LEFT JOIN ledger_journal mj ON mj.id=m.journal_id
                WHERE m.transaction_id=b.id AND NOT (ml.id IS NOT NULL AND mj.id IS NOT NULL AND ml.journal_id=mj.id AND ml.account_id=?4
                  AND mj.company_id=?1 AND mj.status='POSTED' AND mj.reversal_of_id IS NULL AND NOT EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=mj.id)))
              AND (SELECT coalesce(sum(ml.debit),0) FROM bankfeeds_transaction_match m JOIN ledger_journalline ml ON ml.id=m.journal_line_id WHERE m.transaction_id=b.id)=?5
              AND (SELECT coalesce(sum(ml.credit),0) FROM bankfeeds_transaction_match m JOIN ledger_journalline ml ON ml.id=m.journal_line_id WHERE m.transaction_id=b.id)=?6)
          )
        ORDER BY b.ordinal LIMIT 1";
    Ok(q.query_row(same, params![co.to_db(), item.id, item.feed_account_id, account, debit, credit, item.amount, when, item.description, boundary, item.ordinal], row(true)).optional()?)
}

/// An unmapped accepted row cannot recover its original ledger mapping: another downloaded account holds
/// it. The posted line identifies the mapping this row used; the mapping audit proves it belonged to this
/// feed, even after unmapping. A temporary unmapping without a replacement remains recoverable.
fn replaced_account(q: &(impl Query + ?Sized), item: &Item) -> FbResult<bool> {
    Ok(q.query_row(
        "SELECT EXISTS(SELECT 1 FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id
          JOIN ledger_auditevent m ON m.company_id=c.company_id AND m.action='bankfeed.mapped' AND m.target='bankfeedaccount:'||f.id
          JOIN bankfeeds_account other ON other.ledger_account_id=json_extract(m.detail,'$.after') AND other.id<>f.id AND other.active=1
          JOIN bankfeeds_connection oc ON oc.id=other.connection_id AND oc.company_id=c.company_id
          WHERE f.id=?1 AND f.ledger_account_id IS NULL
            AND other.ledger_account_id=(SELECT account_id FROM ledger_journalline WHERE id=?2))",
        params![item.feed_account_id, item.journal_line_id],
        |r| r.get(0),
    )?)
}

/// The connection label of a row whose bank connection was disconnected ("forgotten") and whose downloaded
/// account no longer maps to a ledger account, typically because a new connection to the same bank now holds
/// that mapping after a reconnect. Such a row can never be recorded (one ledger account maps to one
/// downloaded account), so the screen offers only Ignore, and Undo of its acceptance is refused.
pub fn retired_connection(q: &(impl Query + ?Sized), item: &Item) -> FbResult<Option<String>> {
    Ok(q.query_row(
        "SELECT c.label FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE f.id=?1 AND c.active=0 AND (f.ledger_account_id IS NULL OR f.active=0)",
        [&item.feed_account_id],
        |r| r.get(0),
    )
    .optional()?)
}
/// The copy of a disconnected connection's row that a connected bank connection downloaded again: the same
/// bank transaction identity, else the same date, amount and description, on a downloaded account of a
/// connection still connected.
pub fn downloaded_again(q: &(impl Query + ?Sized), co: CompanyId, item: &Item) -> FbResult<Option<Item>> {
    let id: Option<String> = q
        .query_row(
            "SELECT t.id FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id
              WHERE c.company_id=?1 AND c.active=1 AND t.feed_account_id<>?2 AND t.amount=?3
                AND (t.remote_identity=(SELECT remote_identity FROM bankfeeds_transaction WHERE id=?4)
                     OR (coalesce(t.posted_on,t.transacted_on) IS ?5 AND t.description=?6))
              ORDER BY t.remote_identity=(SELECT remote_identity FROM bankfeeds_transaction WHERE id=?4) DESC,t.ordinal LIMIT 1",
            params![co.to_db(), item.feed_account_id, item.amount, item.id, item.date(), item.description],
            |r| r.get(0),
        )
        .optional()?;
    id.map(|id| self::item(q, co, &id)).transpose()
}
/// A decided row that points at an entry no longer in the books, for example after its document was voided
/// from its own page: an accepted or matched row whose entry was voided or reversed, or a linked row whose
/// earlier transaction no longer holds a live entry. Returns what the screen says; the way forward is
/// Return to review.
pub fn stale_decision(q: &(impl Query + ?Sized), item: &Item) -> FbResult<Option<&'static str>> {
    match item.status.as_str() {
        "ACCEPTED" | "MATCHED" => {
            if !accept::journal_live(q, item.journal_id.as_deref())? {
                return Ok(Some(if item.status == "MATCHED" {
                    "The entry this was matched to was voided or reversed."
                } else {
                    "Its entry was voided or reversed."
                }));
            }
            if item.status == "MATCHED" {
                let any_dead: bool = q.query_row(
                    "SELECT EXISTS(SELECT 1 FROM bankfeeds_transaction_match m LEFT JOIN ledger_journal j ON j.id=m.journal_id
                       WHERE m.transaction_id=?1 AND NOT (j.id IS NOT NULL AND j.status='POSTED' AND j.reversal_of_id IS NULL
                         AND NOT EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=j.id)))",
                    [&item.id],
                    |r| r.get(0),
                )?;
                if any_dead {
                    return Ok(Some("One of the entries this was matched to was voided or reversed."));
                }
            }
            Ok(None)
        }
        "IGNORED" => {
            let earlier: Option<(String, String, Option<String>)> = q
                .query_row(
                    "SELECT e.id,e.status,e.journal_id FROM bankfeeds_link k JOIN bankfeeds_transaction e ON e.ordinal=k.earlier_ordinal WHERE k.item_ordinal=?1",
                    [item.ordinal],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            match earlier {
                Some((eid, status, journal)) => {
                    let multi_dead: bool = if status == "MATCHED" {
                        q.query_row(
                            "SELECT EXISTS(SELECT 1 FROM bankfeeds_transaction_match m LEFT JOIN ledger_journal j ON j.id=m.journal_id
                               WHERE m.transaction_id=?1 AND NOT (j.id IS NOT NULL AND j.status='POSTED' AND j.reversal_of_id IS NULL
                                 AND NOT EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=j.id)))",
                            [&eid],
                            |r| r.get(0),
                        )?
                    } else {
                        false
                    };
                    if !matches!(status.as_str(), "ACCEPTED" | "MATCHED") || !accept::journal_live(q, journal.as_deref())? || multi_dead {
                        Ok(Some("The entry it was linked to was voided, reversed or returned to review."))
                    } else {
                        Ok(None)
                    }
                }
                _ => Ok(None),
            }
        }
        _ => Ok(None),
    }
}
/// The company's decided rows that point at an entry no longer in the books ([`stale_decision`]).
pub fn stale_decisions(q: &(impl Query + ?Sized), co: CompanyId) -> FbResult<Vec<Item>> {
    let mut st = q.prepare(&format!(
        "SELECT {ITEM_COLS} FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id
          WHERE c.company_id=?1 AND (t.status IN('ACCEPTED','MATCHED') OR EXISTS(SELECT 1 FROM bankfeeds_link k WHERE k.item_ordinal=t.ordinal)) ORDER BY t.ordinal"
    ))?;
    let items = st.query_map([co.to_db()], item_row)?.collect::<Result<Vec<_>, _>>()?;
    let mut out = vec![];
    for i in items {
        if stale_decision(q, &i)?.is_some() {
            out.push(i);
        }
    }
    Ok(out)
}

/// How many of the company's transactions still to review look [`already_recorded`].
pub fn already_recorded_count(q: &(impl Query + ?Sized), co: CompanyId) -> FbResult<i64> {
    let mut st = q.prepare(&format!(
        "SELECT {ITEM_COLS} FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id
          WHERE c.company_id=?1 AND t.status='NEW' AND t.pending=0 AND f.ledger_account_id IS NOT NULL"
    ))?;
    let items = st.query_map([co.to_db()], item_row)?.collect::<Result<Vec<_>, _>>()?;
    let mut n = 0;
    for i in &items {
        if already_recorded(q, co, i)?.is_some() {
            n += 1;
        }
    }
    Ok(n)
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
    /// In cents.
    pub amount: i64,
}
/// `receipt_candidates`: undeposited customer / sales receipts of this exact amount.
pub fn receipt_candidates(q: &(impl Query + ?Sized), co: CompanyId, item: &Item, limit: i64) -> FbResult<Vec<ReceiptCandidate>> {
    undeposited_receipts(q, co, item, true, limit)
}
/// Worker item 48: every undeposited customer / sales receipt one bank deposit row could take, whatever its
/// amount (a deposit of several payments, or a payout net of a fee): dated in the same window as
/// [`receipt_candidates`] (30 days before the row up to its date), newest first.
pub fn deposit_candidates(q: &(impl Query + ?Sized), co: CompanyId, item: &Item, limit: i64) -> FbResult<Vec<ReceiptCandidate>> {
    undeposited_receipts(q, co, item, false, limit)
}
fn undeposited_receipts(q: &(impl Query + ?Sized), co: CompanyId, item: &Item, exact: bool, limit: i64) -> FbResult<Vec<ReceiptCandidate>> {
    let Some(f) = feed(q, &item.feed_account_id)? else { return Ok(vec![]) };
    let Some(when) = item.date() else { return Ok(vec![]) };
    if !item.money_in() || !is_designated_bank(q, co, f.ledger_account_id.as_deref())? {
        return Ok(vec![]);
    }
    let when_d = parse_date(when)?;
    let lo = add_days(when_d, -(MATCH_WINDOW_DAYS + 25))?.format("%Y-%m-%d").to_string();
    let mut st = q.prepare("SELECT r.id,i.number,r.date,r.amount FROM sales_receipt r JOIN documents_documentidentity i ON i.id=r.identity_id WHERE r.company_id=?1 AND r.destination='UNDEPOSITED' AND (?6=0 OR r.amount=?2) AND NOT EXISTS(SELECT 1 FROM sales_receiptreversal z WHERE z.receipt_id=r.id) AND r.date>=?3 AND r.date<=?4 AND NOT EXISTS(SELECT 1 FROM banking_depositline l WHERE l.receipt_id=r.id AND NOT EXISTS(SELECT 1 FROM banking_depositreversal z WHERE z.deposit_id=l.deposit_id)) ORDER BY r.date DESC,r.id LIMIT ?5")?;
    let rows = st
        .query_map(params![co.to_db(), item.amount, lo, when, limit.max(0), exact], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(key, number, date, amount)| {
            let receipt = crate::sales::receipt(q, co, &key)?;
            let name = receipt["customer_snapshot"]["display_name"].as_str().unwrap_or("");
            Ok(ReceiptCandidate { receipt_id: key, label: py::py_strip(&format!("{number} {name}")).to_owned(), date, amount })
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
    } else if let Some(p)=rule.as_ref().and_then(|r|r.policy.as_ref()) {
        {let _=p; "JOURNAL"}
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
