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
    let cleaned: String = value
        .chars()
        .filter(|&c| fb_db::check_numbers::free_text_char_allowed(c))
        .collect();
    provider_text(secrets, &cleaned)
}
/// Masking is also used on carried human text by the central writer: do not clean it here.
pub(crate) fn provider_text(secrets: &SecretSet, value: &str) -> String {
    let masked = secrets.mask(value);
    let redacted = text::redact(&masked);
    // masking what redaction left unchanged would find nothing new
    if redacted == masked {
        masked
    } else {
        secrets.mask(&redacted)
    }
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
    if let Some(secret) = tx
        .query_row(
            "SELECT secret FROM bankfeeds_identity_secret WHERE name='identity'",
            [],
            |r| r.get::<_, Vec<u8>>(0),
        )
        .optional()?
    {
        return Ok(secret);
    }
    let mut secret = vec![0u8; fb_db::bankfeeds_text::IDENTITY_SECRET_LEN];
    getrandom::fill(&mut secret)
        .map_err(|e| FbError::validation(format!("The system random source failed: {e}")))?;
    w.execute(
        "INSERT INTO bankfeeds_identity_secret(name,secret) VALUES('identity',?1)",
        params![secret],
    )?;
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
        transactions: a
            .transactions
            .iter()
            .map(|t| key(TRANSACTION_DOMAIN, &t.remote_id))
            .collect(),
    }
}
fn provider_transaction(secrets: &SecretSet, t: &StagedTransaction) -> StagedTransaction {
    StagedTransaction {
        remote_id: provider_ingress(secrets, &t.remote_id),
        description: provider_ingress(secrets, &t.description),
        ..t.clone()
    }
}
/// FIX8, defence in depth behind [`super::write::Writes`]: the organization and account names are
/// joined by `str(feed)` in every later message and audit, also after the connection is forgotten and
/// the host holds no credential to mask with. So they are checked joined here, while the set is known:
/// when the label spells a secret that neither name spells alone, the masked label becomes the name and
/// the organization is left empty.
fn joined_names(secrets: &SecretSet, org_name: String, name: String) -> (String, String) {
    let label = feed_label(&org_name, &name);
    let masked = secrets.mask(&label);
    if masked == label {
        (org_name, name)
    } else {
        (String::new(), masked)
    }
}
fn removed_characters(value: &str) -> usize {
    value
        .chars()
        .filter(|&c| !fb_db::check_numbers::free_text_char_allowed(c))
        .count()
}
fn transaction_removed(t: &StagedTransaction) -> usize {
    removed_characters(&t.remote_id) + removed_characters(&t.description)
}
fn pull_removed(pull: &StagedPull) -> usize {
    let all = |values: &[String]| values.iter().map(|s| removed_characters(s)).sum::<usize>();
    all(&pull.skipped)
        + all(&pull.errors)
        + pull
            .accounts
            .iter()
            .map(|a| {
                [&a.conn_key, &a.remote_id, &a.name, &a.org_name, &a.currency]
                    .into_iter()
                    .map(|s| removed_characters(s))
                    .sum::<usize>()
                    + all(&a.skipped)
                    + a.transactions
                        .iter()
                        .map(transaction_removed)
                        .sum::<usize>()
            })
            .sum::<usize>()
}
fn audit_removed(mut detail: Value, count: usize) -> Value {
    // Preserve the existing clean-provider audit contract; count only newly removed characters.
    if count > 0 {
        detail["removed_characters"] = json!(count);
    }
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
                    transactions: a
                        .transactions
                        .iter()
                        .map(|t| provider_transaction(secrets, t))
                        .collect(),
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
    DateTime::parse_from_rfc3339(now)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|_| err("Invalid timestamp."))
}
fn stamp(d: DateTime<Utc>) -> String {
    d.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
}
fn opt_date(d: &Option<String>) -> FbResult<Option<String>> {
    d.as_deref()
        .map(|s| {
            fb_db::dates::parse_iso_date(s)
                .filter(|c| c == s)
                .ok_or_else(|| err("Enter a valid date."))
        })
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
pub fn begin_sync(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    connection_id: &str,
    now: &str,
    min_interval_secs: i64,
    force: bool,
) -> FbResult<i64> {
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
        // A last sync recorded later than this clock says the clock is wrong now or was then
        // ([`clock_warning`]); it never holds syncs back.
        if let Some(last) = last
            && !force
            && ok
            && last <= now
        {
            let since = now - last;
            let interval = chrono::Duration::try_seconds(min_interval_secs).ok_or_else(|| err("The minimum sync interval is out of range."))?;
            if since < interval {
                let remaining = interval.checked_sub(&since).ok_or_else(|| err("The minimum sync interval is out of range."))?;
                let minutes = remaining.num_microseconds().map(|m| m.div_euclid(60_000_000)).unwrap_or(0) + 1;
                return Err(err(format!("Synced {}. SimpleFIN asks apps not to pull too often; try again in {minutes} min.", ago(since))));
            }
        }
        Ok(window_start(q, co, connection_id, now)?.timestamp())
    })
}
fn back(from: DateTime<Utc>, days: i64) -> FbResult<DateTime<Utc>> {
    from.checked_sub_signed(chrono::Duration::days(days)).ok_or_else(|| err("Invalid timestamp."))
}
/// Where the next window starts (`_window`), never earlier than the provider's [`MAX_WINDOW_DAYS`] cap:
///
/// - From this connection's last COMPLETE sync less the overlap, never from the last attempt: a failed attempt
///   or a pull with provider errors downloaded nothing it can vouch for (review 2026-11-10, code-1 finding 3).
/// - A connection that never completed a sync: [`FIRST_SYNC_DAYS`], or further back (within the cap) to the
///   latest complete sync of a connection it may be replacing (another connection of the company that was
///   disconnected or needs reconnecting). Its downloaded accounts are not mapped yet, so which connection it
///   replaces is not known; the window is never shorter than the first-sync default for that reason.
/// - Once its accounts are mapped, the connections it replaced are known ([`replaced_last_good`]): when the
///   dates between their last complete sync (less the overlap) and this connection's first window can still
///   be reached, the next window reaches back to them; when the cap prevents it, that sync names the dates
///   ([`missing_dates`]) and later windows do not keep reaching (nothing more can be fetched).
fn window_start(q: &(impl Query + ?Sized), co: CompanyId, connection_id: &str, now: DateTime<Utc>) -> FbResult<DateTime<Utc>> {
    let cap = back(now, MAX_WINDOW_DAYS)?;
    let Some(good) = last_complete_sync(q, co, connection_id, now)? else {
        if complete_syncs(q, co, connection_id)? > 0 {
            // Every complete sync was recorded under a clock ahead of this one: none can be trusted.
            return Ok(cap);
        }
        let first = back(now, FIRST_SYNC_DAYS)?;
        return Ok(match candidate_last_good(q, co, connection_id, now)? {
            Some(earlier) => first.min(back(earlier, OVERLAP_DAYS)?).max(cap),
            None => first,
        });
    };
    let own = back(good, OVERLAP_DAYS)?;
    let cov = coverage_start(q, co, connection_id)?;
    let reach = replaced_reach(q, co, connection_id, now)?.filter(|want| cov.is_some_and(|cov| (*want).max(cap) < cov));
    Ok(reach.map_or(own, |want| own.min(want)).max(cap))
}
/// The latest complete sync among other connections of the company that a new connection may be replacing:
/// those disconnected or needing reconnect.
fn candidate_last_good(q: &(impl Query + ?Sized), co: CompanyId, connection_id: &str, now: DateTime<Utc>) -> FbResult<Option<DateTime<Utc>>> {
    let mut st = q.prepare("SELECT id FROM bankfeeds_connection WHERE company_id=?1 AND id<>?2 AND (active=0 OR needs_reconnect=1) ORDER BY id")?;
    let others = st.query_map(params![co.to_db(), connection_id], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    let mut latest = None;
    for other in others {
        latest = latest.max(last_complete_sync(q, co, &other, now)?);
    }
    Ok(latest)
}
/// The connections this one replaced: other connections of the company with a downloaded account that is,
/// or once was (`bankfeed.mapped`), mapped to a ledger account one of this connection's accounts maps to now.
fn replaced(q: &(impl Query + ?Sized), co: CompanyId, connection_id: &str) -> FbResult<Vec<String>> {
    let mut st = q.prepare(
        "SELECT DISTINCT o.connection_id FROM bankfeeds_account f JOIN bankfeeds_account o ON o.connection_id<>f.connection_id
           JOIN bankfeeds_connection oc ON oc.id=o.connection_id
          WHERE f.connection_id=?2 AND f.ledger_account_id IS NOT NULL AND oc.company_id=?1
            AND (o.ledger_account_id=f.ledger_account_id OR EXISTS(SELECT 1 FROM ledger_auditevent m WHERE m.company_id=?1 AND m.action='bankfeed.mapped'
                 AND m.target='bankfeedaccount:'||o.id AND json_extract(m.detail,'$.after')=f.ledger_account_id))
          ORDER BY o.connection_id",
    )?;
    Ok(st.query_map(params![co.to_db(), connection_id], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?)
}
/// The latest complete sync among the connections this one replaced ([`replaced`]).
pub(crate) fn replaced_last_good(q: &(impl Query + ?Sized), co: CompanyId, connection_id: &str, now: DateTime<Utc>) -> FbResult<Option<DateTime<Utc>>> {
    let mut latest = None;
    for other in replaced(q, co, connection_id)? {
        latest = latest.max(last_complete_sync(q, co, &other, now)?);
    }
    Ok(latest)
}
/// Where coverage of the replaced connections ended (their latest complete sync less the overlap), when this
/// connection's own windows began later than that: the dates in between were downloaded by neither.
fn replaced_reach(q: &(impl Query + ?Sized), co: CompanyId, connection_id: &str, now: DateTime<Utc>) -> FbResult<Option<DateTime<Utc>>> {
    let Some(cov) = coverage_start(q, co, connection_id)? else { return Ok(None) };
    let Some(good) = replaced_last_good(q, co, connection_id, now)? else { return Ok(None) };
    let want = back(good, OVERLAP_DAYS)?;
    Ok((want < cov).then_some(want))
}
/// The earliest window start among this connection's complete syncs: where its own downloads begin. None
/// when it has none, or when one carried no window (a pull that was not limited, or one recorded before
/// windows were audited): then nothing earlier is known to be missing.
fn coverage_start(q: &(impl Query + ?Sized), co: CompanyId, connection_id: &str) -> FbResult<Option<DateTime<Utc>>> {
    let (earliest, unbounded, n): (Option<String>, i64, i64) = q.query_row(
        &format!("SELECT min(json_extract(detail,'$.window_start')),coalesce(sum(json_type(detail,'$.window_start') IS NULL),0),count(*) FROM ledger_auditevent WHERE {COMPLETE_SYNC}"),
        params![co.to_db(), format!("bankfeedconnection:{connection_id}")],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if n == 0 || unbounded > 0 {
        return Ok(None);
    }
    earliest.as_deref().map(parse_now).transpose()
}
/// A `bankfeed.synced` event of connection `?2` in company `?1` that stored a pull with no provider errors.
const COMPLETE_SYNC: &str =
    "company_id=?1 AND action='bankfeed.synced' AND target=?2 AND json_extract(detail,'$.ok')=1 AND coalesce(json_extract(detail,'$.errors'),0)=0";
fn complete_syncs(q: &(impl Query + ?Sized), co: CompanyId, connection_id: &str) -> FbResult<i64> {
    Ok(q.query_row(&format!("SELECT count(*) FROM ledger_auditevent WHERE {COMPLETE_SYNC}"), params![co.to_db(), format!("bankfeedconnection:{connection_id}")], |r| r.get(0))?)
}
/// The host time of this connection's last complete sync: a stored pull (`bankfeed.synced`, `ok`) that
/// carried no provider errors. It is read from the append-only audit trail, which every stored pull has
/// written since bank feeds began, so a book from before this rule gets the right window with no backfill.
/// `at` is the pull's host time (recorded from this change on); older events fall back to their write time.
///
/// Numbered events use the connection's sync number (`sync`) for insertion order. Older, unnumbered events
/// follow by descending UUID; those time-based ids follow the clock, so their order is only a fallback,
/// not insertion order. A sync recorded with a time later than `now` (the clock was ahead then, or is
/// behind now) is passed over, so it can never move the window forward; [`clock_warning`] tells the user.
pub(crate) fn last_complete_sync(q: &(impl Query + ?Sized), co: CompanyId, connection_id: &str, now: DateTime<Utc>) -> FbResult<Option<DateTime<Utc>>> {
    let mut st = q.prepare(&format!(
        "SELECT coalesce(json_extract(detail,'$.at'),occurred_at) FROM ledger_auditevent WHERE {COMPLETE_SYNC}
          ORDER BY json_type(detail,'$.sync') IS NULL,json_extract(detail,'$.sync') DESC,id DESC"
    ))?;
    let mut rows = st.query(params![co.to_db(), format!("bankfeedconnection:{connection_id}")])?;
    while let Some(row) = rows.next()? {
        let at = parse_now(&row.get::<_, String>(0)?)?;
        if at <= now {
            return Ok(Some(at));
        }
    }
    Ok(None)
}
/// The computer's clock reads earlier than a bank sync this book recorded: either the clock is wrong now, or it
/// was wrong when that sync was recorded. New entries, audit records and the sync window all depend on it, so
/// the screens say so.
#[derive(Clone, Debug, PartialEq)]
pub struct ClockWarning {
    /// The latest recorded sync time, in the display zone (`YYYY-MM-DD HH:MM`).
    pub recorded: String,
    /// This computer's clock, in the display zone.
    pub now: String,
}
/// Clock slack before a recorded time counts as "later than this clock".
const CLOCK_SLACK_SECS: i64 = 10 * 60;
/// [`ClockWarning`] when an active connection of the company recorded its last sync attempt (the latest one written,
/// whatever its time says) later than `now`.
pub fn clock_warning(q: &(impl Query + ?Sized), co: CompanyId, now: &str) -> FbResult<Option<ClockWarning>> {
    let now = parse_now(now)?;
    let mut st = q.prepare("SELECT last_sync_at FROM bankfeeds_connection WHERE company_id=?1 AND active=1 AND last_sync_at IS NOT NULL")?;
    let times = st.query_map([co.to_db()], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    let mut latest: Option<DateTime<Utc>> = None;
    for t in times {
        latest = latest.max(Some(parse_now(&t)?));
    }
    Ok(latest.filter(|l| (*l - now).num_seconds() > CLOCK_SLACK_SECS).map(|l| ClockWarning { recorded: local_minute(l), now: local_minute(now) }))
}
/// The display zone the plugin dates transactions in (`FB_TIME_ZONE`, default America/New_York), so dates the
/// core names agree with the transactions' own dates.
fn zone() -> chrono_tz::Tz {
    std::env::var("FB_TIME_ZONE").ok().and_then(|z| z.parse().ok()).unwrap_or(chrono_tz::America::New_York)
}
/// The accounting date of an instant, in the display zone.
fn local_day(at: DateTime<Utc>) -> String {
    at.with_timezone(&zone()).format("%Y-%m-%d").to_string()
}
fn local_minute(at: DateTime<Utc>) -> String {
    at.with_timezone(&zone()).format("%Y-%m-%d %H:%M").to_string()
}
/// Dates this pull's window could not reach (review 2026-11-10, code-1 finding 3; 2026-11-11, findings 3 and 7),
/// as display-zone dates like the transactions' own: from where coverage should have resumed (this
/// connection's last complete sync less the overlap, or the replaced connections' when this connection began
/// later than they ended) to the day before the window start. A replaced connection's gap is named once.
fn missing_dates(q: &(impl Query + ?Sized), co: CompanyId, connection_id: &str, now: DateTime<Utc>, start: i64) -> FbResult<Option<(String, String)>> {
    let start = DateTime::<Utc>::from_timestamp(start, 0).ok_or_else(|| err("Invalid timestamp."))?;
    let mut from: Option<DateTime<Utc>> = None;
    let mut to: Option<DateTime<Utc>> = None;
    if let Some(good) = last_complete_sync(q, co, connection_id, now)? {
        let own = back(good, OVERLAP_DAYS)?;
        if start > own {
            from = Some(own);
            to = Some(start);
        }
        if let Some(want) = replaced_reach(q, co, connection_id, now)? {
            let cov = coverage_start(q, co, connection_id)?.map_or(start, |c| c.min(start));
            let reported: bool = q.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM ledger_auditevent WHERE {COMPLETE_SYNC} AND json_extract(detail,'$.missing_from')=?3)"),
                params![co.to_db(), format!("bankfeedconnection:{connection_id}"), local_day(want)],
                |r| r.get(0),
            )?;
            if want < cov && !reported {
                from = Some(from.map_or(want, |f| f.min(want)));
                to = Some(to.map_or(cov, |t| t.max(cov)));
            }
        }
    }
    let (Some(from), Some(to)) = (from, to) else { return Ok(None) };
    let last = to.checked_sub_signed(chrono::Duration::seconds(1)).ok_or_else(|| err("Invalid timestamp."))?;
    Ok(Some((local_day(from), local_day(last))))
}
/// Dates a sync could not reach: its window started after the last complete sync less the overlap, because
/// the provider window is capped at [`MAX_WINDOW_DAYS`]. Transactions dated `from`..=`to` may be missing.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncGap {
    pub connection_id: String,
    pub from: String,
    pub to: String,
}
/// Unreviewed sync gaps of a company, oldest first: each `bankfeed.synced` event naming missing dates
/// after the connection's last `bankfeed.gap_reviewed` (audit ids are time-ordered UUIDv7).
pub fn sync_gaps(q: &(impl Query + ?Sized), co: CompanyId) -> FbResult<Vec<SyncGap>> {
    let mut st = q.prepare(
        "SELECT substr(s.target,20),json_extract(s.detail,'$.missing_from'),json_extract(s.detail,'$.missing_to') FROM ledger_auditevent s
          WHERE s.company_id=?1 AND s.action='bankfeed.synced' AND json_type(s.detail,'$.missing_from')='text'
            AND NOT EXISTS(SELECT 1 FROM ledger_auditevent r WHERE r.company_id=s.company_id AND r.action='bankfeed.gap_reviewed' AND r.target=s.target AND r.id>s.id)
          ORDER BY s.id",
    )?;
    Ok(st
        .query_map([co.to_db()], |r| Ok(SyncGap { connection_id: r.get(0)?, from: r.get(1)?, to: r.get(2)? }))?
        .collect::<Result<Vec<_>, _>>()?)
}
/// The user compared the missing dates with the bank's statement: the notice is cleared (audited).
pub fn review_sync_gap(book: &mut Book, actor: UserId, co: CompanyId, connection_id: &str, secrets: &SecretSet) -> FbResult<()> {
    command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        connection_row(tx, co, connection_id)?;
        let gaps: Vec<SyncGap> = sync_gaps(tx, co)?.into_iter().filter(|g| g.connection_id == connection_id).collect();
        if gaps.is_empty() {
            return Ok(());
        }
        w.audit(&user, co, "bankfeed.gap_reviewed", format!("bankfeedconnection:{connection_id}"), json!({"gaps":gaps.iter().map(|g| json!({"from":g.from,"to":g.to})).collect::<Vec<_>>()}))
    })
}
/// `_ago`.
fn ago(delta: chrono::Duration) -> String {
    let minutes = delta
        .num_microseconds()
        .map(|m| m.div_euclid(60_000_000))
        .unwrap_or(i64::MAX);
    if minutes < 120 {
        format!("{minutes} min ago")
    } else {
        format!("{} h ago", minutes.div_euclid(60))
    }
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
        let summary = if removed > 0 {
            format!("{removed} control/format characters removed; {summary}")
        } else {
            summary
        };
        let summary = w.text(&written(secrets, &summary, 500));
        w.execute(
            "UPDATE bankfeeds_connection SET last_sync_at=?2,last_sync_ok=0,last_sync_summary=?3,sync_count=sync_count+1 WHERE id=?1",
            params![connection_id, stamp(parse_now(now)?), summary],
        )?;
        w.audit(
            &user,
            co,
            "bankfeed.synced",
            format!("bankfeedconnection:{connection_id}"),
            audit_removed(
                json!({"ok":false,"summary":summary,"scheduled":scheduled}),
                removed,
            ),
        )
    })
}

struct Stored {
    id: String,
    ordinal: i64,
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
    /// Decided rows whose amount the bank changed since (kept as decided, reported).
    bank_changed: i64,
}
/// Message of Django's `validate_unique` for `bankfeed_transaction_once`.
const DUPLICATE_IN_PULL: &str =
    "Bank feed transaction with this Feed account and Remote id already exists.";
/// Upsert one transaction of a feed account (the inner loop of `_store`). `inserted` holds the ids this
/// pull already created for the account: Python looks rows up in a map read before the loop, so an id
/// listed twice as new is inserted twice and `full_clean` refuses the whole store.
/// `t` is the redacted transaction and `identity` its raw id's key (FIX5): rows are found and kept apart by
/// the key, never by the displayed id.
fn upsert(
    w: &Writes<'_, '_>,
    feed_id: &str,
    t: &StagedTransaction,
    identity: &str,
    now: &str,
    counts: &mut Counts,
    inserted: Option<&mut Vec<String>>,
) -> FbResult<String> {
    let tx = w.tx;
    let posted_on = opt_date(&t.posted_on)?;
    let transacted_on = opt_date(&t.transacted_on)?;
    let existing = tx
        .query_row(
            "SELECT id,status,posted_on,transacted_on,amount,description,pending,ordinal,EXISTS(SELECT 1 FROM bankfeeds_link k WHERE k.item_ordinal=ordinal) FROM bankfeeds_transaction WHERE feed_account_id=?1 AND remote_identity=?2",
            params![feed_id, identity],
            |r| {
                Ok(Stored {
                    id: r.get(0)?,
                    ordinal: r.get(7)?,
                    // A row linked to an existing entry is decided like an accepted one.
                    status: if r.get::<_, bool>(8)? { "LINKED".to_owned() } else { r.get(1)? },
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
        let ordinal: i64 = tx.query_row(
            "SELECT coalesce((SELECT value FROM bankfeeds_counter WHERE name='transaction'),0)+1",
            [],
            |r| r.get(0),
        )?;
        let key = fb_ids::new_v7().to_string();
        w.execute(
            "INSERT INTO bankfeeds_transaction(id,ordinal,feed_account_id,remote_id,posted_on,transacted_on,amount,description,pending,first_seen_at,updated_at,status,accepted_as,journal_id,journal_line_id,reference,reference_url,decided_at,decided_by_id,return_count,remote_identity) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?10,'NEW','',NULL,NULL,'','',NULL,NULL,0,?11)",
            params![key, ordinal, feed_id, t.remote_id, posted_on, transacted_on, t.amount, t.description, t.pending, now, identity],
        )?;
        counts.new += 1;
        return Ok(key);
    };
    if row.status != "NEW" && row.status != "IGNORED" {
        // Decided values are frozen. A bank revision of the amount is recorded beside the row and reported,
        // and the books change only through the user (review 2026-11-10, code-1 finding 10).
        bank_change(w, &row, t.amount, counts)?;
        return Ok(row.id);
    }
    let changed = row.posted_on != posted_on
        || row.transacted_on != transacted_on
        || row.amount != t.amount
        || row.description != t.description
        || row.pending != t.pending;
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
/// Record (or clear) the bank's current amount for a decided row. A change the user chose to keep for this
/// same bank amount is not reported again.
fn bank_change(w: &Writes<'_, '_>, row: &Stored, amount: i64, counts: &mut Counts) -> FbResult<()> {
    let tx = w.tx;
    if row.amount == amount {
        w.execute("DELETE FROM bankfeeds_bank_change WHERE item_ordinal=?1", params![row.ordinal])?;
        counts.unchanged += 1;
        return Ok(());
    }
    check_transaction(&StagedTransaction { remote_id: "-".into(), posted_on: None, transacted_on: None, amount, description: String::new(), pending: false })?;
    let known: Option<(i64, bool)> =
        tx.query_row("SELECT amount,kept FROM bankfeeds_bank_change WHERE item_ordinal=?1", [row.ordinal], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    match known {
        Some((same, true)) if same == amount => {
            counts.unchanged += 1;
            return Ok(());
        }
        Some(_) => {
            w.execute("UPDATE bankfeeds_bank_change SET amount=?2,kept=0 WHERE item_ordinal=?1", params![row.ordinal, amount])?;
        }
        None => {
            w.execute("INSERT INTO bankfeeds_bank_change(item_ordinal,amount,kept) VALUES(?1,?2,0)", params![row.ordinal, amount])?;
        }
    }
    counts.bank_changed += 1;
    Ok(())
}
/// A decided transaction whose amount the bank changed after the decision.
#[derive(Clone, Debug, PartialEq)]
pub struct BankChange {
    pub transaction_id: String,
    pub ordinal: i64,
    pub date: Option<String>,
    pub description: String,
    pub recorded: i64,
    pub bank: i64,
    pub reference: String,
    pub reference_url: String,
}
/// The company's decided transactions the bank has changed and the user has not chosen to keep.
pub fn bank_changes(q: &(impl Query + ?Sized), co: CompanyId) -> FbResult<Vec<BankChange>> {
    let mut st = q.prepare(
        "SELECT t.id,t.ordinal,coalesce(t.posted_on,t.transacted_on),t.description,t.amount,g.amount,t.reference,t.reference_url FROM bankfeeds_bank_change g
          JOIN bankfeeds_transaction t ON t.ordinal=g.item_ordinal JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id
          WHERE c.company_id=?1 AND g.kept=0 ORDER BY t.posted_on DESC,t.ordinal DESC",
    )?;
    Ok(st
        .query_map([co.to_db()], |r| {
            Ok(BankChange {
                transaction_id: r.get(0)?,
                ordinal: r.get(1)?,
                date: r.get(2)?,
                description: r.get(3)?,
                recorded: r.get(4)?,
                bank: r.get(5)?,
                reference: r.get(6)?,
                reference_url: r.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?)
}
/// "Keep as recorded": the user looked at the bank's change and keeps the books as they are (audited). A later
/// different bank amount is reported again.
pub fn keep_recorded(book: &mut Book, actor: UserId, co: CompanyId, transaction_id: &str, secrets: &SecretSet) -> FbResult<()> {
    command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        let item = item(tx, co, transaction_id)?;
        let bank: i64 = tx
            .query_row("SELECT amount FROM bankfeeds_bank_change WHERE item_ordinal=?1 AND kept=0", [item.ordinal], |r| r.get(0))
            .optional()?
            .ok_or_else(|| err("The bank has not changed this transaction since it was decided."))?;
        w.execute("UPDATE bankfeeds_bank_change SET kept=1 WHERE item_ordinal=?1", params![item.ordinal])?;
        w.audit(&user, co, "bankfeed.bank_change_kept", format!("bankfeedtransaction:{}", item.id), json!({"recorded":money(item.amount),"bank":money(bank),"reference":item.reference}))
    })
}

fn active_connection(tx: &Writer<'_>, co: CompanyId, connection_id: &str) -> FbResult<Value> {
    let c = connection_row(tx, co, connection_id)?;
    if c["needs_reconnect"] == true {
        return Err(err(UNSAFE_CREDENTIAL));
    }
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
    store_pull_mode(
        book,
        actor,
        co,
        connection_id,
        pull,
        now,
        scheduled,
        window_start,
        secrets,
        false,
    )
}
#[allow(clippy::too_many_arguments)]
pub fn store_pull_for_desktop(
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
    store_pull_mode(
        book,
        actor,
        co,
        connection_id,
        pull,
        now,
        scheduled,
        window_start,
        secrets,
        true,
    )
}
#[allow(clippy::too_many_arguments)]
fn store_pull_mode(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    connection_id: &str,
    pull: &StagedPull,
    now: &str,
    scheduled: bool,
    window_start: Option<i64>,
    secrets: &SecretSet,
    automatic: bool,
) -> FbResult<Value> {
    let raw = pull;
    let removed = pull_removed(pull);
    let pull = &provider_pull(secrets, pull);
    command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        active_connection(tx, co, connection_id)?;
        let secret = identity_secret(w)?;
        let keys: Vec<AccountKeys> = raw
            .accounts
            .iter()
            .map(|a| account_keys(&secret, a))
            .collect();
        let now_text = stamp(parse_now(now)?);
        let mut counts = Counts::default();
        let mut auto_post_candidates = Vec::new();
        let mut accounts = 0i64;
        let mut skipped = pull.skipped.len() as i64;
        let mut pending_dropped = 0i64;
        let mut dropped_ids: Vec<String> = vec![];
        let mut reasons: Vec<String> = pull.skipped.clone();
        let window = window_start.map(epoch_text).transpose()?;
        let missing = match window_start {
            Some(start) => missing_dates(tx, co, connection_id, parse_now(now)?, start)?,
            None => None,
        };
        let mut recorded = 0i64;
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
            for (value, max) in [
                (&entry.name, 120),
                (&entry.org_name, 120),
                (&entry.currency, 200),
            ] {
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
                let before = (counts.new, counts.posted);
                let id = upsert(
                    w,
                    &feed_id,
                    t,
                    identity,
                    &py::now_utc(),
                    &mut counts,
                    Some(&mut inserted),
                )?;
                if counts.new > before.0 && already_recorded(tx, co, &item(tx, co, &id)?)?.is_some() {
                    recorded += 1;
                }
                if !t.pending && before != (counts.new, counts.posted) {
                    if automatic {
                        super::automatic::enqueue(w, co, &id)?;
                    }
                    auto_post_candidates.push(id);
                }
            }
            let mut st = tx.prepare("SELECT id,remote_id,first_seen_at,remote_identity FROM bankfeeds_transaction WHERE feed_account_id=?1 AND pending=1 AND status='NEW' ORDER BY ordinal")?;
            let stale = st
                .query_map([&feed_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            for (key, remote, first_seen, identity) in stale {
                if window
                    .as_ref()
                    .is_some_and(|w| first_seen.as_str() < w.as_str())
                    || keys.transactions.contains(&identity)
                {
                    continue;
                }
                w.execute(
                    "DELETE FROM bankfeeds_transaction WHERE id=?1",
                    params![key],
                )?;
                dropped_ids.push(remote);
                pending_dropped += 1;
            }
        }
        let mut messages = vec![format!(
            "{accounts} accounts, {} new transactions",
            counts.new
        )];
        if removed > 0 {
            messages.push(format!("{removed} control/format characters removed"));
        }
        if counts.posted > 0 {
            messages.push(format!("{} now posted", counts.posted));
        }
        if counts.updated > 0 {
            messages.push(format!("{} updated", counts.updated));
        }
        if pending_dropped > 0 {
            messages.push(format!("{pending_dropped} pending withdrawn (replaced by the posted item or dropped by the bank)"));
        }
        if recorded > 0 {
            messages.push(format!("{recorded} {} already recorded from an earlier download (review and link them)", if recorded == 1 { "looks" } else { "look" }));
        }
        if counts.bank_changed > 0 {
            messages.push(if counts.bank_changed == 1 {
                "1 accepted transaction was changed by the bank".to_owned()
            } else {
                format!("{} accepted transactions were changed by the bank", counts.bank_changed)
            });
        }
        if let Some((from, to)) = &missing {
            messages.push(format!("transactions dated {from} to {to} may be missing: the bank feed reaches back only {MAX_WINDOW_DAYS} days"));
        }
        if skipped > 0 {
            messages.push(format!(
                "{skipped} skipped ({})",
                reasons
                    .iter()
                    .take(5)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        for error in &pull.errors {
            messages.push(format!("SimpleFIN: {error}"));
        }
        let summary = w.text(&written(secrets, &messages.join("; "), 500));
        w.execute(
            "UPDATE bankfeeds_connection SET last_sync_at=?2,last_sync_ok=1,last_sync_summary=?3,sync_count=sync_count+1 WHERE id=?1",
            params![connection_id, now_text, summary],
        )?;
        let dropped: Vec<String> = dropped_ids
            .iter()
            .take(MAX_DROPPED_IDS_AUDITED)
            .map(|i| written(secrets, i, 200))
            .collect();
        let errors = pull.errors.len() as i64;
        w.audit(
            &user,
            co,
            "bankfeed.synced",
            format!("bankfeedconnection:{connection_id}"),
            audit_removed(sync_detail(json!({"ok":true,"scheduled":scheduled,"summary":summary,"pending_dropped_ids":dropped,"accounts":accounts,"new":counts.new,"updated":counts.updated,"posted":counts.posted,"unchanged":counts.unchanged,"skipped":skipped,"errors":errors,"pending_dropped":pending_dropped}), &now_text, sync_number(tx, connection_id)?, window.as_deref(), &missing, recorded, counts.bank_changed), removed),
        )?;
        Ok(
            json!({"auto_post_candidates":auto_post_candidates,"summary":summary,"already_recorded":recorded,"bank_changed":counts.bank_changed,"missing":missing.as_ref().map(|(from, to)| json!({"from":from,"to":to})),"provider_errors":pull.errors,"reasons":reasons.iter().map(|r| secrets.mask(r)).collect::<Vec<_>>(),"accounts":accounts,"new":counts.new,"updated":counts.updated,"posted":counts.posted,"unchanged":counts.unchanged,"skipped":skipped,"errors":errors,"pending_dropped":pending_dropped}),
        )
    })
}

/// The connection's sync number after this sync was counted: the insertion order of its sync events.
fn sync_number(q: &(impl Query + ?Sized), connection_id: &str) -> FbResult<i64> {
    Ok(q.query_row("SELECT sync_count FROM bankfeeds_connection WHERE id=?1", [connection_id], |r| r.get(0))?)
}
/// The sync-window evidence a stored pull adds to its `bankfeed.synced` event: the pull's host time (`at`), the
/// connection's sync number (`sync`, insertion order), its window start, and when it fell short of the last complete sync the dates that may be missing. Keys
/// only appear when they carry something, so a plain pull's event keeps its earlier shape plus `at`.
fn sync_detail(mut detail: Value, at: &str, sync: i64, window: Option<&str>, missing: &Option<(String, String)>, recorded: i64, bank_changed: i64) -> Value {
    detail["at"] = json!(at);
    detail["sync"] = json!(sync);
    if let Some(window) = window {
        detail["window_start"] = json!(window);
    }
    if let Some((from, to)) = missing {
        detail["missing_from"] = json!(from);
        detail["missing_to"] = json!(to);
    }
    if recorded > 0 {
        detail["already_recorded"] = json!(recorded);
    }
    if bank_changed > 0 {
        detail["bank_changed"] = json!(bank_changed);
    }
    detail
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
    let removed = transactions
        .iter()
        .map(|(a, t)| removed_characters(a) + transaction_removed(t))
        .sum();
    let transactions: Vec<(String, StagedTransaction)> = transactions
        .iter()
        .map(|(a, t)| {
            (
                provider_ingress(secrets, a),
                provider_transaction(secrets, t),
            )
        })
        .collect();
    command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        let c = active_connection(tx, co, connection_id)?;
        if c["needs_reconnect"] == true {
            return Err(err(UNSAFE_CREDENTIAL));
        }
        if c["active"] != true {
            return Err(err(
                "This connection was forgotten. Connect again with a new setup token.",
            ));
        }
        let secret = identity_secret(w)?;
        let mut counts = Counts::default();
        let mut out = vec![];
        for ((raw_account, raw_t), (_, t)) in raw.iter().zip(&transactions) {
            let account_key =
                fb_db::bankfeeds_text::identity_key(&secret, ACCOUNT_DOMAIN, raw_account);
            let mut st = tx.prepare("SELECT id FROM bankfeeds_account WHERE connection_id=?1 AND remote_identity=?2 AND active=1")?;
            let feeds = st
                .query_map(params![connection_id, account_key], |r| {
                    r.get::<_, String>(0)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let [feed_id] = feeds.as_slice() else {
                return Err(err(
                    "Sync the account list before staging its transactions.",
                ));
            };
            let identity =
                fb_db::bankfeeds_text::identity_key(&secret, TRANSACTION_DOMAIN, &raw_t.remote_id);
            out.push(upsert(
                w,
                feed_id,
                t,
                &identity,
                &py::now_utc(),
                &mut counts,
                None,
            )?);
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
