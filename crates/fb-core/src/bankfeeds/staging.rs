//! The DB phase of a sync (`_sync_locked` preflight, `_record_sync`, `_store`) and the plugin's
//! `CoreBridge::stage_bank_transactions` entry point. The network phase is the simplefin plugin's.
use super::*;
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// One parsed transaction, as the plugin hands it over: local accounting dates in the book's zone,
/// integer cents, sanitized description.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StagedTransaction {
    pub remote_id: String,
    pub posted_on: Option<String>,
    pub transacted_on: Option<String>,
    pub amount: i64,
    pub description: String,
    pub pending: bool,
}
/// One parsed account of a complete pull (`parse_account_set()['accounts'][i]`).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StagedAccount {
    pub conn_key: String,
    pub remote_id: String,
    pub name: String,
    pub org_name: String,
    pub currency: String,
    pub balance: Option<i64>,
    pub available: Option<i64>,
    pub balance_epoch: Option<i64>,
    #[serde(default)]
    pub skipped: Vec<String>,
    pub transactions: Vec<StagedTransaction>,
}
/// A whole parsed pull (`parse_account_set()`): accounts, skipped reasons and provider error messages.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct StagedPull {
    pub accounts: Vec<StagedAccount>,
    #[serde(default)]
    pub skipped: Vec<String>,
    #[serde(default)]
    pub errors: Vec<String>,
}

/// Not copied from Python (L4-BANKFEEDS.md "Python bugs not copied", FIX4): every provider-derived string
/// passes through [`text::redact`] once, where it enters the core, before anything is stored, summarized,
/// audited or returned. Python stores what its parser's `text()` kept, so a credential its regex misses
/// reaches the account row, `str(feed)` and every later audit (`bankfeed.mapped`, `.accepted`, …).
/// `redact` is idempotent on Python's own output, so this changes nothing where Python removed every
/// credential. Bounds are checked on the redacted value, the one that is stored.
///
/// FIX6: the guarantee is known-secret masking. The connection's own secrets ([`SecretSet`]) are masked in
/// every spelling first, on the raw text, so the structural rule can never cut a secret in half and leave
/// a piece; `redact` stays as the best-effort backstop for credentials the book never held, and the
/// result is masked again in case redaction joined a spelling back together.
pub(crate) fn provider_ingress(secrets: &SecretSet, value: &str) -> String {
    // Removing controls can join a credential spelling: clean BEFORE known-secret masking.
    let cleaned: String = value.chars().filter(|&c| fb_db::check_numbers::free_text_char_allowed(c)).collect();
    provider_text(secrets, &cleaned)
}
/// Masking is also used on carried human text by the central writer: do not clean it here.
pub(crate) fn provider_text(secrets: &SecretSet, value: &str) -> String {
    let masked = secrets.mask(value);
    let redacted = text::redact(&masked);
    // masking what redaction left unchanged would find nothing new
    if redacted == masked { masked } else { secrets.mask(&redacted) }
}
/// A written text built from provider text (summaries, sanitized ids): masked before and after
/// `sanitize`, whose tag removal and joins could complete a spelling.
pub(crate) fn written(secrets: &SecretSet, value: &str, limit: usize) -> String {
    secrets.mask(&text::sanitize(&provider_text(secrets, value), limit))
}

/// Identity domains of provider identifiers (FIX5, migration 0136).
pub const CONN_KEY_DOMAIN: &str = "connection-key";
pub const ACCOUNT_DOMAIN: &str = "account";
pub const TRANSACTION_DOMAIN: &str = "transaction";
/// FIX5: identity is kept apart from display. Redacting at entry (FIX4) made two provider ids that differ
/// only in URL userinfo one stored id, so dedupe and lookups merged them. Every identifier now also gets a
/// stable, credential-free key ([`fb_db::bankfeeds_text::identity_key`]: HMAC-SHA256 of the RAW value under
/// the book's random secret), and every uniqueness, dedupe, mapping and lookup decision uses that key.
/// The redacted text stays the only text stored, summarized or audited. The secret is made on first use,
/// in the same writer as the first downloaded row, and kept in the encrypted book.
fn identity_secret(w: &Writes<'_, '_>) -> FbResult<Vec<u8>> {
    let tx = w.tx;
    if let Some(secret) = tx.query_row("SELECT secret FROM bankfeeds_identity_secret WHERE name='identity'", [], |r| r.get::<_, Vec<u8>>(0)).optional()? {
        return Ok(secret);
    }
    let mut secret = vec![0u8; fb_db::bankfeeds_text::IDENTITY_SECRET_LEN];
    getrandom::fill(&mut secret).map_err(|e| FbError::validation(format!("The system random source failed: {e}")))?;
    w.execute("INSERT INTO bankfeeds_identity_secret(name,secret) VALUES('identity',?1)", params![secret])?;
    Ok(secret)
}
/// The identity key a raw provider value has in this book, or None when the book has no secret yet (it
/// then holds no downloaded rows). For lookups by the provider's own id (the harness, tests).
pub fn identity_of(q: &impl Query, domain: &str, raw: &str) -> FbResult<Option<String>> {
    let mut st = q.prepare("SELECT secret FROM bankfeeds_identity_secret WHERE name='identity'")?;
    let secret: Option<Vec<u8>> = st.query_row([], |r| r.get(0)).optional()?;
    Ok(secret.map(|s| fb_db::bankfeeds_text::identity_key(&s, domain, raw)))
}
/// Identity keys of one pulled account, computed from the raw values before redaction.
struct AccountKeys {
    conn_key: String,
    remote: String,
    transactions: Vec<String>,
}
fn account_keys(secret: &[u8], a: &StagedAccount) -> AccountKeys {
    let key = |domain: &str, raw: &str| fb_db::bankfeeds_text::identity_key(secret, domain, raw);
    AccountKeys {
        conn_key: key(CONN_KEY_DOMAIN, &a.conn_key),
        remote: key(ACCOUNT_DOMAIN, &a.remote_id),
        transactions: a.transactions.iter().map(|t| key(TRANSACTION_DOMAIN, &t.remote_id)).collect(),
    }
}
fn provider_transaction(secrets: &SecretSet, t: &StagedTransaction) -> StagedTransaction {
    StagedTransaction { remote_id: provider_ingress(secrets, &t.remote_id), description: provider_ingress(secrets, &t.description), ..t.clone() }
}
/// FIX8, defence in depth behind [`super::write::Writes`]: the organization and account names are
/// joined by `str(feed)` in every later message and audit, also after the connection is forgotten and
/// the host holds no credential to mask with. So they are checked joined here, while the set is known:
/// when the label spells a secret that neither name spells alone, the masked label becomes the name and
/// the organization is left empty.
fn joined_names(secrets: &SecretSet, org_name: String, name: String) -> (String, String) {
    let label = feed_label(&org_name, &name);
    let masked = secrets.mask(&label);
    if masked == label { (org_name, name) } else { (String::new(), masked) }
}
fn removed_characters(value: &str) -> usize {
    value.chars().filter(|&c| !fb_db::check_numbers::free_text_char_allowed(c)).count()
}
fn transaction_removed(t: &StagedTransaction) -> usize {
    removed_characters(&t.remote_id) + removed_characters(&t.description)
}
fn pull_removed(pull: &StagedPull) -> usize {
    let all = |values: &[String]| values.iter().map(|s| removed_characters(s)).sum::<usize>();
    all(&pull.skipped) + all(&pull.errors) + pull.accounts.iter().map(|a| {
        [&a.conn_key, &a.remote_id, &a.name, &a.org_name, &a.currency].into_iter().map(|s| removed_characters(s)).sum::<usize>()
            + all(&a.skipped) + a.transactions.iter().map(transaction_removed).sum::<usize>()
    }).sum::<usize>()
}
fn audit_removed(mut detail: Value, count: usize) -> Value {
    // Preserve the existing clean-provider audit contract; count only newly removed characters.
    if count > 0 { detail["removed_characters"] = json!(count); }
    detail
}
fn provider_pull(secrets: &SecretSet, pull: &StagedPull) -> StagedPull {
    let one = |s: &str| provider_ingress(secrets, s);
    let all = |v: &[String]| v.iter().map(|s| one(s)).collect::<Vec<_>>();
    StagedPull {
        accounts: pull
            .accounts
            .iter()
            .map(|a| {
                let (org_name, name) = joined_names(secrets, one(&a.org_name), one(&a.name));
                StagedAccount {
                    conn_key: one(&a.conn_key),
                    remote_id: one(&a.remote_id),
                    name,
                    org_name,
                    currency: one(&a.currency),
                    skipped: all(&a.skipped),
                    transactions: a.transactions.iter().map(|t| provider_transaction(secrets, t)).collect(),
                    ..a.clone()
                }
            })
            .collect(),
        skipped: all(&pull.skipped),
        errors: all(&pull.errors),
    }
}

fn epoch_text(epoch: i64) -> FbResult<String> {
    DateTime::<Utc>::from_timestamp(epoch, 0)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string())
        .ok_or_else(|| err("Invalid timestamp."))
}
fn parse_now(now: &str) -> FbResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(now).map(|d| d.with_timezone(&Utc)).map_err(|_| err("Invalid timestamp."))
}
fn stamp(d: DateTime<Utc>) -> String {
    d.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
}
fn opt_date(d: &Option<String>) -> FbResult<Option<String>> {
    d.as_deref()
        .map(|s| fb_db::dates::parse_iso_date(s).filter(|c| c == s).ok_or_else(|| err("Enter a valid date.")))
        .transpose()
}
/// Model field bounds `full_clean` applies before saving (Django's max_length message).
fn bounded(value: &str, max: usize) -> FbResult<()> {
    if py::py_len(value) > max {
        return Err(err(py::max_length_message(max, value)));
    }
    Ok(())
}
fn check_transaction(t: &StagedTransaction) -> FbResult<()> {
    bounded(&t.remote_id, 200)?;
    if t.remote_id.is_empty() {
        return Err(err(py::BLANK));
    }
    bounded(&t.description, 500)?;
    if !(-9_999_999_999_999_999..=9_999_999_999_999_999).contains(&t.amount) {
        return Err(err("Constraint “bankfeed_amount_bounds” is violated."));
    }
    Ok(())
}

/// `_sync_locked` before the network call: refuses a forgotten connection or a pull inside the minimum
/// interval, and returns the window start epoch (`_window`). `now` is RFC 3339 (the host's clock).
pub fn begin_sync(book: &mut Book, actor: UserId, co: CompanyId, connection_id: &str, now: &str, min_interval_secs: i64, force: bool) -> FbResult<i64> {
    book.read(|q| {
        services::access(q, actor, co)?;
        let c = connection_row(q, co, connection_id)?;
        if c["needs_reconnect"] == true { return Err(err(UNSAFE_CREDENTIAL)); }
        if c["active"] != true {
            return Err(err("This connection was forgotten. Connect again with a new setup token."));
        }
        let now = parse_now(now)?;
        let last = c["last_sync_at"].as_str().map(parse_now).transpose()?;
        let ok = c["last_sync_ok"] == true;
        if let Some(last) = last
            && !force
            && ok
        {
            let since = now - last;
            let interval = chrono::Duration::try_seconds(min_interval_secs).ok_or_else(|| err("The minimum sync interval is out of range."))?;
            if since < interval {
                let remaining = interval.checked_sub(&since).ok_or_else(|| err("The minimum sync interval is out of range."))?;
                let minutes = remaining.num_microseconds().map(|m| m.div_euclid(60_000_000)).unwrap_or(0) + 1;
                return Err(err(format!("Synced {}. SimpleFIN asks apps not to pull too often; try again in {minutes} min.", ago(since))));
            }
        }
        let back = |from: DateTime<Utc>, days: i64| from.checked_sub_signed(chrono::Duration::days(days)).ok_or_else(|| err("Invalid timestamp."));
        let start = match last {
            Some(last) if ok => back(last, OVERLAP_DAYS)?.max(back(now, MAX_WINDOW_DAYS)?),
            _ => back(now, FIRST_SYNC_DAYS)?,
        };
        Ok(start.timestamp())
    })
}
/// `_ago`.
fn ago(delta: chrono::Duration) -> String {
    let minutes = delta.num_microseconds().map(|m| m.div_euclid(60_000_000)).unwrap_or(i64::MAX);
    if minutes < 120 { format!("{minutes} min ago") } else { format!("{} h ago", minutes.div_euclid(60)) }
}

/// `_record_sync(ok=False)`: a failed pull is recorded with its sanitized, user-safe summary, masked of the
/// connection's `secrets` (FIX6).
#[allow(clippy::too_many_arguments)]
pub fn record_sync_failure(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    connection_id: &str,
    now: &str,
    summary: &str,
    scheduled: bool,
    secrets: &SecretSet,
) -> FbResult<()> {
    command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        connection_row(tx, co, connection_id)?;
        let removed = removed_characters(summary);
        let summary = provider_ingress(secrets, summary);
        let summary = if removed > 0 { format!("{removed} control/format characters removed; {summary}") } else { summary };
        let summary = w.text(&written(secrets, &summary, 500));
        w.execute(
            "UPDATE bankfeeds_connection SET last_sync_at=?2,last_sync_ok=0,last_sync_summary=?3,sync_count=sync_count+1 WHERE id=?1",
            params![connection_id, stamp(parse_now(now)?), summary],
        )?;
        w.audit(&user, co, "bankfeed.synced", format!("bankfeedconnection:{connection_id}"), audit_removed(json!({"ok":false,"summary":summary,"scheduled":scheduled}), removed))
    })
}

struct Stored {
    id: String,
    status: String,
    posted_on: Option<String>,
    transacted_on: Option<String>,
    amount: i64,
    description: String,
    pending: bool,
}
#[derive(Default)]
struct Counts {
    new: i64,
    updated: i64,
    posted: i64,
    unchanged: i64,
}
/// Message of Django's `validate_unique` for `bankfeed_transaction_once`.
const DUPLICATE_IN_PULL: &str = "Bank feed transaction with this Feed account and Remote id already exists.";
/// Upsert one transaction of a feed account (the inner loop of `_store`). `inserted` holds the ids this
/// pull already created for the account: Python looks rows up in a map read before the loop, so an id
/// listed twice as new is inserted twice and `full_clean` refuses the whole store.
/// `t` is the redacted transaction and `identity` its raw id's key (FIX5): rows are found and kept apart by
/// the key, never by the displayed id.
fn upsert(w: &Writes<'_, '_>, feed_id: &str, t: &StagedTransaction, identity: &str, now: &str, counts: &mut Counts, inserted: Option<&mut Vec<String>>) -> FbResult<String> {
    let tx = w.tx;
    let posted_on = opt_date(&t.posted_on)?;
    let transacted_on = opt_date(&t.transacted_on)?;
    let existing = tx
        .query_row(
            "SELECT id,status,posted_on,transacted_on,amount,description,pending FROM bankfeeds_transaction WHERE feed_account_id=?1 AND remote_identity=?2",
            params![feed_id, identity],
            |r| {
                Ok(Stored {
                    id: r.get(0)?,
                    status: r.get(1)?,
                    posted_on: r.get(2)?,
                    transacted_on: r.get(3)?,
                    amount: r.get(4)?,
                    description: r.get(5)?,
                    pending: r.get(6)?,
                })
            },
        )
        .optional()?;
    if let (Some(_), Some(seen)) = (&existing, &inserted)
        && seen.iter().any(|k| k == identity)
    {
        check_transaction(t)?;
        return Err(err(DUPLICATE_IN_PULL));
    }
    let Some(row) = existing else {
        check_transaction(t)?;
        if let Some(seen) = inserted {
            seen.push(identity.to_owned());
        }
        let ordinal: i64 = tx.query_row("SELECT coalesce((SELECT value FROM bankfeeds_counter WHERE name='transaction'),0)+1", [], |r| r.get(0))?;
        let key = fb_ids::new_v7().to_string();
        w.execute(
            "INSERT INTO bankfeeds_transaction(id,ordinal,feed_account_id,remote_id,posted_on,transacted_on,amount,description,pending,first_seen_at,updated_at,status,accepted_as,journal_id,journal_line_id,reference,reference_url,decided_at,decided_by_id,return_count,remote_identity) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?10,'NEW','',NULL,NULL,'','',NULL,NULL,0,?11)",
            params![key, ordinal, feed_id, t.remote_id, posted_on, transacted_on, t.amount, t.description, t.pending, now, identity],
        )?;
        counts.new += 1;
        return Ok(key);
    };
    if row.status != "NEW" && row.status != "IGNORED" {
        counts.unchanged += 1;
        return Ok(row.id);
    }
    let changed = row.posted_on != posted_on || row.transacted_on != transacted_on || row.amount != t.amount || row.description != t.description || row.pending != t.pending;
    if !changed {
        counts.unchanged += 1;
        return Ok(row.id);
    }
    if row.pending && !t.pending {
        counts.posted += 1;
    } else {
        counts.updated += 1;
    }
    check_transaction(t)?;
    w.execute(
        "UPDATE bankfeeds_transaction SET posted_on=?2,transacted_on=?3,amount=?4,description=?5,pending=?6,updated_at=?7 WHERE id=?1",
        params![row.id, posted_on, transacted_on, t.amount, t.description, t.pending, now],
    )?;
    Ok(row.id)
}
fn active_connection(tx: &Writer<'_>, co: CompanyId, connection_id: &str) -> FbResult<Value> {
    let c = connection_row(tx, co, connection_id)?;
    if c["needs_reconnect"] == true { return Err(err(UNSAFE_CREDENTIAL)); }
    Ok(c)
}

/// `_store`: the DB phase of a complete sync. Upserts accounts and transactions (dedupe on the feed account
/// and SimpleFIN transaction id), withdraws superseded pending rows first seen inside this pull's window,
/// and records the result on the connection. `window_start` is the pull's start epoch; None disables the
/// window filter exactly as Python's `window_start=None`. `secrets` are the connection's own credential
/// values (FIX6): masked from every provider string before anything is stored, summarized, audited or
/// returned; identity keys are computed from the raw values first.
#[allow(clippy::too_many_arguments)]
pub fn store_pull(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    connection_id: &str,
    pull: &StagedPull,
    now: &str,
    scheduled: bool,
    window_start: Option<i64>,
    secrets: &SecretSet,
) -> FbResult<Value> {
    let raw = pull;
    let removed = pull_removed(pull);
    let pull = &provider_pull(secrets, pull);
    command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        active_connection(tx, co, connection_id)?;
        let secret = identity_secret(w)?;
        let keys: Vec<AccountKeys> = raw.accounts.iter().map(|a| account_keys(&secret, a)).collect();
        let now_text = stamp(parse_now(now)?);
        let mut counts = Counts::default();
        let mut accounts = 0i64;
        let mut skipped = pull.skipped.len() as i64;
        let mut pending_dropped = 0i64;
        let mut dropped_ids: Vec<String> = vec![];
        let mut reasons: Vec<String> = pull.skipped.clone();
        let window = window_start.map(epoch_text).transpose()?;
        for (entry, keys) in pull.accounts.iter().zip(&keys) {
            bounded(&entry.conn_key, 200)?;
            bounded(&entry.remote_id, 200)?;
            let found: Option<String> = tx
                .query_row(
                    "SELECT id FROM bankfeeds_account WHERE connection_id=?1 AND conn_key_identity=?2 AND remote_identity=?3",
                    params![connection_id, keys.conn_key, keys.remote],
                    |r| r.get(0),
                )
                .optional()?;
            for (value, max) in [(&entry.name, 120), (&entry.org_name, 120), (&entry.currency, 200)] {
                bounded(value, max)?;
            }
            if entry.name.is_empty() || entry.currency.is_empty() || entry.remote_id.is_empty() {
                return Err(err(py::BLANK));
            }
            let balance_at = entry.balance_epoch.map(epoch_text).transpose()?;
            let feed_id = match found {
                Some(key) => {
                    w.execute(
                        "UPDATE bankfeeds_account SET name=?2,org_name=?3,currency=?4,balance=?5,available_balance=?6,balance_at=?7,last_seen_at=?8 WHERE id=?1",
                        params![key, entry.name, entry.org_name, entry.currency, entry.balance, entry.available, balance_at, now_text],
                    )?;
                    key
                }
                None => {
                    let key = fb_ids::new_v7().to_string();
                    w.execute(
                        "INSERT INTO bankfeeds_account(id,connection_id,conn_key,remote_id,name,org_name,currency,balance,available_balance,balance_at,ledger_account_id,mapped_at,first_seen_at,last_seen_at,active,conn_key_identity,remote_identity) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,NULL,NULL,?11,?12,1,?13,?14)",
                        params![key, connection_id, entry.conn_key, entry.remote_id, entry.name, entry.org_name, entry.currency, entry.balance, entry.available, balance_at, py::now_utc(), now_text, keys.conn_key, keys.remote],
                    )?;
                    key
                }
            };
            accounts += 1;
            let label = feed(tx, &feed_id)?.map(|f| f.label()).unwrap_or_default();
            reasons.extend(entry.skipped.iter().map(|why| format!("{label}: {why}")));
            skipped += entry.skipped.len() as i64;
            let mut inserted = vec![];
            for (t, identity) in entry.transactions.iter().zip(&keys.transactions) {
                upsert(w, &feed_id, t, identity, &py::now_utc(), &mut counts, Some(&mut inserted))?;
            }
            let mut st = tx.prepare("SELECT id,remote_id,first_seen_at,remote_identity FROM bankfeeds_transaction WHERE feed_account_id=?1 AND pending=1 AND status='NEW' ORDER BY ordinal")?;
            let stale = st
                .query_map([&feed_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            for (key, remote, first_seen, identity) in stale {
                if window.as_ref().is_some_and(|w| first_seen.as_str() < w.as_str()) || keys.transactions.contains(&identity) {
                    continue;
                }
                w.execute("DELETE FROM bankfeeds_transaction WHERE id=?1", params![key])?;
                dropped_ids.push(remote);
                pending_dropped += 1;
            }
        }
        let mut messages = vec![format!("{accounts} accounts, {} new transactions", counts.new)];
        if removed > 0 { messages.push(format!("{removed} control/format characters removed")); }
        if counts.posted > 0 {
            messages.push(format!("{} now posted", counts.posted));
        }
        if counts.updated > 0 {
            messages.push(format!("{} updated", counts.updated));
        }
        if pending_dropped > 0 {
            messages.push(format!("{pending_dropped} pending withdrawn (replaced by the posted item or dropped by the bank)"));
        }
        if skipped > 0 {
            messages.push(format!("{skipped} skipped ({})", reasons.iter().take(5).cloned().collect::<Vec<_>>().join("; ")));
        }
        for error in &pull.errors {
            messages.push(format!("SimpleFIN: {error}"));
        }
        let summary = w.text(&written(secrets, &messages.join("; "), 500));
        w.execute(
            "UPDATE bankfeeds_connection SET last_sync_at=?2,last_sync_ok=1,last_sync_summary=?3,sync_count=sync_count+1 WHERE id=?1",
            params![connection_id, now_text, summary],
        )?;
        let dropped: Vec<String> = dropped_ids.iter().take(MAX_DROPPED_IDS_AUDITED).map(|i| written(secrets, i, 200)).collect();
        let errors = pull.errors.len() as i64;
        w.audit(
            &user,
            co,
            "bankfeed.synced",
            format!("bankfeedconnection:{connection_id}"),
            audit_removed(json!({"ok":true,"scheduled":scheduled,"summary":summary,"pending_dropped_ids":dropped,"accounts":accounts,"new":counts.new,"updated":counts.updated,"posted":counts.posted,"unchanged":counts.unchanged,"skipped":skipped,"errors":errors,"pending_dropped":pending_dropped}), removed),
        )?;
        Ok(json!({"summary":summary,"provider_errors":pull.errors,"reasons":reasons.iter().map(|r| secrets.mask(r)).collect::<Vec<_>>(),"accounts":accounts,"new":counts.new,"updated":counts.updated,"posted":counts.posted,"unchanged":counts.unchanged,"skipped":skipped,"errors":errors,"pending_dropped":pending_dropped}))
    })
}

/// `CoreBridge::stage_bank_transactions` for a scoped connection: stage (never post) transactions of feed
/// accounts this connection already downloaded, deduplicated on (account, transaction id). A partial
/// stage never withdraws pending rows and does not record a sync: only [`store_pull`] knows the pull's
/// complete inventory. The batch is audited (`bankfeed.staged`: counts and every staged account /
/// transaction id) in the same writer. Returns the staged row ids in input order. `secrets` as in
/// [`store_pull`].
pub fn stage_bank_transactions(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    connection_id: &str,
    transactions: &[(String, StagedTransaction)],
    secrets: &SecretSet,
) -> FbResult<Vec<String>> {
    let raw = transactions;
    let removed = transactions.iter().map(|(a,t)| removed_characters(a) + transaction_removed(t)).sum();
    let transactions: Vec<(String, StagedTransaction)> = transactions.iter().map(|(a, t)| (provider_ingress(secrets, a), provider_transaction(secrets, t))).collect();
    command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        let c = active_connection(tx, co, connection_id)?;
        if c["needs_reconnect"] == true { return Err(err(UNSAFE_CREDENTIAL)); }
        if c["active"] != true {
            return Err(err("This connection was forgotten. Connect again with a new setup token."));
        }
        let secret = identity_secret(w)?;
        let mut counts = Counts::default();
        let mut out = vec![];
        for ((raw_account, raw_t), (_, t)) in raw.iter().zip(&transactions) {
            let account_key = fb_db::bankfeeds_text::identity_key(&secret, ACCOUNT_DOMAIN, raw_account);
            let mut st = tx.prepare("SELECT id FROM bankfeeds_account WHERE connection_id=?1 AND remote_identity=?2 AND active=1")?;
            let feeds = st.query_map(params![connection_id, account_key], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
            let [feed_id] = feeds.as_slice() else {
                return Err(err("Sync the account list before staging its transactions."));
            };
            let identity = fb_db::bankfeeds_text::identity_key(&secret, TRANSACTION_DOMAIN, &raw_t.remote_id);
            out.push(upsert(w, feed_id, t, &identity, &py::now_utc(), &mut counts, None)?);
        }
        let staged: Vec<Value> = transactions.iter().map(|(account, t)| json!({"account":written(secrets, account, 200),"remote_id":written(secrets, &t.remote_id, 200)})).collect();
        w.audit(
            &user,
            co,
            "bankfeed.staged",
            format!("bankfeedconnection:{connection_id}"),
            audit_removed(json!({"staged":staged.len(),"new":counts.new,"updated":counts.updated,"posted":counts.posted,"unchanged":counts.unchanged,"transactions":staged}), removed),
        )?;
        Ok(out)
    })
}
